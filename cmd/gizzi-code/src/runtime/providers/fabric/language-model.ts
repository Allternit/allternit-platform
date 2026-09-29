/**
 * SubscriptionFabricLanguageModel — a gizzi LanguageModelV2 backed by the
 * Subscription Fabric (a ChatGPT/Claude/Kimi web subscription driven on the
 * user's Sessions computer).
 *
 * - Thread mapping: the gizzi session id is the fabric thread id. The first
 *   turn is `chat.create`, later turns `chat.continue` (the provider keeps
 *   the conversation, so only the new user turn is sent).
 * - D16: the turn carries the human action the chat bridge minted for this
 *   send; allternit-api stamps it into the task. Without one the forwarder
 *   refuses, so nothing here can start a task on its own.
 * - Streaming: gateway `reply.text.delta` events → text deltas as they
 *   arrive; `done.text` is authoritative for anything not yet streamed.
 * - Abort → `POST /v1/tasks/:id/cancel`. A dropped event stream reconnects
 *   (the gateway replays missed events) until the task is terminal.
 */

import type { LanguageModelV2, LanguageModelV2StreamPart } from "@ai-sdk/provider"
import { Log } from "@/shared/util/log"
import { Token } from "@/shared/util/token"
import { resolveTaskSessionID } from "@/runtime/session/stream-context"
import { FabricError, HUMAN_ACTION_HEADER, describeError } from "./client"
import { cancelFabricTask, followFabricTask, submitFabricTask, type FabricTask, type FabricTaskBody } from "./tasks"

const log = Log.create({ service: "fabric-lm" })

export class SubscriptionFabricLanguageModel implements LanguageModelV2 {
  readonly specificationVersion = "v2" as const
  readonly provider = "subscription-fabric"
  readonly supportedUrls: Record<string, RegExp[]> = {}

  constructor(
    private readonly providerID: string,
    /** gateway provider id (chatgpt, claude, kimi) */
    private readonly fabricProvider: string,
    /** model class (fast, standard, reasoning, …) */
    readonly modelId: string,
  ) {}

  async doGenerate(options: any): Promise<any> {
    const chunks: string[] = []
    const { stream } = await this.doStream(options)
    const reader = stream.getReader()
    let usage = { inputTokens: 0, outputTokens: 0, totalTokens: 0 }
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      if (value.type === "text-delta") chunks.push(value.delta)
      if (value.type === "error") throw value.error
      if (value.type === "finish") usage = value.usage as typeof usage
    }
    return {
      content: [{ type: "text", text: chunks.join("") }],
      finishReason: "stop" as const,
      usage,
      warnings: [],
      response: { id: "subscription-fabric", timestamp: new Date(), modelId: this.modelId },
    }
  }

  async doStream(options: any): Promise<{
    stream: ReadableStream<LanguageModelV2StreamPart>
    rawCall: { rawPrompt: unknown; rawSettings: Record<string, unknown> }
  }> {
    const headers: Record<string, string | undefined> = options?.headers ?? {}
    const prompt = lastUserText(options.prompt ?? [])
    const sessionID = resolveTaskSessionID(headers)
    const humanAction = headers[HUMAN_ACTION_HEADER]
    const requestID = headers["x-gizzi-request"] ?? `${Date.now().toString(36)}`
    const abortSignal: AbortSignal | undefined = options.abortSignal
    const continuing = hasAssistantTurn(options.prompt ?? [])
    const fabricProvider = this.fabricProvider
    const modelClass = this.modelId

    const stream = new ReadableStream<LanguageModelV2StreamPart>({
      start: async (controller) => {
        controller.enqueue({ type: "stream-start", warnings: [] })
        let textOpen = false
        let streamed = ""
        let taskID: string | undefined
        const onAbort = () => {
          if (!taskID) return
          void cancelFabricTask(taskID)
        }
        abortSignal?.addEventListener("abort", onAbort, { once: true })

        const text = (delta: string) => {
          if (!delta) return
          if (!textOpen) {
            textOpen = true
            controller.enqueue({ type: "text-start", id: "text-1" })
          }
          streamed += delta
          controller.enqueue({ type: "text-delta", id: "text-1", delta })
        }
        const finish = (reason: "stop" | "error" | "other") => {
          if (textOpen) controller.enqueue({ type: "text-end", id: "text-1" })
          const inputTokens = Token.estimate(prompt)
          const outputTokens = Token.estimate(streamed)
          controller.enqueue({
            type: "finish",
            finishReason: reason,
            usage: { inputTokens, outputTokens, totalTokens: inputTokens + outputTokens },
            providerMetadata: { gizzi: { usageEstimated: true }, fabric: { taskID: taskID ?? null } },
          } as LanguageModelV2StreamPart)
        }
        const fail = (error: unknown) => {
          controller.enqueue({ type: "error", error: error instanceof Error ? error : new Error(String(error)) })
          finish("error")
        }

        try {
          if (!prompt) throw new Error("Nothing to send: the message has no text.")
          if (!sessionID) throw new Error("Subscription models need a chat session.")
          if (!humanAction) {
            throw new FabricError(describeError(403, "human_action_required", undefined), 403, "human_action_required")
          }
          const task = await submit({
            sessionID,
            prompt,
            humanAction,
            requestID,
            continuing,
            fabricProvider,
            modelClass,
            signal: abortSignal,
          })
          taskID = task.task_id
          if (abortSignal?.aborted) onAbort()

          const outcome = await followFabricTask(task.task_id, abortSignal, { onText: (delta) => text(delta) })
          if (outcome.status === "completed" || outcome.status === "partial") {
            const full = outcome.result?.text ?? ""
            if (full.startsWith(streamed)) text(full.slice(streamed.length))
            else if (!streamed) text(full)
            else log.warn("final text diverged from the streamed text", { taskID })
            finish("stop")
            return
          }
          if (outcome.status === "cancelled") {
            finish("stop")
            return
          }
          fail(new Error(taskFailureMessage(outcome, fabricProvider)))
        } catch (error) {
          if (abortSignal?.aborted) {
            finish("stop")
          } else {
            log.error("fabric turn failed", { error: error instanceof Error ? error.message : String(error), taskID })
            fail(error)
          }
        } finally {
          abortSignal?.removeEventListener("abort", onAbort)
          controller.close()
        }
      },
    })

    return {
      stream,
      rawCall: { rawPrompt: prompt, rawSettings: { provider: this.providerID, modelClass } },
    }
  }
}

async function submit(input: {
  sessionID: string
  prompt: string
  humanAction: string
  requestID: string
  continuing: boolean
  fabricProvider: string
  modelClass: string
  signal?: AbortSignal
}): Promise<FabricTask> {
  // One idempotency key per send: the continue→create fallback reuses the
  // same human action (allternit-api allows a retry of the same submission).
  const body = (capability: "chat.create" | "chat.continue"): FabricTaskBody => ({
    capability,
    prompt: input.prompt,
    thread_id: input.sessionID,
    options: { model_class: input.modelClass },
    routing: { provider: input.fabricProvider },
    priority: "interactive",
    idempotency_key: `gizzi-${input.sessionID}-${input.requestID}`,
  })
  const send = (capability: "chat.create" | "chat.continue") =>
    submitFabricTask(body(capability), input.humanAction, input.signal)
  if (!input.continuing) return send("chat.create")
  try {
    return await send("chat.continue")
  } catch (error) {
    // The session has turns but none on this provider yet (model switched
    // mid-conversation): start the provider thread now.
    if (error instanceof FabricError && error.code === "thread_not_mapped") return send("chat.create")
    throw error
  }
}

function taskFailureMessage(task: FabricTask, provider: string): string {
  const name = provider === "chatgpt" ? "ChatGPT" : provider === "claude" ? "Claude" : provider === "kimi" ? "Kimi" : provider
  if (task.status === "needs_user") {
    return `${name} needs you: ${task.status_detail ?? task.error?.user_action ?? "open your Sessions computer to continue"}.`
  }
  const detail = task.error?.detail ?? task.status_detail
  return `${name} could not finish this reply${detail ? `: ${detail}` : ""}.`
}

function lastUserText(prompt: any[]): string {
  for (let i = prompt.length - 1; i >= 0; i--) {
    const msg = prompt[i]
    if (msg?.role !== "user") continue
    const text = Array.isArray(msg.content)
      ? msg.content
          .filter((p: any) => p.type === "text")
          .map((p: any) => String(p.text ?? ""))
          .join("")
      : String(msg.content ?? "")
    if (text.trim()) return text
  }
  return ""
}

function hasAssistantTurn(prompt: any[]): boolean {
  return prompt.some((m) => m?.role === "assistant")
}
