/**
 * Wire formats for person-only turns, chat only, no tools:
 *   - OpenAI Responses (`POST https://api.openai.com/v1/responses`, ChatGPT
 *     direct sign-in, §9.9): body builder, SSE event parser, error text.
 *   - cloud-api chat completions (`POST /v1/chat/completions` for `plan/…`
 *     and `copilot/…`, §9.3): body builder, SSE delta parser, error text.
 */

export type ChatMessage = { role: "system" | "user" | "assistant"; content: string }

export type TurnEvent =
  | { type: "delta"; text: string }
  | { type: "done"; inputTokens?: number; outputTokens?: number; incomplete?: string }
  | { type: "error"; code?: string; message: string }

/** Raw SSE frames: `{event, data}` per blank-line-separated block. */
export async function* sseFrames(body: ReadableStream<Uint8Array>): AsyncGenerator<{ event?: string; data: string }> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  const parse = (block: string) => {
    let event: string | undefined
    const data: string[] = []
    for (const raw of block.split("\n")) {
      const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw
      if (!line || line.startsWith(":")) continue
      const colon = line.indexOf(":")
      const field = colon === -1 ? line : line.slice(0, colon)
      let value = colon === -1 ? "" : line.slice(colon + 1)
      if (value.startsWith(" ")) value = value.slice(1)
      if (field === "event") event = value
      else if (field === "data") data.push(value)
    }
    return data.length ? { event, data: data.join("\n") } : undefined
  }
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n")
      let cut: number
      while ((cut = buffer.indexOf("\n\n")) !== -1) {
        const frame = parse(buffer.slice(0, cut))
        buffer = buffer.slice(cut + 2)
        if (frame) yield frame
      }
    }
    buffer += decoder.decode()
    const tail = parse(buffer)
    if (tail) yield tail
  } finally {
    reader.releaseLock()
  }
}

// ─── OpenAI Responses (ChatGPT direct) ────────────────────────────────────────

/**
 * The Responses body for one chat turn: `store:false`, `stream:true`, no
 * tools. System text goes in `instructions`; the history is replayed as
 * input messages (assistant turns as `output_text`).
 */
export function responsesBody(model: string, messages: ChatMessage[]): Record<string, unknown> {
  const system = messages.filter((m) => m.role === "system" && m.content.trim()).map((m) => m.content)
  const input = messages
    .filter((m) => m.role !== "system" && m.content.length > 0)
    .map((m) =>
      m.role === "assistant"
        ? { role: "assistant", content: [{ type: "output_text", text: m.content }] }
        : { role: "user", content: [{ type: "input_text", text: m.content }] },
    )
  return {
    model,
    input,
    ...(system.length ? { instructions: system.join("\n\n") } : {}),
    store: false,
    stream: true,
  }
}

const CHATGPT_ERROR_TEXT: Record<string, string> = {
  subscription_sharing_usage_limit_exceeded:
    "ChatGPT limit reached. Check ChatGPT Settings › Usage, or pick another model for now.",
  subscription_sharing_usage_unavailable: "ChatGPT could not check your plan's usage right now. Try again in a moment.",
  subscription_sharing_user_not_eligible: "Your ChatGPT plan cannot be used here (account, workspace or region).",
  subscription_sharing_unsupported_capability: "ChatGPT did not accept part of this message (an attachment or option it does not support here).",
  subscription_sharing_route_not_supported: "ChatGPT refused this request type.",
  subscription_sharing_invalid_user: "ChatGPT did not accept your sign-in. Run `gizzi connect chatgpt` to sign in again.",
  subscription_sharing_user_unavailable: "ChatGPT could not load your account right now. Try again in a moment.",
  chatpass_v2_scope_not_authorized: "Your ChatGPT sign-in does not allow plan use. Run `gizzi connect chatgpt` to sign in again.",
  chatpass_v2_invalid_authorization_context: "Your ChatGPT sign-in does not allow plan use. Run `gizzi connect chatgpt` to sign in again.",
}

/** The plain message for a ChatGPT failure: by error code, else by HTTP status. */
export function chatgptErrorText(code: string | undefined, status?: number, detail?: string): string {
  if (code && CHATGPT_ERROR_TEXT[code]) return CHATGPT_ERROR_TEXT[code]!
  if (status === 401) return "ChatGPT did not accept your sign-in. Run `gizzi connect chatgpt` to sign in again."
  if (status === 403) return `ChatGPT refused this request${detail ? `: ${detail}` : "."}`
  if (status === 429) return "ChatGPT limit reached. Check ChatGPT Settings › Usage, or pick another model for now."
  if (status === 503) return "ChatGPT plan use is not available right now. Try again in a moment."
  const tail = detail ? `: ${detail}` : code ? ` (${code})` : status ? ` (HTTP ${status})` : ""
  return `ChatGPT could not answer${tail}.`
}

/** Error code + detail from a non-2xx Responses reply (`{error:{code,message}}` or `{detail}`). */
export function readErrorBody(text: string): { code?: string; message?: string } {
  try {
    const body = JSON.parse(text) as { error?: unknown; detail?: unknown; message?: unknown }
    if (body.error && typeof body.error === "object") {
      const e = body.error as { code?: unknown; message?: unknown; type?: unknown }
      return {
        code: typeof e.code === "string" ? e.code : typeof e.type === "string" ? e.type : undefined,
        message: typeof e.message === "string" ? e.message : undefined,
      }
    }
    if (typeof body.error === "string") return { code: body.error, message: typeof body.message === "string" ? body.message : undefined }
    if (typeof body.detail === "string") return { message: body.detail }
  } catch {}
  return text.trim() ? { message: text.trim().slice(0, 300) } : {}
}

/**
 * Responses stream events → turn events. Text arrives as
 * `response.output_text.delta`; `response.completed` ends the turn with
 * usage; `response.failed` / `error` carry the code (usage limits arrive
 * this way after streaming started); `response.incomplete` ends early with
 * its reason. A stream that stops before any terminal event is an error.
 */
export async function* parseResponsesStream(body: ReadableStream<Uint8Array>): AsyncGenerator<TurnEvent> {
  for await (const frame of sseFrames(body)) {
    if (frame.data === "[DONE]") break
    let ev: any
    try {
      ev = JSON.parse(frame.data)
    } catch {
      continue
    }
    const type: string = ev?.type ?? frame.event ?? ""
    if (type === "response.output_text.delta") {
      if (typeof ev.delta === "string" && ev.delta) yield { type: "delta", text: ev.delta }
      continue
    }
    if (type === "response.completed") {
      const usage = ev.response?.usage
      yield { type: "done", inputTokens: num(usage?.input_tokens), outputTokens: num(usage?.output_tokens) }
      return
    }
    if (type === "response.incomplete") {
      const usage = ev.response?.usage
      yield {
        type: "done",
        inputTokens: num(usage?.input_tokens),
        outputTokens: num(usage?.output_tokens),
        incomplete: ev.response?.incomplete_details?.reason ?? "incomplete",
      }
      return
    }
    if (type === "response.failed") {
      const code: string | undefined = ev.response?.error?.code ?? undefined
      yield { type: "error", code, message: chatgptErrorText(code, undefined, ev.response?.error?.message) }
      return
    }
    if (type === "error") {
      const code: string | undefined = ev.code ?? ev.error?.code ?? undefined
      yield { type: "error", code, message: chatgptErrorText(code, undefined, ev.message ?? ev.error?.message) }
      return
    }
  }
  yield { type: "error", code: "stream_ended", message: "ChatGPT stopped before the reply finished. Try again." }
}

// ─── cloud-api chat completions (plan/…, copilot/…) ───────────────────────────

export function chatCompletionsBody(model: string, messages: ChatMessage[]): Record<string, unknown> {
  return {
    model,
    messages: messages.filter((m) => m.content.length > 0).map((m) => ({ role: m.role, content: m.content })),
    stream: true,
    stream_options: { include_usage: true },
  }
}

const PLAN_ERROR_TEXT: Record<string, string> = {
  plan_person_only: "Plans answer only messages you send yourself in chat, while signed in as yourself.",
  plan_rate: "Too many plan messages in the last few minutes. Wait a moment and try again.",
  plan_turn_in_flight: "Your plan is still answering the last message. Wait for it to finish.",
  plan_not_connected: "That plan is not connected. Run `gizzi connect` to connect it.",
  plan_no_tools: "Plans are chat only and cannot take tool results.",
  runner_busy: "Busy. Try again",
  could_not_start: "Could not start. Try again",
  login_expired: "Sign-in expired. Run `gizzi connect` to sign in to that plan again.",
  vendor_limited: "The vendor limited this plan.",
}

/** The plain message for a refused plan turn. The server's own message wins when it sends one. */
export function planErrorText(code: string | undefined, status: number, serverMessage?: string): string {
  if (serverMessage && serverMessage.trim()) return serverMessage.trim()
  if (code && PLAN_ERROR_TEXT[code]) return PLAN_ERROR_TEXT[code]!
  if (status === 401) return "Allternit did not accept your sign-in."
  return `The plan could not answer (HTTP ${status}).`
}

/** OpenAI-compatible SSE deltas → turn events (`choices[0].delta.content`, final `usage`). */
export async function* parseChatCompletionsStream(body: ReadableStream<Uint8Array>): AsyncGenerator<TurnEvent> {
  let usage: { inputTokens?: number; outputTokens?: number } = {}
  let finished = false
  for await (const frame of sseFrames(body)) {
    if (frame.data === "[DONE]") {
      yield { type: "done", ...usage }
      return
    }
    let ev: any
    try {
      ev = JSON.parse(frame.data)
    } catch {
      continue
    }
    if (ev?.error) {
      const code = typeof ev.error === "string" ? ev.error : ev.error.code
      const message = typeof ev.error === "object" ? ev.error.message : ev.message
      yield { type: "error", code, message: planErrorText(code, 200, message) }
      return
    }
    if (ev?.usage) usage = { inputTokens: num(ev.usage.prompt_tokens), outputTokens: num(ev.usage.completion_tokens) }
    const choice = ev?.choices?.[0]
    const text = choice?.delta?.content
    if (typeof text === "string" && text) yield { type: "delta", text }
    if (choice?.finish_reason) finished = true
  }
  if (finished) {
    yield { type: "done", ...usage }
    return
  }
  yield { type: "error", code: "stream_ended", message: "The plan stopped before the reply finished. Try again." }
}

function num(v: unknown): number | undefined {
  return typeof v === "number" && Number.isFinite(v) ? v : undefined
}
