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
 * - Progress: gateway `submitted` / `progress` / `progress.heartbeat` /
 *   `artifact.ready` events → raw `__gizzi: "progress"` parts. The processor
 *   publishes them as `session.progress` (a live status line, not stored on
 *   the message) and the chat bridges forward them as `progress` frames.
 * - Artifacts: when the task ends, each `result.artifact_ids` entry is
 *   downloaded through the forwarder, its bytes checked against the
 *   gateway's sha256, and emitted as an AI SDK `file` part (preceded by a raw
 *   `generated_file` part with its name/title) → a FilePart on the assistant
 *   message → an artifact card in the chat. A file over the inline limit is
 *   not held in memory: it becomes a FilePart that points at its forwarder
 *   download URL.
 */

import { createHash } from "node:crypto"

import type { LanguageModelV2, LanguageModelV2StreamPart } from "@ai-sdk/provider"
import { Log } from "@/shared/util/log"
import { Token } from "@/shared/util/token"
import { resolveTaskSessionID } from "@/runtime/session/stream-context"
import { FabricError, HUMAN_ACTION_HEADER, describeError, fabricFetch, fabricJson, readSse } from "./client"

const log = Log.create({ service: "fabric-lm" })

const TERMINAL = new Set(["completed", "partial", "failed", "needs_user", "cancelled"])
const MAX_RECONNECTS = 20
/** Files up to this size are inlined into the message (data URL). */
export const FABRIC_INLINE_LIMIT_BYTES = 24 * 1024 * 1024

export interface FabricProgress {
  label?: string
  fraction?: number
  elapsedS?: number
}

interface FabricArtifactMeta {
  artifact_id: string
  type?: string
  mime_type?: string | null
  format?: string | null
  title?: string | null
  storage?: { sha256?: string | null; size_bytes?: number | null }
}

interface FabricTask {
  task_id: string
  status: string
  status_detail: string | null
  result: { text?: string; artifact_ids: string[] } | null
  error: { class?: string; detail?: string; user_action?: string } | null
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
        let streamed = ""
        let taskID: string | undefined
        const onAbort = () => {
          if (!taskID) return
          fabricFetch("POST", `/v1/tasks/${taskID}/cancel`, { body: {} }).catch((error) =>
            log.warn("cancel failed", { taskID, error: String(error) }),
          )
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
        const progress = (p: FabricProgress) => {
          controller.enqueue({ type: "raw", raw: { __gizzi: "progress", ...p } } as unknown as LanguageModelV2StreamPart)
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

          const outcome = await follow(
            task.task_id,
            abortSignal,
            (delta) => text(delta),
            (p) => progress(p),
            providerName(fabricProvider),
          )
          if (outcome.status === "completed" || outcome.status === "partial") {
            const full = outcome.result?.text ?? ""
            if (full.startsWith(streamed)) text(full.slice(streamed.length))
            else if (!streamed) text(full)
            else log.warn("final text diverged from the streamed text", { taskID })
            const ids = outcome.result?.artifact_ids ?? []
            if (ids.length > 0) {
              progress({ label: ids.length === 1 ? "Downloading the file" : `Downloading ${ids.length} files` })
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
                if (textOpen) {
                  controller.enqueue({ type: "text-end", id: "text-1" })
                  textOpen = false
                }
                for (const part of parts) controller.enqueue(part)
              }
              if (missing.length > 0) {
                const noun = missing.length === 1 ? "A file" : `${missing.length} files`
                text(
                  `${streamed ? "\n\n" : ""}${noun} from this reply could not be downloaded intact. ` +
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
  const body = (capability: "chat.create" | "chat.continue") => ({
    capability,
    prompt: input.prompt,
    thread_id: input.sessionID,
    options: { model_class: input.modelClass },
    routing: { provider: input.fabricProvider },
    priority: "interactive",
    idempotency_key: `gizzi-${input.sessionID}-${input.requestID}`,
  })
  const send = (capability: "chat.create" | "chat.continue") =>
    fabricJson<FabricTask>("POST", "/v1/tasks", {
      body: body(capability),
      headers: { [HUMAN_ACTION_HEADER]: input.humanAction },
      signal: input.signal,
    })
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

async function follow(
  taskID: string,
  signal: AbortSignal | undefined,
  onText: (delta: string) => void,
  onProgress: (progress: FabricProgress) => void = () => {},
  providerLabel = "The provider",
): Promise<FabricTask> {
  for (let attempt = 0; attempt <= MAX_RECONNECTS; attempt++) {
    try {
      const res = await fabricFetch("GET", `/v1/tasks/${taskID}/events`, {
        accept: "text/event-stream",
        signal,
      })
      if (!res.ok || !res.body) {
        const body = await res.text().catch(() => "")
        throw new FabricError(`event stream failed (${res.status}) ${body.slice(0, 200)}`, res.status)
      }
      for await (const msg of readSse(res.body, signal)) {
        let payload: any
        try {
          payload = JSON.parse(msg.data)
        } catch {
          continue
        }
        if (msg.event === "reply" && payload?.event?.type === "reply.text.delta") {
          onText(String(payload.event.delta ?? ""))
          continue
        }
        const p = progressFromEvent(msg.event, payload, providerLabel)
        if (p) {
          onProgress(p)
          continue
        }
        if (msg.event === "task.status" && TERMINAL.has(payload?.status)) {
          return await fabricJson<FabricTask>("GET", `/v1/tasks/${taskID}`)
        }
      }
      if (signal?.aborted) throw new DOMException("aborted", "AbortError")
    } catch (error) {
      if (signal?.aborted) throw error
      log.warn("event stream dropped", { taskID, attempt, error: error instanceof Error ? error.message : String(error) })
    }
    // The stream ended without a terminal status: check, then reconnect.
    const task = await fabricJson<FabricTask>("GET", `/v1/tasks/${taskID}`)
    if (TERMINAL.has(task.status)) return task
    await new Promise((r) => setTimeout(r, Math.min(1000 * 2 ** attempt, 10_000)))
  }
  throw new Error("Lost the connection to the subscription task.")
}

/**
 * A gateway task event → the live status line, in plain words. `null` for
 * events that are not progress (reply text, status, needs_user, …).
 */
export function progressFromEvent(event: string, payload: any, providerLabel: string): FabricProgress | null {
  const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined)
  switch (event) {
    case "submitted":
      return { label: `${providerLabel} is working on it` }
    case "progress": {
      const label = typeof payload?.label === "string" && payload.label.trim() ? payload.label.trim() : undefined
      const fraction = num(payload?.fraction)
      return label || fraction !== undefined ? { label, fraction } : null
    }
    case "progress.heartbeat": {
      const elapsedS = num(payload?.elapsed_s)
      return elapsedS !== undefined ? { elapsedS } : null
    }
    case "artifact.ready": {
      const type = typeof payload?.meta?.type === "string" ? payload.meta.type : ""
      return { label: type === "image" ? "Image ready" : "File ready" }
    }
    default:
      return null
  }
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

function providerName(provider: string): string {
  return provider === "chatgpt" ? "ChatGPT" : provider === "claude" ? "Claude" : provider === "kimi" ? "Kimi" : provider
}

function taskFailureMessage(task: FabricTask, provider: string): string {
  const name = providerName(provider)
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
