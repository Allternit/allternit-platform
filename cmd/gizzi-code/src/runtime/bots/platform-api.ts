/**
 * Minimal authenticated client for the Allternit platform API (bots, threads,
 * agent sessions). Shared by the TUI pet HUD and `gizzi bot threads`.
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

export async function platformToken(): Promise<string | undefined> {
  const env = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (env) return env
  const stored = await Pairing.load().catch(() => undefined)
  return Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
}

export async function platformSignedIn(): Promise<boolean> {
  return (await platformToken()) !== undefined
}

export interface PlatformRequestOptions {
  signal?: AbortSignal
  /** Per-request timeout; turns can take minutes, lookups should not. */
  timeoutMs?: number
}

export async function platformRequest<T>(
  method: "GET" | "POST" | "PATCH" | "PUT" | "DELETE",
  path: string,
  body?: unknown,
  options: PlatformRequestOptions = {},
): Promise<T> {
  const token = await platformToken()
  if (!token) throw new PlatformSignedOutError()
  const signals = [options.signal, options.timeoutMs ? AbortSignal.timeout(options.timeoutMs) : undefined].filter(
    (s): s is AbortSignal => s !== undefined,
  )
  const response = await fetch(`${platformApiBase()}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/json",
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: signals.length === 0 ? undefined : signals.length === 1 ? signals[0] : AbortSignal.any(signals),
  })
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
