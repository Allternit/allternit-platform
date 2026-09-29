/**
 * Subscription Fabric client — gizzi's only way to the subscription gateway.
 *
 * The gateway runs on the user's Sessions computer (D15). gizzi never holds
 * its token: every call goes through allternit-api's forwarder
 * (`/api/v1/subscriptions/gateway/*`), which adds the token server-side and
 * enforces D16 (a current provider-terms acknowledgement + a single-use
 * human action per task). gizzi authenticates to allternit-api as the user
 * with its runtime token.
 */

export const FABRIC_PROVIDER_PREFIX = "subs-"
/** Request header carrying the send's human action (minted by the chat bridge). */
export const HUMAN_ACTION_HEADER = "x-allternit-human-action"

export function isFabricProviderID(providerID: string): boolean {
  return providerID.startsWith(FABRIC_PROVIDER_PREFIX)
}

function apiOrigin(): string | undefined {
  const origin = (process.env.ALLTERNIT_API_URL || "").trim()
  return origin ? origin.replace(/\/+$/, "") : undefined
}

function apiToken(): string | undefined {
  return process.env.ALLTERNIT_API_TOKEN?.trim() || process.env.ALLTERNIT_API_KEY?.trim() || undefined
}

export function fabricConfigured(): boolean {
  return Boolean(apiOrigin() && apiToken())
}

export class FabricError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code?: string,
    readonly body?: unknown,
  ) {
    super(message)
    this.name = "FabricError"
  }
}

export async function fabricFetch(
  method: string,
  path: string,
  opts: { body?: unknown; headers?: Record<string, string>; signal?: AbortSignal; accept?: string } = {},
): Promise<Response> {
  const origin = apiOrigin()
  const token = apiToken()
  if (!origin || !token) throw new FabricError("Allternit API is not configured (ALLTERNIT_API_URL / token)", 0)
  const headers: Record<string, string> = {
    Authorization: `Bearer ${token}`,
    ...(opts.accept ? { Accept: opts.accept } : {}),
    ...(opts.body !== undefined ? { "Content-Type": "application/json" } : {}),
    ...opts.headers,
  }
  return fetch(`${origin}/api/v1/subscriptions/gateway${path}`, {
    method,
    headers,
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
    signal: opts.signal,
  })
}

export async function fabricJson<T>(
  method: string,
  path: string,
  opts: { body?: unknown; headers?: Record<string, string>; signal?: AbortSignal } = {},
): Promise<T> {
  const res = await fabricFetch(method, path, opts)
  const text = await res.text()
  let body: any = undefined
  try {
    body = text ? JSON.parse(text) : undefined
  } catch {
    body = text
  }
  if (!res.ok) {
    const code = typeof body?.error === "string" ? body.error : undefined
    throw new FabricError(describeError(res.status, code, body), res.status, code, body)
  }
  return body as T
}

/** Plain-language errors for the chat surface (Register 1). */
export function describeError(status: number, code: string | undefined, body: any): string {
  const provider = typeof body?.provider === "string" ? body.provider : "this provider"
  switch (code) {
    case "disclosure_required":
      return `Before using your ${provider} subscription, read and acknowledge the subscription disclosure.`
    case "human_action_required":
    case "human_action_invalid":
      return "Subscription models only run when you send a message yourself. Send it again from the chat."
    case "sessions_computer_not_bound":
      return "No Sessions computer is set up for subscriptions yet."
    case "sessions_computer_missing":
      return "Your Sessions computer was not found."
    case "sessions_computer_not_running":
      return "Your Sessions computer is not running. Start it and send again."
    case "gateway_unreachable":
      return "The subscription gateway on your Sessions computer is not reachable."
    default:
      return `Subscription request failed (${status}${code ? ` ${code}` : ""}).`
  }
}

export interface SseMessage {
  id?: string
  event: string
  data: string
}

/** Parse an SSE body into messages. Comments (heartbeats) are skipped. */
export async function* readSse(body: ReadableStream<Uint8Array>, signal?: AbortSignal): AsyncGenerator<SseMessage> {
  const reader = body.getReader()
  // An aborted turn stops reading at once, even if the server holds the stream open.
  const stop = () => void reader.cancel().catch(() => {})
  signal?.addEventListener("abort", stop, { once: true })
  const decoder = new TextDecoder()
  let buffer = ""
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      let sep: number
      while ((sep = buffer.search(/\r?\n\r?\n/)) !== -1) {
        const block = buffer.slice(0, sep)
        buffer = buffer.slice(sep).replace(/^\r?\n\r?\n/, "")
        const msg: SseMessage = { event: "message", data: "" }
        const data: string[] = []
        for (const line of block.split(/\r?\n/)) {
          if (!line || line.startsWith(":")) continue
          const idx = line.indexOf(":")
          const field = idx === -1 ? line : line.slice(0, idx)
          const value = idx === -1 ? "" : line.slice(idx + 1).replace(/^ /, "")
          if (field === "event") msg.event = value
          else if (field === "data") data.push(value)
          else if (field === "id") msg.id = value
        }
        if (data.length === 0) continue
        msg.data = data.join("\n")
        yield msg
      }
    }
  } finally {
    signal?.removeEventListener("abort", stop)
    reader.releaseLock()
  }
}
