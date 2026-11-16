/**
 * The Files drive ships dark behind the per-user flag `files_drive` (PLAN D10).
 * cloud-api serves it as `flags.files_drive` (a boolean) on
 * `GET /api/v1/me/flags` (routes/me_flags.rs; added in platform #2441).
 *
 * Lookup order:
 * 1. `ALLTERNIT_FILES_DRIVE=1` forces it on (dev, and the runtime's own switch).
 * 2. A cached `/me/flags` answer in `~/.allternit/state/files-drive-flag.json`,
 *    keyed by the signed-in account, fresh for 10 minutes.
 * 3. A fetch with the same auth gizzi uses for cloud calls (ALLTERNIT_API_TOKEN,
 *    else the paired device token), 4 s timeout. Anything that fails is off.
 *
 * The TUI's command list is synchronous, so it reads only the switch and the
 * cache (`filesDriveEnabledSync`, which also accepts a cached answer up to a
 * day old); the TUI refreshes the cache in the background at start.
 *
 * Kept light (no imports of the client or the API helpers) so the TUI command
 * list can load it at startup.
 */
import crypto from "node:crypto"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"

export const FILES_DRIVE_OFF = "Files drive is not on for this account yet"

const FRESH_MS = 10 * 60_000
const SYNC_MAX_AGE_MS = 24 * 60 * 60_000
const FETCH_TIMEOUT_MS = 4_000

type CacheEntry = { files_drive: boolean; fetched_at: number }
type CacheFile = { current?: string; accounts: Record<string, CacheEntry> }

function cachePath(): string {
  return path.join(os.homedir(), ".allternit", "state", "files-drive-flag.json")
}

/** The developer switch. */
export function filesDriveForced(env: NodeJS.ProcessEnv = process.env): boolean {
  const v = env.ALLTERNIT_FILES_DRIVE?.trim().toLowerCase()
  return v === "1" || v === "true" || v === "on"
}

function readCache(): CacheFile {
  try {
    const c = JSON.parse(fs.readFileSync(cachePath(), "utf8")) as CacheFile
    return c && typeof c.accounts === "object" ? c : { accounts: {} }
  } catch {
    return { accounts: {} }
  }
}

function writeCache(key: string, filesDrive: boolean) {
  try {
    const c = readCache()
    c.accounts[key] = { files_drive: filesDrive, fetched_at: Date.now() }
    c.current = key
    fs.mkdirSync(path.dirname(cachePath()), { recursive: true })
    fs.writeFileSync(cachePath(), JSON.stringify(c), { mode: 0o600 })
  } catch {}
}

/** Cache key for an account: a hash of its token (the token itself is never stored). */
function accountKey(token: string): string {
  return crypto.createHash("sha256").update(token).digest("hex").slice(0, 24)
}

/** The token gizzi uses for cloud calls: an API key, else the paired device token. */
async function cloudToken(): Promise<string | undefined> {
  const key = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (key) return key
  try {
    const { Pairing } = await import("@/runtime/services/pairing/pairing")
    const d = await Pairing.load()
    return Pairing.tokenUsable(d) ? d.deviceToken : undefined
  } catch {
    return undefined
  }
}

/** Ask cloud-api. Returns undefined when it can't be read (no auth, offline, old server). */
async function fetchFlag(token: string): Promise<boolean | undefined> {
  const base = (process.env.ALLTERNIT_CLOUD_URL || "https://api.allternit.com").replace(/\/+$/, "")
  try {
    const res = await fetch(`${base}/api/v1/me/flags`, {
      headers: { Authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
    })
    if (!res.ok) return undefined
    const body = (await res.json()) as { flags?: { files_drive?: unknown } }
    // A server without the field (before #2441) means off.
    return body?.flags?.files_drive === true
  } catch {
    return undefined
  }
}

/**
 * Whether the Files drive is on for the signed-in account (CLI path).
 * Fresh cache first, then a fetch; off when nothing answers.
 */
export async function filesDriveEnabled(): Promise<boolean> {
  if (filesDriveForced()) return true
  const token = await cloudToken()
  if (!token) return false
  const key = accountKey(token)
  const hit = readCache().accounts[key]
  if (hit && Date.now() - hit.fetched_at < FRESH_MS) return hit.files_drive
  const fetched = await fetchFlag(token)
  if (fetched === undefined) return false
  writeCache(key, fetched)
  return fetched
}

/**
 * Synchronous check for the TUI command list: the switch, else the cached
 * answer for this account (ALLTERNIT_API_TOKEN's, else the account the last
 * fetch was for), up to a day old. Off when there is none.
 */
export function filesDriveEnabledSync(): boolean {
  if (filesDriveForced()) return true
  const c = readCache()
  const envToken = process.env.ALLTERNIT_API_TOKEN?.trim()
  const key = envToken ? accountKey(envToken) : c.current
  const hit = key ? c.accounts[key] : undefined
  return !!hit && Date.now() - hit.fetched_at < SYNC_MAX_AGE_MS && hit.files_drive
}

/** Refresh the cache in the background (TUI start). Never throws. */
export async function refreshFilesDriveFlag(): Promise<void> {
  if (filesDriveForced()) return
  const token = await cloudToken()
  if (!token) return
  const fetched = await fetchFlag(token)
  if (fetched !== undefined) writeCache(accountKey(token), fetched)
}
