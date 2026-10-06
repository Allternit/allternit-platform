/**
 * Where Memory Drive checkouts live. Synchronous and dependency-light so the
 * memdir path resolvers (src/memdir/paths.ts and the ink-app copy) can call it.
 *
 *   <data>/memory-drive/<account>/<drive-slug>/        the checkout
 *   <data>/memory-drive/<account>/<drive-slug>.rejected invalid hand edits moved aside
 *   <data>/memory-drive/<account>/state.json            cached mounts/settings
 *
 * <account> is `local` when signed out, `t-<sha256(token)[0:16]>` when
 * ALLTERNIT_API_TOKEN is set, else `u-<sha256(userId)[0:16]>` for the
 * `gizzi login` user, so
 * an account switch never shares a checkout.
 */
import { createHash } from "crypto"
import { readFileSync } from "fs"
import path from "path"
import { GlobalPaths } from "@/runtime/context/global/paths"

export const LOCAL_ACCOUNT = "local"

export interface DriveAccount {
  key: string
  signedIn: boolean
  userId?: string
  email?: string
}

/** Memory Drive on unless GIZZI_MEMORY_DRIVE=0. */
export function memoryDriveEnabled(): boolean {
  const v = process.env.GIZZI_MEMORY_DRIVE?.trim().toLowerCase()
  return !(v === "0" || v === "false" || v === "off")
}

export function memoryDriveRoot(): string {
  if (process.env.GIZZI_MEMORY_DRIVE_ROOT) return process.env.GIZZI_MEMORY_DRIVE_ROOT
  // Remote/CCR sessions keep memory on their persistent mount, like the memdir did.
  const remote = process.env.GIZZI_CODE_REMOTE_MEMORY_DIR ?? process.env.GIZZI_REMOTE_MEMORY_DIR
  if (remote) return path.join(remote, "memory-drive")
  return path.join(GlobalPaths.data, "memory-drive")
}

const hash = (s: string) => createHash("sha256").update(s).digest("hex").slice(0, 16)

let cached: { at: number; account: DriveAccount } | undefined

/** The account whose drive this process uses (5 s cache; reads the `gizzi login` device file). */
export function driveAccountSync(): DriveAccount {
  if (cached && Date.now() - cached.at < 5_000) return cached.account
  const account = resolveAccount()
  cached = { at: Date.now(), account }
  return account
}

export function resetDriveAccountCache(): void {
  cached = undefined
}

function resolveAccount(): DriveAccount {
  // Same credential order as platform-api (env token first, then `gizzi login`),
  // so the checkout always belongs to the identity the API calls use.
  const env = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (env) return { key: `t-${hash(env)}`, signedIn: true }
  try {
    const raw = readFileSync(path.join(GlobalPaths.data, "runtime-device.json"), "utf8")
    const stored = JSON.parse(raw) as { userId?: string; userEmail?: string; deviceToken?: string; tokenExpiresAt?: string }
    const usable = !!stored.deviceToken && !!stored.tokenExpiresAt && Date.parse(stored.tokenExpiresAt) > Date.now()
    if (usable && stored.userId) {
      return { key: `u-${hash(stored.userId)}`, signedIn: true, userId: stored.userId, email: stored.userEmail }
    }
  } catch {
    // not paired
  }
  return { key: LOCAL_ACCOUNT, signedIn: false }
}

/** `personal` → `personal`; `project:abc` → `project-abc` (filesystem-safe, unique). */
export function driveSlug(ref: string): string {
  if (ref === "personal") return "personal"
  const safe = ref.replace(/[^A-Za-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 60)
  return `${safe || "drive"}-${hash(ref).slice(0, 8)}`
}

export function accountDir(account: DriveAccount = driveAccountSync()): string {
  return path.join(memoryDriveRoot(), account.key)
}

/** Checkout path of a drive for an account (default: the current account's personal drive). */
export function driveCheckoutPath(ref = "personal", account: DriveAccount = driveAccountSync()): string {
  return path.join(accountDir(account), driveSlug(ref))
}

/** The signed-out drive (merged into the first account that signs in). */
export function localDriveCheckoutPath(): string {
  return driveCheckoutPath("personal", { key: LOCAL_ACCOUNT, signedIn: false })
}

/**
 * True when the server's nightly Dream owns the current account's personal
 * drive (GET /memory/drive/settings → dreaming_enabled, cached at session
 * start in <account>/credentials.json). Local autoDream stays off then so the
 * two never compete. Sync + light: the TUI's autoDream config reads it.
 */
export function serverDreamingActiveSync(): boolean {
  if (!memoryDriveEnabled()) return false
  const account = driveAccountSync()
  if (!account.signedIn) return false
  try {
    const raw = readFileSync(path.join(accountDir(account), "credentials.json"), "utf8")
    const store = JSON.parse(raw) as { settings?: Record<string, { dreaming_enabled?: boolean }> }
    return store.settings?.personal?.dreaming_enabled === true
  } catch {
    return false
  }
}
