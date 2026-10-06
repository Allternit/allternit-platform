/**
 * SubscriptionFabricLanguageModel — a gizzi LanguageModelV2 backed by the
 * Subscription Fabric (a ChatGPT/Claude/Kimi web subscription driven on the
 * user's Sessions computer).
 *
 * - Thread mapping: the gizzi session id is the fabric thread id. The first
 *   turn is `chat.create`, later turns `chat.continue` (the provider keeps
 *   the conversation, so only the new user turn is sent).
 * - D16: the turn carries the human action the chat bridge minted for this
 *   send; allternit-api stamps it into the task. A turn without one (an
 *   agent, tool, subagent or bot prepared it) waits on a `subscription`
 *   approval card instead — the person confirms, allternit-api mints the
 *   action. A provider question mid-task (`needs_user`) goes to the same
 *   card; the person's answer is sent as the next turn. Nothing here can
 *   start or answer a task on its own.
 * - Streaming: gateway `reply.text.delta` events → text deltas as they
 *   arrive; `done.text` is authoritative for anything not yet streamed.
 * - Thought stream: until the reply starts, what the Sessions computer is
 *   doing (sending, queued, sent, the provider's step labels, still
 *   working) goes out as a reasoning part — see thoughts.ts.
 * - Files: a finished task's artifacts are downloaded through the forwarder,
 *   sha256-checked, and emitted after the reply text as a raw
 *   `__gizzi: "generated_file"` part (filename, title, origin) followed by
 *   the AI SDK `file` part. The processor stores them as FileParts on the
 *   assistant message; the chat bridges turn them into artifact cards.
 * - Abort → `POST /v1/tasks/:id/cancel`. A dropped event stream reconnects
 *   (the gateway replays missed events) until the task is terminal.
 */

import { createHash } from "crypto"
import type { LanguageModelV2, LanguageModelV2StreamPart } from "@ai-sdk/provider"
import { Log } from "@/shared/util/log"
import { Token } from "@/shared/util/token"
import { resolveTaskSessionID } from "@/runtime/session/stream-context"
import { FabricError, HUMAN_ACTION_HEADER, fabricFetch, fabricJson, providerDisplayName } from "./client"
import { askProviderQuestion, confirmSend } from "./human-gate"
import { cancelFabricTask, followFabricTask, submitFabricTask, type FabricTask, type FabricTaskBody, type FollowHandlers } from "./tasks"
import { createThoughtStream } from "./thoughts"

const log = Log.create({ service: "fabric-lm" })

/** Provider questions answered in one turn before it stops asking. */
const MAX_QUESTION_ROUNDS = 5
/** Files up to this size are inlined into the message (data URL). */
export const FABRIC_INLINE_LIMIT_BYTES = 24 * 1024 * 1024

interface FabricArtifactMeta {
  artifact_id: string
  type?: string
  mime_type?: string | null
  format?: string | null
  title?: string | null
  storage?: { sha256?: string | null; size_bytes?: number | null }
}

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
        // `streamed` is the current task's text; `emitted` the whole turn's.
        let streamed = ""
        let emitted = ""
        let taskID: string | undefined
        const onAbort = () => {
          if (!taskID) return
          void cancelFabricTask(taskID)
        }
        abortSignal?.addEventListener("abort", onAbort, { once: true })

        const thoughts = createThoughtStream((part) => controller.enqueue(part), providerDisplayName(fabricProvider))
        const text = (delta: string) => {
          if (!delta) return
          thoughts.end()
          if (!textOpen) {
            textOpen = true
            controller.enqueue({ type: "text-start", id: "text-1" })
          }
          streamed += delta
          emitted += delta
          controller.enqueue({ type: "text-delta", id: "text-1", delta })
        }
        const finish = (reason: "stop" | "error" | "other") => {
          thoughts.end()
          if (textOpen) controller.enqueue({ type: "text-end", id: "text-1" })
          const inputTokens = Token.estimate(prompt)
          const outputTokens = Token.estimate(emitted)
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
          // No send behind this turn: a person confirms it on a card first.
          const action =
            humanAction ??
            (await confirmSend({
              sessionID,
              provider: fabricProvider,
              modelClass,
              prompt,
              capability: continuing ? "chat.continue" : "chat.create",
              options: taskOptions(modelClass),
              signal: abortSignal,
            }))
          thoughts.sending()
          const handlers: FollowHandlers = {
            onText: (delta) => text(delta),
            onStatus: (status) => thoughts.status(status),
            onSubmitted: () => thoughts.submitted(),
            onProgress: (label) => thoughts.progress(label),
            onHeartbeat: (elapsedS) => thoughts.heartbeat(elapsedS),
          }
          const task = await submit({
            sessionID,
            prompt,
            humanAction: action,
            requestID,
            continuing,
            fabricProvider,
            modelClass,
            signal: abortSignal,
          })
          taskID = task.task_id
          if (abortSignal?.aborted) onAbort()

          let outcome = await followFabricTask(task.task_id, abortSignal, handlers)
          // A provider question pauses the task: a person answers it on the
          // card, the answer goes to the provider as the next turn.
          for (let round = 1; outcome.status === "needs_user" && round <= MAX_QUESTION_ROUNDS; round++) {
            const question = outcome.status_detail ?? outcome.error?.user_action ?? "It needs you to continue."
            let reply: { humanAction: string; answer: string }
            try {
              reply = await askProviderQuestion({
                sessionID,
                provider: fabricProvider,
                taskID: outcome.task_id,
                question,
                reason: outcome.error?.class,
                options: taskOptions(modelClass),
                signal: abortSignal,
              })
            } catch (error) {
              if (abortSignal?.aborted) throw error
              break
            }
            const next = await submit({
              sessionID,
              prompt: reply.answer,
              humanAction: reply.humanAction,
              requestID: `${requestID}-answer${round}`,
              continuing: true,
              fabricProvider,
              modelClass,
              signal: abortSignal,
            })
            taskID = next.task_id
            if (abortSignal?.aborted) onAbort()
            if (streamed && !streamed.endsWith("\n")) text("\n\n")
            streamed = ""
            outcome = await followFabricTask(next.task_id, abortSignal, handlers)
          }
          if (outcome.status === "completed" || outcome.status === "partial") {
            const full = outcome.result?.text ?? ""
            if (full.startsWith(streamed)) text(full.slice(streamed.length))
            else if (!streamed) text(full)
            else log.warn("final text diverged from the streamed text", { taskID })
            const ids = outcome.result?.artifact_ids ?? []
            if (ids.length > 0) {
              thoughts.progress(ids.length === 1 ? "Downloading the file" : `Downloading ${ids.length} files`)
              const missing: string[] = []
              for (const id of ids) {
                const parts = await fetchArtifact(id, abortSignal).catch((error) => {
                  log.warn("artifact download failed", { taskID, artifactID: id, error: String(error) })
                  return undefined
                })
                if (!parts) {
                  missing.push(id)
                  continue
                }
                // Close the reply text first so each file follows it in order.
                thoughts.end()
                if (textOpen) {
                  controller.enqueue({ type: "text-end", id: "text-1" })
                  textOpen = false
                }
                for (const part of parts) controller.enqueue(part)
              }
              if (missing.length > 0) {
                const noun = missing.length === 1 ? "A file" : `${missing.length} files`
                text(
                  `${emitted ? "\n\n" : ""}${noun} from this reply could not be downloaded intact. ` +
                    `They are still on your Sessions computer (${missing.join(", ")}).`,
                )
              }
            }
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

/** A chat task's options; the approval card binds to exactly these. */
function taskOptions(modelClass: string): Record<string, unknown> {
  return { model_class: modelClass }
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
    options: taskOptions(input.modelClass),
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
  const name = providerDisplayName(provider)
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

/**
 * One artifact → stream parts: a raw `generated_file` part (name, title,
 * origin) then the AI SDK `file` part with the verified bytes. Over the
 * inline limit: only the raw part, pointing at the forwarder download URL.
 * Throws when the bytes do not match the gateway's sha256.
 */
export async function fetchArtifact(artifactID: string, signal?: AbortSignal): Promise<LanguageModelV2StreamPart[]> {
  const id = encodeURIComponent(artifactID)
  const meta = await fabricJson<FabricArtifactMeta>("GET", `/v1/artifacts/${id}`, { signal }).catch(
    () => undefined as FabricArtifactMeta | undefined,
  )
  const title = meta?.title?.trim() || undefined
  const described = {
    __gizzi: "generated_file",
    title,
    sourceUri: `fabric-artifact://${artifactID}`,
  }
  const size = meta?.storage?.size_bytes
  if (typeof size === "number" && size > FABRIC_INLINE_LIMIT_BYTES) {
    const mediaType = meta?.mime_type || "application/octet-stream"
    return [
      {
        type: "raw",
        raw: {
          ...described,
          mediaType,
          filename: artifactFilename(artifactID, meta?.format),
          url: `/api/v1/subscriptions/gateway/v1/artifacts/${id}/download`,
        },
      } as unknown as LanguageModelV2StreamPart,
    ]
  }
  const res = await fabricFetch("GET", `/v1/artifacts/${id}/download`, { signal })
  if (!res.ok) throw new FabricError(`artifact download failed (${res.status})`, res.status)
  const bytes = new Uint8Array(await res.arrayBuffer())
  const expected = (res.headers.get("x-artifact-sha256") || meta?.storage?.sha256 || "").toLowerCase()
  const actual = createHash("sha256").update(bytes).digest("hex")
  if (!expected || expected !== actual) {
    throw new Error(expected ? `sha256 mismatch (expected ${expected}, got ${actual})` : "no sha256 to check against")
  }
  const mediaType = (res.headers.get("content-type") || meta?.mime_type || "application/octet-stream").split(";")[0].trim()
  const filename = dispositionFilename(res.headers.get("content-disposition")) ?? artifactFilename(artifactID, meta?.format)
  return [
    { type: "raw", raw: { ...described, mediaType, filename } } as unknown as LanguageModelV2StreamPart,
    { type: "file", mediaType, data: bytes } as LanguageModelV2StreamPart,
  ]
}

function artifactFilename(artifactID: string, format?: string | null): string {
  return format ? `${artifactID}.${format}` : artifactID
}

function dispositionFilename(header: string | null): string | undefined {
  const match = header ? /filename="?([^";]+)"?/i.exec(header) : null
  return match?.[1]?.trim() || undefined
}
