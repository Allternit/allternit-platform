/**
 * Minimal authenticated client for the Allternit platform API (bots, threads,
 * agent sessions). Shared by the TUI pet HUD and `gizzi agents bot threads`.
 *
 * Base URL: the Allternit gateway (ALLTERNIT_API_URL / GIZZI_GATEWAY_URL,
 * loopback :8013 by default — the API Desktop runs locally).
 * Credential: ALLTERNIT_API_TOKEN, else the runtime device token from
 * `gizzi login` / `gizzi pair` (accepted by the API as the paired user).
 * Desktop's own session stays in its keychain; it is never read here.
 *
 * Free of CLI/UI imports so bun tests can drive it with a stubbed fetch.
 */
import { ALLTERNIT_GATEWAY_BASE } from "@/shared/constants/allternitGateway"
import { Pairing } from "@/runtime/services/pairing/pairing"

export class PlatformApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
    this.name = "PlatformApiError"
  }
}

/** Raised when there is no usable credential; the UI shows "run gizzi login". */
export class PlatformSignedOutError extends Error {
  constructor() {
    super("Not signed in to Allternit. Run `gizzi login`.")
    this.name = "PlatformSignedOutError"
  }
}

export function platformApiBase(): string {
  return ALLTERNIT_GATEWAY_BASE
}

async function deviceToken(): Promise<string | undefined> {
  const stored = await Pairing.load().catch(() => undefined)
  return Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
}

/** Credentials in order of preference: an explicit env token, then the `gizzi login` device token. */
async function platformTokens(): Promise<string[]> {
  const env = process.env.ALLTERNIT_API_TOKEN?.trim()
  const device = await deviceToken()
  return [...new Set([env, device].filter((t): t is string => !!t))]
}

export async function platformToken(): Promise<string | undefined> {
  return (await platformTokens())[0]
}

export async function platformSignedIn(): Promise<boolean> {
  return (await platformToken()) !== undefined
}

export interface PlatformRequestOptions {
  signal?: AbortSignal
  /** Per-request timeout; turns can take minutes, lookups should not. */
  timeoutMs?: number
}

/**
 * Send one authenticated request and return the raw response (no status
 * check). Shared by `platformRequest` and the `/sync` event stream, which
 * reads the body incrementally.
 */
export async function platformFetch(
  method: "GET" | "POST" | "PATCH" | "PUT" | "DELETE",
  path: string,
  body?: unknown,
  options: PlatformRequestOptions & { accept?: string; headers?: Record<string, string> } = {},
): Promise<Response> {
  const tokens = await platformTokens()
  if (tokens.length === 0) throw new PlatformSignedOutError()
  const signals = [options.signal, options.timeoutMs ? AbortSignal.timeout(options.timeoutMs) : undefined].filter(
    (s): s is AbortSignal => s !== undefined,
  )
  const send = (token: string) =>
    fetch(`${platformApiBase()}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: options.accept ?? "application/json",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
        ...options.headers,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: signals.length === 0 ? undefined : signals.length === 1 ? signals[0] : AbortSignal.any(signals),
    })
  // A stale ALLTERNIT_API_TOKEN left in a shell shouldn't hide a good
  // `gizzi login`: on a rejected credential, try the next one.
  let response = await send(tokens[0]!)
  for (const next of tokens.slice(1)) {
    if (response.status !== 401 && response.status !== 403) break
    response = await send(next)
  }
  return response
}

export async function platformRequest<T>(
  method: "GET" | "POST" | "PATCH" | "PUT" | "DELETE",
  path: string,
  body?: unknown,
  options: PlatformRequestOptions = {},
): Promise<T> {
  const response = await platformFetch(method, path, body, options)
  if (!response.ok) {
    const text = await response.text().catch(() => "")
    let message = text
    try {
      const parsed = JSON.parse(text) as { message?: string; error?: string }
      message = parsed.message || parsed.error || text
    } catch {
      // plain-text error body
    }
    throw new PlatformApiError(response.status, message || `HTTP ${response.status}`)
  }
  if (response.status === 204) return undefined as T
  return (await response.json()) as T
}
