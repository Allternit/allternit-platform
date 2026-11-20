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
import { platformRequest, platformSignedIn } from "@/runtime/bots/platform-api"
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

export function resetSleepClientForTests(): void {
  status = null
  inflight = null
}
