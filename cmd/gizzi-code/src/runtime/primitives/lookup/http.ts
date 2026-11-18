/**
 * The one fetch path for `lookup` providers (MET Norway, Frankfurter,
 * Nominatim, Wikimedia Commons, Openverse) and for link previews.
 *
 * - Every request goes through the SSRF guard (no loopback, private, link-local
 *   or metadata addresses), so a model-chosen URL cannot reach the machine's
 *   own network. Redirects are re-vetted per hop by the guard.
 * - An identifying User-Agent, as MET Norway and Nominatim require. It names
 *   the product and version only; never the user, machine or account.
 * - A timeout combined with the caller's abort signal.
 * - A small in-memory cache keyed by URL, for successful responses only.
 * - An optional per-host minimum interval (Nominatim allows 1 request/second).
 */
import * as SsrfGuard from "@/shared/utils/hooks/ssrfGuard"
import { Installation } from "@/shared/installation"
import { abortAfterAny } from "@/shared/util/abort"

export const LOOKUP_TIMEOUT_MS = 8_000
export const MAX_RESPONSE_BYTES = 2 * 1024 * 1024
const CACHE_MAX_ENTRIES = 200

/** "Allternit/<version> (+https://allternit.com)". No personal data. */
export function userAgent(): string {
  return `Allternit/${Installation.VERSION} (+https://allternit.com)`
}

/** Test seam: replace `transport.fetch` to stub the network. */
export const transport = {
  fetch: (input: string | URL | Request, init?: RequestInit) =>
    SsrfGuard.createGuardedFetch({ allowLoopback: false })(input, init),
}

export class LookupHttpError extends Error {
  constructor(
    readonly status: number,
    readonly url: string,
    message: string,
  ) {
    super(message)
    this.name = "LookupHttpError"
  }
}

export interface FetchOptions {
  signal?: AbortSignal
  timeoutMs?: number
  /** Cache a successful response for this long; 0 or unset means no cache. */
  ttlMs?: number
  headers?: Record<string, string>
  /** Wait so requests to this host are at least this far apart. */
  minIntervalMs?: number
  maxBytes?: number
}

export interface TextResponse {
  /** The final URL after redirects. */
  url: string
  status: number
  contentType: string
  text: string
}

type Entry = { expires: number; value: TextResponse }
const cache = new Map<string, Entry>()
const lastRequestAt = new Map<string, number>()

export function clearLookupCache(): void {
  cache.clear()
  lastRequestAt.clear()
}

function cacheGet(key: string): TextResponse | undefined {
  const hit = cache.get(key)
  if (!hit) return undefined
  if (hit.expires <= Date.now()) {
    cache.delete(key)
    return undefined
  }
  // Refresh recency (Map keeps insertion order; the oldest is evicted first).
  cache.delete(key)
  cache.set(key, hit)
  return hit.value
}

function cacheSet(key: string, value: TextResponse, ttlMs: number): void {
  cache.set(key, { expires: Date.now() + ttlMs, value })
  while (cache.size > CACHE_MAX_ENTRIES) cache.delete(cache.keys().next().value!)
}

async function spacing(host: string, minIntervalMs: number, signal: AbortSignal): Promise<void> {
  const wait = (lastRequestAt.get(host) ?? 0) + minIntervalMs - Date.now()
  lastRequestAt.set(host, Date.now() + Math.max(0, wait))
  if (wait <= 0) return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, wait)
    signal.addEventListener("abort", () => (clearTimeout(timer), reject(signal.reason)), { once: true })
  })
}

/** GET a URL as text. Throws LookupHttpError on a non-2xx status or an oversized body. */
export async function fetchText(url: string, opts: FetchOptions = {}): Promise<TextResponse> {
  const key = `${opts.headers?.Accept ?? ""} ${url}`
  if (opts.ttlMs) {
    const hit = cacheGet(key)
    if (hit) return hit
  }
  const { signal, clearTimeout } = abortAfterAny(
    opts.timeoutMs ?? LOOKUP_TIMEOUT_MS,
    ...(opts.signal ? [opts.signal] : []),
  )
  try {
    if (opts.minIntervalMs) await spacing(new URL(url).host, opts.minIntervalMs, signal)
    const response = await transport.fetch(url, {
      signal,
      headers: { "User-Agent": userAgent(), ...opts.headers },
    })
    const limit = opts.maxBytes ?? MAX_RESPONSE_BYTES
    if (Number(response.headers.get("content-length") ?? 0) > limit) {
      await response.body?.cancel().catch(() => {})
      throw new LookupHttpError(response.status, url, `Response from ${new URL(url).host} is too large.`)
    }
    const buffer = await response.arrayBuffer()
    if (buffer.byteLength > limit) throw new LookupHttpError(response.status, url, `Response from ${new URL(url).host} is too large.`)
    if (!response.ok) throw new LookupHttpError(response.status, url, `${new URL(url).host} answered ${response.status}.`)
    const value: TextResponse = {
      url: response.url || url,
      status: response.status,
      contentType: response.headers.get("content-type") ?? "",
      text: new TextDecoder().decode(buffer),
    }
    if (opts.ttlMs) cacheSet(key, value, opts.ttlMs)
    return value
  } finally {
    clearTimeout()
  }
}

/** GET a URL as JSON (sends Accept: application/json). */
export async function fetchJson<T>(url: string, opts: FetchOptions = {}): Promise<T> {
  const response = await fetchText(url, { ...opts, headers: { Accept: "application/json", ...opts.headers } })
  try {
    return JSON.parse(response.text) as T
  } catch {
    throw new LookupHttpError(response.status, url, `${new URL(url).host} did not return JSON.`)
  }
}
