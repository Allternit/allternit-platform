/**
 * Memory Drive platform calls (docs/MEMORY_DRIVE_CONTRACT.md) and the drive
 * token store.
 *
 * Tokens: gizzi mints its own write-scoped (or read-scoped, for read-only
 * mounts) drive token with POST /memory/drive/tokens, labelled
 * "gizzi on <hostname>", and keeps it in a 0600 JSON file next to gizzi's
 * other credentials (the same pattern as auth.json / runtime-device.json),
 * keyed by account and drive. It is handed to git only through the
 * credential helper environment in git.ts — never argv, never a URL.
 */
import { mkdir, readFile, rename, writeFile, chmod } from "fs/promises"
import os from "os"
import path from "path"
import { platformRequest } from "@/runtime/bots/platform-api"
import { memoryDriveRoot, type DriveAccount } from "./paths"

const TIMEOUT_MS = 8_000

export interface DriveInfo {
  ref: string
  name: string
  brain_id: string
  revision?: string | null
  indexed_revision?: string | null
  index_dirty?: boolean
  imported_at?: string | null
  clone_url: string
  branch?: string
}

export interface DriveMount {
  ref: string
  kind: string
  name: string
  access: "read" | "write"
  revision?: string | null
}

export interface DriveSettings {
  dreaming_enabled: boolean
}

export interface DreamSummary {
  merged?: number
  resolved?: number
  lessons?: number
  pruned?: number
  proposals?: number
}

export interface Dream {
  id: string
  date: string
  status: "applied" | "no_changes" | "failed" | "undone" | "running"
  revision?: string | null
  base_revision?: string | null
  report?: string | null
  summary?: DreamSummary
  error?: string | null
  undo_revision?: string | null
  created_at?: string
}

export interface MintedToken {
  id: string
  token: string
  username: string
  clone_url: string
  access: "read" | "write"
}

const q = (ref: string) => (ref === "personal" ? "" : `?drive=${encodeURIComponent(ref)}`)
const qa = (ref: string) => (ref === "personal" ? "" : `&drive=${encodeURIComponent(ref)}`)

export namespace DrivePlatform {
  export function info(ref = "personal"): Promise<DriveInfo> {
    return platformRequest<DriveInfo>("GET", `/api/v1/memory/drive/info${q(ref)}`, undefined, { timeoutMs: TIMEOUT_MS })
  }

  export async function mounts(): Promise<DriveMount[]> {
    const r = await platformRequest<{ mounts: DriveMount[] }>("GET", "/api/v1/memory/drive/mounts", undefined, { timeoutMs: TIMEOUT_MS })
    return r.mounts ?? []
  }

  export function settings(ref = "personal"): Promise<DriveSettings> {
    return platformRequest<DriveSettings>("GET", `/api/v1/memory/drive/settings${q(ref)}`, undefined, { timeoutMs: TIMEOUT_MS })
  }

  export async function dreams(ref = "personal", limit = 20): Promise<Dream[]> {
    const r = await platformRequest<{ dreams: Dream[] }>(
      "GET",
      `/api/v1/memory/drive/dreams?limit=${Math.max(1, Math.min(limit, 100))}${qa(ref)}`,
      undefined,
      { timeoutMs: TIMEOUT_MS },
    )
    return r.dreams ?? []
  }

  export function undoDream(id: string, ref = "personal"): Promise<{ revision: string; undone: boolean }> {
    return platformRequest("POST", `/api/v1/memory/drive/dreams/${encodeURIComponent(id)}/undo${q(ref)}`, ref === "personal" ? {} : { drive: ref }, {
      timeoutMs: 30_000,
    })
  }

  export function mintToken(ref: string, access: "read" | "write"): Promise<MintedToken> {
    const host = os.hostname().replace(/[<>\n\r]/g, "").slice(0, 60) || "this computer"
    return platformRequest<MintedToken>(
      "POST",
      "/api/v1/memory/drive/tokens",
      { ...(ref === "personal" ? {} : { drive: ref }), access, label: `gizzi on ${host}` },
      { timeoutMs: TIMEOUT_MS },
    )
  }
}

// ── token + cache store ─────────────────────────────────────────────────────

interface StoredDrive {
  token?: string
  token_id?: string
  access?: "read" | "write"
  clone_url?: string
  branch?: string
  name?: string
  brain_id?: string
}

interface AccountStore {
  drives: Record<string, StoredDrive>
  mounts?: DriveMount[]
  mountsAt?: string
  settings?: Record<string, DriveSettings & { at: string }>
  /** Signed-out drive already merged into this account. */
  mergedLocal?: string
}

function storePath(account: DriveAccount): string {
  return path.join(memoryDriveRoot(), account.key, "credentials.json")
}

export async function readAccountStore(account: DriveAccount): Promise<AccountStore> {
  return readFile(storePath(account), "utf8")
    .then((t) => {
      const parsed = JSON.parse(t) as AccountStore
      return { ...parsed, drives: parsed.drives ?? {} }
    })
    .catch(() => ({ drives: {} }))
}

export async function writeAccountStore(account: DriveAccount, store: AccountStore): Promise<void> {
  const target = storePath(account)
  await mkdir(path.dirname(target), { recursive: true, mode: 0o700 })
  const temp = `${target}.${process.pid}.tmp`
  await writeFile(temp, JSON.stringify(store, null, 2), { mode: 0o600 })
  await chmod(temp, 0o600)
  await rename(temp, target)
}

export async function updateAccountStore(account: DriveAccount, fn: (store: AccountStore) => void): Promise<AccountStore> {
  const store = await readAccountStore(account)
  fn(store)
  await writeAccountStore(account, store)
  return store
}
