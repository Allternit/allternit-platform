/**
 * Allternit Sleep client: the per-process "is Sleep on for this person"
 * check and the signal call (POST /api/v1/memory/sleep/signal).
 *
 * The status check is one GET /memory/sleep/status per process, kept five
 * minutes. Signed out, an older runtime (404) or any failure means off.
 * Each fresh answer is also written into the account's cached settings
 * (`<account>/credentials.json` → settings.personal.sleep_enabled, next to
 * `dreaming_enabled`), so the TUI's sync autoDream gate (`sleepActiveSync`
 * in ../drive/paths) can keep the local consolidator off while Sleep runs.
 *
 * Used by the TUI turn capture and by the Sleep triggers (./triggers).
 */
import { platformFetch, platformRequest, platformSignedIn } from "@/runtime/bots/platform-api"
import { driveAccountSync } from "../drive/paths"
import { updateAccountStore } from "../drive/platform"

export const SLEEP_STATUS_TTL_MS = 5 * 60 * 1000

export type SleepSignalEvent = "idle" | "pre_compact" | "session_end" | "resume"

export interface SleepSignal {
  session_id: string
  event: SleepSignalEvent
  cache_ttl_s: number
}

let status: { enabled: boolean; at: number } | null = null
let inflight: Promise<boolean> | null = null

async function remember(enabled: boolean): Promise<void> {
  const account = driveAccountSync()
  if (!account.signedIn) return
  await updateAccountStore(account, (s) => {
    const prev = s.settings?.personal
    if (prev?.sleep_enabled === enabled) return
    s.settings = { ...s.settings, personal: { ...prev, sleep_enabled: enabled, sleep_at: new Date().toISOString() } }
  })
}

async function fetchStatus(): Promise<boolean> {
  let enabled = false
  try {
    if (await platformSignedIn()) {
      const r = await platformRequest<{ enabled?: boolean }>("GET", "/api/v1/memory/sleep/status", undefined, {
        timeoutMs: 5_000,
      })
      enabled = r?.enabled === true
    }
  } catch {
    enabled = false
  }
  status = { enabled, at: Date.now() }
  await remember(enabled).catch(() => {})
  return enabled
}

/** Whether the server's Allternit Sleep is on for this person (cached ~5 min per process). */
export async function sleepEnabled(): Promise<boolean> {
  if (status && Date.now() - status.at < SLEEP_STATUS_TTL_MS) return status.enabled
  inflight ??= fetchStatus().finally(() => {
    inflight = null
  })
  return inflight
}

/** The last known answer without a network call (undefined before the first check). */
export function sleepEnabledCached(): boolean | undefined {
  return status ? status.enabled : undefined
}

/** Send one signal. Throws on network/HTTP errors; callers fire and forget. */
export async function sendSleepSignal(signal: SleepSignal, timeoutMs = 5_000): Promise<void> {
  await platformRequest("POST", "/api/v1/memory/sleep/signal", signal, { timeoutMs })
}

// ── Sleep management calls (CLI + TUI) ─────────────────────────────────────

export type SleepOutcome<T = any> =
  | { kind: "ok"; body: T }
  | { kind: "refused"; message: string }
  | { kind: "no_sleep"; message: string }
  | { kind: "error"; status: number; message: string }

export const NO_SLEEP_MESSAGE = "This runtime has no Sleep yet. Update Allternit."

/** Map a response to an outcome: refusals and older runtimes get plain messages. */
export function interpretSleepResponse<T = any>(status: number, body: any): SleepOutcome<T> {
  if (body && body.accepted === false) {
    const message =
      body.reason === "sleep_disabled"
        ? "Sleep is off. Turn it on with `gizzi memory sleep on`."
        : body.reason === "already_settled"
          ? "Already settled."
          : `Not built yet: ${body.stage ?? body.reason ?? "this stage"}.`
    return { kind: "refused", message }
  }
  if (status >= 200 && status < 300) return { kind: "ok", body: body as T }
  // The runtime's own "Cycle not found" 404s carry an error body; a route that is missing has none.
  if (status === 404 && !(body && typeof body.error === "string")) return { kind: "no_sleep", message: NO_SLEEP_MESSAGE }
  return { kind: "error", status, message: (body && (body.error || body.message)) || `HTTP ${status}` }
}

/** One Sleep route call. Throws only for signed-out or connection failures. */
export async function sleepCall<T = any>(
  method: "GET" | "POST" | "PUT",
  path: string,
  body?: unknown,
  timeoutMs = 15_000,
): Promise<SleepOutcome<T>> {
  const response = await platformFetch(method, `/api/v1/memory/${path}`, body, { timeoutMs })
  const text = await response.text().catch(() => "")
  let parsed: any
  try {
    parsed = text ? JSON.parse(text) : undefined
  } catch {
    parsed = undefined
  }
  return interpretSleepResponse<T>(response.status, parsed)
}

export const sleepApi = {
  status: () => sleepCall("GET", "sleep/status"),
  cycles: () => sleepCall("GET", "sleep/cycles"),
  cycle: (id: string) => sleepCall("GET", `sleep/cycles/${encodeURIComponent(id)}`),
  undo: (id: string) => sleepCall("POST", `sleep/cycles/${encodeURIComponent(id)}/undo`, {}),
  run: (kind?: string) => sleepCall("POST", "sleep/run", kind ? { kind } : {}),
  setEnabled: (enabled: boolean) => sleepCall("PUT", "sleep/settings", { enabled }),
  disputes: () => sleepCall("GET", "disputes"),
  resolve: (id: string, choice: string, reason?: string) =>
    sleepCall("POST", `disputes/${encodeURIComponent(id)}/resolve`, reason ? { choice, reason } : { choice }),
  pages: () => sleepCall("GET", "sleep/pages"),
  page: (topic: string) => sleepCall("GET", `sleep/pages/${encodeURIComponent(topic)}`),
}

export function sleepStatusLine(s: any): string {
  if (!s || s.available === false) return "Sleep: not available on this runtime"
  const last = s.last_cycle
    ? `last ${s.last_cycle.kind} ${s.last_cycle.state} ${s.last_cycle.started_at}`
    : "no sleeps yet"
  const sleeping = s.last_cycle?.state === "running" ? ", sleeping now" : ""
  return `Sleep: ${s.enabled ? "on" : "off"}${sleeping} · debt ${Number(s.debt ?? 0).toFixed(2)} · ${last} · spend today $${Number(s.spend_today_usd ?? 0).toFixed(2)} of $${Number(s.daily_spend_cap_usd ?? 0).toFixed(2)}`
}

export function resetSleepClientForTests(): void {
  status = null
  inflight = null
}
