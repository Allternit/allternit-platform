/**
 * SiwcLanguageModel — a gizzi LanguageModelV2 that calls the public OpenAI
 * Responses API with the user's ChatGPT plan (Sign in with ChatGPT).
 *
 * Request shape follows OpenAI's documented preview limits: `POST
 * /v1/responses` with `store: false`, `stream: true`, history in `input`,
 * system text in `instructions`, and none of the unsupported fields
 * (temperature, max_output_tokens, previous_response_id, tools, …). A turn
 * succeeds only on `response.completed`; failures keep the HTTP status, error
 * code and request id.
 *
 * Fallback: if SIWC cannot serve the turn before any output (no signed-in
 * session, transport failure, 401, 5xx), the turn goes to the web-chat
 * subscription adapter for the same lane, when one is configured. Usage
 * limits, ineligibility and unsupported requests are NOT retried elsewhere —
 * they are the answer, and the user is told.
 */

import type { LanguageModelV2, LanguageModelV2StreamPart } from "@ai-sdk/provider"
import { Log } from "@/shared/util/log"
import { readSse } from "../fabric/client"
import { SIWC_RESPONSES_URL, siwcToken } from "./broker"

const log = Log.create({ service: "siwc-lm" })

const USAGE_URL = "https://chatgpt.com/settings/usage"
const DEFAULT_INSTRUCTIONS = "You are a helpful assistant."

/** Map a real model slug to a fabric model class for the web-chat fallback. */
export function fabricClassFor(slug: string): string {
  if (/mini|nano|fast|flash|lite/i.test(slug)) return "fast"
  if (/(^|[-_.])(o\d|pro|reason|think)/i.test(slug)) return "reasoning"
  return "standard"
}

export class SiwcError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code?: string,
    readonly requestID?: string,
    /** True when the web-chat adapter may serve this turn instead. */
    readonly fallbackEligible = false,
  ) {
    super(message)
    this.name = "SiwcError"
  }
}

/** Plain-language errors (Register 1) for the documented failure codes. */
export function describeSiwcError(status: number, code: string | undefined, detail: string | undefined): string {
  switch (code) {
    case "subscription_sharing_user_not_eligible":
      return "Your ChatGPT plan or workspace can't be used this way, so Allternit can't send this message through it."
    case "subscription_sharing_usage_limit_exceeded":
      return `Your ChatGPT plan's usage limit for Allternit is reached. Review or raise it at ${USAGE_URL}.`
    case "subscription_sharing_usage_unavailable":
    case "subscription_sharing_user_unavailable":
      return "ChatGPT plan usage couldn't be checked just now. Try again in a moment."
    case "subscription_sharing_unsupported_capability":
      return `This request uses something ChatGPT plan usage doesn't support${detail ? ` (${detail})` : ""}.`
    case "subscription_sharing_route_not_supported":
      return "ChatGPT plan usage doesn't support this kind of request."
    case "subscription_sharing_invalid_user":
      return "Allternit couldn't confirm your ChatGPT session. Sign in with ChatGPT again in Settings › Subscriptions."
    case "chatpass_v2_scope_not_authorized":
    case "chatpass_v2_invalid_authorization_context":
      return "ChatGPT plan usage isn't authorized for this request. Check the connection in Settings › Subscriptions."
  }
  switch (status) {
    case 401:
      return "ChatGPT didn't accept your sign-in. Sign in with ChatGPT again in Settings › Subscriptions."
    case 403:
      return `ChatGPT plan usage was refused${detail ? `: ${detail}` : ""}.`
    case 429:
      return `Your ChatGPT plan's usage limit is reached. Review it at ${USAGE_URL}.`
    case 503:
      return "ChatGPT plan usage is unavailable right now. Try again in a moment."
    default:
      return `ChatGPT request failed (${status}${code ? ` ${code}` : ""}).`
  }
}

async function readError(res: Response): Promise<SiwcError> {
  const requestID = res.headers.get("x-request-id") ?? undefined
  const text = await res.text().catch(() => "")
  let body: any
  try {
    body = text ? JSON.parse(text) : undefined
  } catch {
    body = undefined
  }
  // Direct admission can answer `{"detail":"..."}` instead of an error object:
  // diagnostic text only, never a machine code.
  const code: string | undefined = typeof body?.error?.code === "string" ? body.error.code : undefined
  const detail: string | undefined =
    typeof body?.error?.param === "string" ? body.error.param : typeof body?.detail === "string" ? body.detail : undefined
  const eligible = res.status === 401 || res.status >= 500
  return new SiwcError(
    `${describeSiwcError(res.status, code, detail)}${requestID ? ` (request ${requestID})` : ""}`,
    res.status,
    code,
    requestID,
    eligible && code !== "subscription_sharing_usage_unavailable" && code !== "subscription_sharing_user_unavailable",
  )
}

// ─── prompt → Responses input ────────────────────────────────────────────────

function textOf(content: unknown): string {
  if (typeof content === "string") return content
  if (!Array.isArray(content)) return ""
  return content
    .filter((p: any) => p?.type === "text")
    .map((p: any) => String(p.text ?? ""))
    .join("")
}

export function toResponsesBody(model: string, prompt: any[]): Record<string, unknown> {
  const instructions: string[] = []
  const input: any[] = []
  for (const msg of prompt) {
    if (msg?.role === "system") {
      const t = textOf(msg.content)
      if (t) instructions.push(t)
    } else if (msg?.role === "user") {
      const parts: any[] = []
      for (const p of Array.isArray(msg.content) ? msg.content : [{ type: "text", text: msg.content }]) {
        if (p?.type === "text" && p.text) parts.push({ type: "input_text", text: String(p.text) })
        else if (p?.type === "file" && String(p.mediaType ?? "").startsWith("image/") && typeof p.data === "string") {
          parts.push({
            type: "input_image",
            image_url: p.data.startsWith("data:") || p.data.startsWith("http") ? p.data : `data:${p.mediaType};base64,${p.data}`,
          })
        }
      }
      if (parts.length) input.push({ role: "user", content: parts })
    } else if (msg?.role === "assistant") {
      const t = textOf(msg.content)
      if (t) input.push({ role: "assistant", content: [{ type: "output_text", text: t }] })
    }
  }
  // Only the fields the ChatGPT plan route accepts.
  return {
    model,
    instructions: instructions.join("\n\n") || DEFAULT_INSTRUCTIONS,
    input,
    store: false,
    stream: true,
  }
}

// ─── the model ───────────────────────────────────────────────────────────────

export class SiwcLanguageModel implements LanguageModelV2 {
  readonly specificationVersion = "v2" as const
  readonly provider = "siwc"
  readonly supportedUrls: Record<string, RegExp[]> = {}

  constructor(
    private readonly providerID: string,
    /** the account's model slug */
    readonly modelId: string,
    /** The web-chat lane for the same provider, when configured. */
    private readonly fallback?: () => LanguageModelV2 | undefined,
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
      response: { id: "siwc", timestamp: new Date(), modelId: this.modelId },
    }
  }

  async doStream(options: any): Promise<{
    stream: ReadableStream<LanguageModelV2StreamPart>
    rawCall: { rawPrompt: unknown; rawSettings: Record<string, unknown> }
  }> {
    const abortSignal: AbortSignal | undefined = options.abortSignal
    let res: Response
    try {
      res = await this.open(options, abortSignal)
    } catch (error) {
      const lane = error instanceof SiwcError && !error.fallbackEligible ? undefined : this.fallback?.()
      if (!lane) throw error
      log.info("falling back to the web-chat adapter", { reason: error instanceof Error ? error.message : String(error) })
      return lane.doStream(options) as any
    }
    return { stream: this.consume(res, abortSignal), rawCall: { rawPrompt: options.prompt, rawSettings: { provider: this.providerID, model: this.modelId } } }
  }

  /** Everything that can fail before a stream opens. */
  private async open(options: any, signal?: AbortSignal): Promise<Response> {
    const token = await siwcToken(signal)
    if (!token) {
      throw new SiwcError("Sign in with ChatGPT in Settings › Subscriptions to use your ChatGPT plan.", 0, "not_signed_in", undefined, true)
    }
    let res: Response
    try {
      res = await fetch(SIWC_RESPONSES_URL, {
        method: "POST",
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json", Accept: "text/event-stream" },
        body: JSON.stringify(toResponsesBody(this.modelId, options.prompt ?? [])),
        signal,
      })
    } catch (error) {
      if (signal?.aborted) throw error
      throw new SiwcError("Couldn't reach ChatGPT.", 0, "network", undefined, true)
    }
    if (!res.ok || !res.body) throw await readError(res)
    return res
  }

  private consume(res: Response, signal?: AbortSignal): ReadableStream<LanguageModelV2StreamPart> {
    const requestID = res.headers.get("x-request-id") ?? undefined
    return new ReadableStream<LanguageModelV2StreamPart>({
      start: async (controller) => {
        controller.enqueue({ type: "stream-start", warnings: [] })
        let textOpen = false
        let done = false
        const closeText = () => {
          if (textOpen) controller.enqueue({ type: "text-end", id: "text-1" })
          textOpen = false
        }
        const finish = (reason: "stop" | "length" | "error", usage?: any) => {
          if (done) return
          done = true
          closeText()
          const inputTokens = Number(usage?.input_tokens ?? 0)
          const outputTokens = Number(usage?.output_tokens ?? 0)
          controller.enqueue({
            type: "finish",
            finishReason: reason,
            usage: { inputTokens, outputTokens, totalTokens: Number(usage?.total_tokens ?? inputTokens + outputTokens) },
            providerMetadata: { siwc: { requestID: requestID ?? null } },
          } as LanguageModelV2StreamPart)
        }
        const fail = (message: string, code?: string) => {
          if (done) return
          const detail = requestID ? `${message} (request ${requestID})` : message
          controller.enqueue({ type: "error", error: new SiwcError(detail, 200, code, requestID) })
          finish("error")
        }
        try {
          for await (const evt of readSse(res.body!, signal)) {
            let data: any
            try {
              data = JSON.parse(evt.data)
            } catch {
              continue
            }
            const type: string = data?.type ?? evt.event
            if (type === "response.output_text.delta" && typeof data.delta === "string" && data.delta) {
              if (!textOpen) {
                textOpen = true
                controller.enqueue({ type: "text-start", id: "text-1" })
              }
              controller.enqueue({ type: "text-delta", id: "text-1", delta: data.delta })
            } else if (type === "response.completed") {
              finish("stop", data.response?.usage)
            } else if (type === "response.incomplete") {
              finish("length", data.response?.usage)
            } else if (type === "response.failed") {
              const e = data.response?.error
              fail(describeSiwcError(e?.code === "subscription_sharing_usage_limit_exceeded" ? 429 : 500, e?.code, e?.param), e?.code)
            } else if (type === "error") {
              fail(data.code ? describeSiwcError(500, data.code, data.param) : String(data.message ?? "The ChatGPT stream reported an error."), data.code)
            }
            if (done) break
          }
          // Success is only response.completed; a stream that just ends is a failure.
          if (!done) {
            if (signal?.aborted) finish("stop")
            else fail("The ChatGPT stream ended before the reply finished.", "stream_interrupted")
          }
        } catch (error) {
          if (signal?.aborted) finish("stop")
          else fail(error instanceof Error ? error.message : String(error), "stream_error")
        } finally {
          controller.close()
        }
      },
    })
  }
}
