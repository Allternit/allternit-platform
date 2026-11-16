/**
 * Files drive client for Gizzi (`files/2`, contracts/files-drive/SPEC.md).
 * `gizzi files …` (cli/commands/files.ts) and the TUI `/files` both use it.
 *
 * Transports (SPEC section 10), picked per computer:
 *
 * - **loopback**: this machine's own runtime through Desktop's gateway
 *   (`ALLTERNIT_API_URL`, default 127.0.0.1:8013), with this machine's device
 *   token, the same way the other work views reach it (cli/work/api.ts).
 * - **mesh**: another of the user's own devices, when cloud-api lists a mesh
 *   candidate and it answers `ping` within 1.5 s.
 * - **relay**: cloud-api's proxy `POST /api/v1/runtime-devices/:id/proxy`, with
 *   an Allternit API key with the compute scope (ALLTERNIT_API_TOKEN), as
 *   `gizzi run --on` does. Cloud computers are relay only.
 *
 * Wake (SPEC section 9): nothing here wakes a computer by itself. Browsing a
 * sleeping computer is answered from its snapshot. Content on a sleeping one
 * fails with `wake_consent_required`; the caller asks the user (or the user
 * passed `--wake`), then `wakeConsent()` gets a ticket that rides on every
 * later request until it expires.
 *
 * Types mirror contracts/files-drive/schema/*.json (field names are the wire
 * names). Only the fields this client reads are typed.
 */
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { ALLTERNIT_GATEWAY_BASE } from "@/shared/constants/allternitGateway"
import { cloudBase, listRuntimes, pick, type Runtime } from "@/cli/work/api"

export const FILES_V2 = "/api/v1/files/v2"
/** Small write limit (SPEC section 2); bigger files go through an upload session. */
export const SMALL_WRITE_MAX = 4 * 1024 * 1024
/** Read piece: the largest chunk, so one relay request each. */
export const READ_PIECE = 4 * 1024 * 1024

// ── the mount helper ──────────────────────────────────────────────────────

export const MOUNT_BIN = process.platform === "win32" ? "allternit-files-mount.exe" : "allternit-files-mount"

function isExecutable(p: string): boolean {
  try {
    const st = fs.statSync(p)
    if (!st.isFile()) return false
    if (process.platform !== "win32") fs.accessSync(p, fs.constants.X_OK)
    return true
  } catch {
    return false
  }
}

/**
 * `allternit-files-mount` (platform/files/allternit-files-mount), used by
 * `gizzi files mount` and uploads over 4 MiB. Where installs put it:
 *
 * 1. ALLTERNIT_FILES_MOUNT_BIN (explicit; nothing else is tried when it is set but wrong).
 * 2. Next to the gizzi binary: the npm platform package, the release tarball,
 *    brew's libexec, and Desktop's resources/bin (Desktop runs gizzi-code from there).
 * 3. Allternit Desktop on Linux (.deb: /opt/Allternit Desktop/resources/bin;
 *    AppImage: $APPDIR/resources/bin while it runs).
 * 4. ~/.allternit/bin, then PATH.
 */
export function mountBinary(env: NodeJS.ProcessEnv = process.env, execPath: string = process.execPath): string | undefined {
  const explicit = env.ALLTERNIT_FILES_MOUNT_BIN?.trim()
  if (explicit) return isExecutable(explicit) ? explicit : undefined
  let execDir = path.dirname(execPath)
  try {
    execDir = path.dirname(fs.realpathSync(execPath)) // brew's bin/ symlink -> libexec/
  } catch {}
  const candidates = [
    path.join(execDir, MOUNT_BIN),
    ...(process.platform === "linux"
      ? [
          path.join("/opt", "Allternit Desktop", "resources", "bin", MOUNT_BIN),
          ...(env.APPDIR ? [path.join(env.APPDIR, "resources", "bin", MOUNT_BIN)] : []),
        ]
      : []),
    ...(process.platform === "darwin" ? [path.join("/Applications", "Allternit Desktop.app", "Contents", "Resources", "bin", MOUNT_BIN)] : []),
    path.join(os.homedir(), ".allternit", "bin", MOUNT_BIN),
    ...(env.PATH ?? "").split(path.delimiter).filter(Boolean).map((d) => path.join(d, MOUNT_BIN)),
  ]
  return candidates.find(isExecutable)
}

// ── contract types (subset) ───────────────────────────────────────────────

export type ComputerState = "awake" | "asleep" | "waking" | "offline"
export type Freshness = { served_by: "live" | "snapshot" | "replica" | "cache"; computer_state: ComputerState; snapshot_at?: string }
export type Permissions = Record<"list" | "read" | "write" | "delete" | "share" | "comment" | "agent_process" | "transfer" | "execute", boolean>
export type Item = {
  id: string
  root_id: string
  parent_id?: string | null
  name: string
  kind: "file" | "folder"
  size: number
  rev: string
  meta_version: number
  content_hash?: string
  mime?: string
  created_at: string
  modified_at: string
  root_link?: string
  pinned_on?: string[]
  tags?: string[]
  permissions: Permissions
}
export type RootPolicy = { read: boolean; write: boolean; shareable: boolean; protected: boolean; trash_days: number; [k: string]: unknown }
export type Root = {
  id: string
  kind: string
  name: string
  path_hint?: string
  policy: RootPolicy
  root_item_id: string
  parent_root_id?: string
  cursor: string
  usage_bytes?: number
  provider_ready: boolean
}
export type OnConflict = "fail" | "keep_both" | "replace"
export type ConflictInfo = { conflict_name: string; conflict_item: Item }
export type MutationResponse = { item: Item; conflict?: ConflictInfo; cursor: string }
export type ListResponse = { root_id: string; parent: Item; items: Item[]; next_page_token?: string; cursor: string; freshness: Freshness }
export type TrashEntry = { trash_id: string; item: Item; original_parent_id: string; trashed_at: string; purge_at: string }
export type Version = { version_id: string; rev: string; size: number; created_at: string; created_by: { kind: string; id: string; display_name?: string }; label?: string }
export type ShareRole = "view_download" | "upload_only" | "edit" | "comment"
export type Share = {
  share_id: string
  root_id: string
  item_id: string
  role: ShareRole
  link_url: string
  expires_at?: string
  passcode_required: boolean
  guest_wake: boolean
  revoked_at?: string
  guest_count: number
}
export type WakeSetting = "ask" | "auto" | "never"
export type WakeReason = "open_file" | "upload" | "search_content" | "sync" | "replica_restore" | "share_access"
export type TransportCandidate = { kind: "loopback" | "mesh" | "relay" | "gateway"; base_url: string; priority: number; max_request_bytes: number }
export type EndpointsResponse = {
  computer_id: string
  computer_state: ComputerState
  wake: WakeSetting
  candidates: TransportCandidate[]
  host: "own_device" | "cloud_computer"
}

/** A failed Files call: the contract's `ErrorBody`, kept whole for `--json`. */
export class FilesError extends Error {
  constructor(
    readonly code: string,
    readonly status: number,
    message: string,
    readonly body: Record<string, any> = {},
  ) {
    super(message)
  }
  get retryAfterS(): number | undefined {
    return typeof this.body.retry_after_s === "number" ? this.body.retry_after_s : undefined
  }
  toJSON() {
    return { error: this.code, code: this.code, status: this.status, message: this.message, ...this.body }
  }
}

function errorFrom(status: number, text: string): FilesError {
  let body: Record<string, any> = {}
  try {
    body = JSON.parse(text)
  } catch {}
  const raw = body?.error
  const code: string =
    body?.code ??
    (typeof raw === "string" ? raw : raw?.code) ??
    ({ 401: "unauthorized", 403: "forbidden", 404: "not_found", 413: "payload_too_large", 429: "rate_limited", 503: "offline" } as Record<number, string>)[status] ??
    "internal"
  const message: string = body?.message ?? (typeof raw === "object" ? raw?.message : undefined) ?? `the computer answered ${status}`
  return new FilesError(code, status, message, typeof body === "object" && body ? body : {})
}

// ── this device ───────────────────────────────────────────────────────────

export type DeviceInfo = { id: string; name: string; token?: string; userId: string }

let devicePromise: Promise<DeviceInfo> | undefined
/** This machine's runtime device id and token (from `gizzi pair`), for pins and `X-Allternit-Device-Id`. */
export function thisDevice(): Promise<DeviceInfo> {
  devicePromise ??= (async () => {
    const { Pairing } = await import("@/runtime/services/pairing/pairing")
    const d = await Pairing.load().catch(() => undefined)
    return {
      id: process.env.ALLTERNIT_DEVICE_ID?.trim() || d?.runtimeId || `dev_gizzi_${os.hostname()}`,
      name: process.env.ALLTERNIT_DEVICE_NAME?.trim() || d?.name || os.hostname(),
      token: Pairing.tokenUsable(d) ? d!.deviceToken : undefined,
      userId: process.env.ALLTERNIT_USER_ID?.trim() || d?.userId || "gizzi-local",
    }
  })()
  return devicePromise
}

function apiKey(): string | undefined {
  return process.env.ALLTERNIT_API_TOKEN?.trim() || undefined
}

/** Auth for cloud-api calls (endpoints, wake consent, shares): an API key, else this device's token. */
async function cloudAuth(): Promise<string | undefined> {
  return apiKey() ?? (await thisDevice()).token
}

async function cloudJson<T>(method: string, path: string, body?: unknown): Promise<T> {
  const token = await cloudAuth()
  let res: Response
  try {
    res = await fetch(`${cloudBase()}${path}`, {
      method,
      headers: {
        ...(body === undefined ? {} : { "content-type": "application/json" }),
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(20_000),
    })
  } catch (e) {
    throw new FilesError("offline", 0, `Couldn't reach Allternit Cloud: ${(e as Error).message}`)
  }
  const text = await res.text()
  if (res.status === 401) throw new FilesError("unauthorized", 401, "Not signed in. Run `gizzi login`, or set ALLTERNIT_API_TOKEN.")
  if (!res.ok) throw errorFrom(res.status, text)
  return (text ? JSON.parse(text) : {}) as T
}

// ── connection to one computer ────────────────────────────────────────────

export type Transport =
  | { kind: "loopback" | "mesh"; baseUrl: string; token?: string; userId?: string }
  | { kind: "relay"; proxyUrl: string; token: string }

export type FileReply = { status: number; headers: Headers; bytes: Uint8Array; text(): string; json<T = any>(): T }

const RELAY_KEY_NEEDED =
  "This computer is reached through the Allternit relay, which needs an Allternit API key with the compute scope. Set ALLTERNIT_API_TOKEN (Settings → API keys) and try again."

/** One computer's Files drive. */
export class Drive {
  private ticket?: { value: string; expiresAt: number }
  private rootsCache?: Root[]

  constructor(
    /** cloud-api's runtime device id (SPEC "Computer ids"), or the local runtime's id. */
    readonly computerId: string,
    readonly name: string,
    readonly transport: Transport,
    readonly device: DeviceInfo,
    /** The relay can wake it (a cloud computer). */
    readonly wakeable: boolean,
  ) {}

  /** The current wake ticket, if one is held and not about to expire. */
  wakeTicket(): { value: string; expiresAt: string } | undefined {
    if (!this.ticket || this.ticket.expiresAt - 5_000 < Date.now()) return undefined
    return { value: this.ticket.value, expiresAt: new Date(this.ticket.expiresAt).toISOString() }
  }

  /** The consent call (SPEC section 9). Only call it after the user agreed (prompt, `--wake`, or setting `auto`). */
  async wakeConsent(reason: WakeReason, itemId?: string): Promise<{ computer_state: ComputerState; estimated_wake_s?: number; expires_at: string }> {
    const r = await cloudJson<{ ticket: string; expires_at: string; computer_state: ComputerState; estimated_wake_s?: number }>(
      "POST",
      `/api/v1/computer-files/${encodeURIComponent(this.computerId)}/wake-consent`,
      { reason, device_id: this.device.id, ...(itemId ? { item_id: itemId } : {}) },
    )
    const at = Date.parse(r.expires_at)
    this.ticket = { value: r.ticket, expiresAt: Number.isFinite(at) ? at : Date.now() + 60_000 }
    return r
  }

  getWakeSetting(): Promise<{ computer_id: string; wake: WakeSetting }> {
    return cloudJson("GET", `/api/v1/computer-files/${encodeURIComponent(this.computerId)}/wake-settings`)
  }

  setWakeSetting(wake: WakeSetting): Promise<{ computer_id: string; wake: WakeSetting }> {
    return cloudJson("PUT", `/api/v1/computer-files/${encodeURIComponent(this.computerId)}/wake-settings`, { wake })
  }

  /** One request to `/api/v1/files/v2<path>`. Retries `waking` (with its `retry_after_s`) a few times. */
  async request(
    method: string,
    path: string,
    opts: { json?: unknown; bytes?: Uint8Array; headers?: Record<string, string>; timeoutMs?: number } = {},
  ): Promise<FileReply> {
    for (let attempt = 0; ; attempt++) {
      const reply = await this.send(method, path, opts)
      if ((reply.status >= 200 && reply.status < 300) || reply.status === 304) return reply
      const err = errorFrom(reply.status, reply.text())
      if (err.code === "waking" && attempt < 6) {
        await Bun.sleep(Math.min(Math.max(err.retryAfterS ?? 3, 1), 10) * 1000)
        continue
      }
      throw err
    }
  }

  private async send(
    method: string,
    path: string,
    opts: { json?: unknown; bytes?: Uint8Array; headers?: Record<string, string>; timeoutMs?: number },
  ): Promise<FileReply> {
    const headers: Record<string, string> = {
      "x-allternit-device-id": this.device.id,
      "x-allternit-device-name": this.device.name,
      ...(opts.headers ?? {}),
    }
    const ticket = this.wakeTicket()
    if (ticket) headers["x-allternit-wake-ticket"] = ticket.value
    if (opts.json !== undefined) headers["content-type"] ??= "application/json"
    if (opts.bytes) headers["content-type"] ??= "application/octet-stream"
    const signal = AbortSignal.timeout(opts.timeoutMs ?? 120_000)
    let res: Response
    try {
      if (this.transport.kind === "relay") {
        const body =
          opts.bytes !== undefined
            ? { body: Buffer.from(opts.bytes).toString("base64"), encoding: "base64" }
            : { body: opts.json === undefined ? "" : JSON.stringify(opts.json), encoding: "utf8" }
        res = await fetch(this.transport.proxyUrl, {
          method: "POST",
          headers: { "content-type": "application/json", Authorization: `Bearer ${this.transport.token}` },
          // cloud-api reads `bodyEncoding`; the SPEC names it `body_encoding`. Send both (SPEC section 2).
          body: JSON.stringify({ method, path: `${FILES_V2}${path}`, headers, body: body.body, body_encoding: body.encoding, bodyEncoding: body.encoding }),
          signal,
        })
      } else {
        if (this.transport.token) headers.Authorization = `Bearer ${this.transport.token}`
        if (this.transport.userId) headers["x-allternit-user-id"] = this.transport.userId
        res = await fetch(`${this.transport.baseUrl}${path}`, {
          method,
          headers,
          body: opts.bytes ?? (opts.json === undefined ? undefined : JSON.stringify(opts.json)),
          signal,
        })
      }
    } catch (e) {
      const err = e as Error
      const timedOut = err?.name === "TimeoutError" || err?.name === "AbortError"
      throw new FilesError("offline", 0, `${this.name}: ${timedOut ? "no answer in time" : `couldn't reach it (${err?.message ?? e})`}`)
    }
    return unwrapStubEnvelope(res.status, res.headers, new Uint8Array(await res.arrayBuffer()))
  }

  async json<T>(method: string, path: string, json?: unknown): Promise<T> {
    return (await this.request(method, path, json === undefined ? {} : { json })).json<T>()
  }

  // ── routes (SPEC section 2) ──

  async roots(): Promise<Root[]> {
    this.rootsCache ??= (await this.json<{ roots: Root[] }>("GET", "/roots")).roots
    return this.rootsCache
  }

  async rootsWithFreshness(): Promise<{ roots: Root[]; freshness?: Freshness }> {
    const r = await this.json<{ roots: Root[]; freshness?: Freshness }>("GET", "/roots")
    this.rootsCache = r.roots
    return r
  }

  /** Every item in a folder (all pages) and the freshness of the first page. */
  async list(rootId: string, parentId: string): Promise<{ items: Item[]; freshness?: Freshness }> {
    const items: Item[] = []
    let token: string | undefined
    let freshness: Freshness | undefined
    do {
      const q = new URLSearchParams({ parent_id: parentId, limit: "500" })
      if (token) q.set("page_token", token)
      const page = await this.json<ListResponse>("GET", `/roots/${enc(rootId)}/list?${q}`)
      items.push(...page.items)
      freshness ??= page.freshness
      token = page.next_page_token || undefined
    } while (token)
    return { items, freshness }
  }

  stat(itemId: string): Promise<{ item: Item; freshness: Freshness }> {
    return this.json("GET", `/items/${enc(itemId)}`)
  }

  /** `length` bytes at `offset` (one ranged GET). */
  async readRange(item: Item, offset: number, length: number): Promise<Uint8Array> {
    if (length <= 0 || offset >= item.size) return new Uint8Array()
    const len = Math.min(length, item.size - offset)
    const r = await this.request("GET", `/items/${enc(item.id)}/content`, { headers: { range: `bytes=${offset}-${offset + len - 1}` } })
    if (r.status === 206) return r.bytes
    return r.bytes.subarray(Math.min(offset, r.bytes.length), Math.min(offset + len, r.bytes.length))
  }

  /** Small write (at most 4 MiB). `replace` = the item and the rev the caller last saw. */
  smallWrite(rootId: string, parentId: string, name: string, bytes: Uint8Array, onConflict: OnConflict, replace?: Item): Promise<MutationResponse> {
    const q = new URLSearchParams({
      parent_id: parentId,
      name,
      on_conflict: onConflict,
      device_name: this.device.name,
      utc_offset_minutes: String(utcOffsetMinutes()),
    })
    if (replace) q.set("base_rev", replace.rev)
    return this.request("PUT", `/roots/${enc(rootId)}/content?${q}`, { bytes }).then((r) => r.json<MutationResponse>())
  }

  mkdir(parentId: string, name: string, onConflict: OnConflict = "fail"): Promise<MutationResponse> {
    return this.json("POST", "/mkdir", { parent_id: parentId, name, on_conflict: onConflict })
  }

  move(item: Item, to: { parentId?: string; name?: string }, onConflict: OnConflict = "fail"): Promise<MutationResponse> {
    return this.json("POST", "/move", {
      item_id: item.id,
      base_rev: item.rev,
      on_conflict: onConflict,
      ...(to.parentId ? { new_parent_id: to.parentId } : {}),
      ...(to.name ? { new_name: to.name } : {}),
    })
  }

  copy(item: Item, destParentId: string, newName?: string, onConflict: OnConflict = "keep_both"): Promise<{ item?: Item; job?: { job_id: string; state: string } }> {
    return this.json("POST", "/copy", { item_id: item.id, dest_parent_id: destParentId, on_conflict: onConflict, ...(newName ? { new_name: newName } : {}) })
  }

  delete(item: Item, permanent = false): Promise<{ trashed?: TrashEntry; cursor: string }> {
    return this.json("POST", "/delete", { item_id: item.id, base_rev: item.rev, permanent })
  }

  async trash(rootId: string): Promise<TrashEntry[]> {
    const out: TrashEntry[] = []
    let token: string | undefined
    do {
      const page = await this.json<{ entries: TrashEntry[]; next_page_token?: string }>(
        "GET",
        `/roots/${enc(rootId)}/trash${token ? `?page_token=${enc(token)}` : ""}`,
      )
      out.push(...page.entries)
      token = page.next_page_token || undefined
    } while (token)
    return out
  }

  restore(trashId: string, onConflict: OnConflict = "keep_both"): Promise<MutationResponse> {
    return this.json("POST", "/restore", { trash_id: trashId, on_conflict: onConflict })
  }

  async versions(item: Item): Promise<Version[]> {
    return (await this.json<{ versions: Version[] }>("GET", `/items/${enc(item.id)}/versions`)).versions
  }

  restoreVersion(item: Item, versionId: string): Promise<MutationResponse> {
    return this.json("POST", `/items/${enc(item.id)}/versions/restore`, { version_id: versionId, base_rev: item.rev })
  }

  pin(item: Item, pinned: boolean): Promise<{ item_id: string; pinned_on: string[] }> {
    return this.json("POST", "/pin", { item_id: item.id, device_id: this.device.id, pinned })
  }

  search(query: string, mode: "name" | "full_text" | "semantic", rootIds?: string[], limit = 50): Promise<{ hits: { item: Item; score: number; snippet?: string }[]; index_state: string; freshness: Freshness }> {
    return this.json("POST", "/search", { query, mode, limit, ...(rootIds?.length ? { root_ids: rootIds } : {}) })
  }

  // ── cloud-api: shares (SPEC section 2, cloud-api table) ──

  createShare(req: { root_id: string; item_id: string; role: ShareRole; expires_at?: string; passcode?: string; guest_wake: boolean; invite_emails?: string[] }): Promise<{ share: Share }> {
    return cloudJson("POST", `/api/v1/computer-files/${encodeURIComponent(this.computerId)}/shares`, req)
  }

  listShares(): Promise<{ shares: Share[] }> {
    return cloudJson("GET", `/api/v1/computer-files/${encodeURIComponent(this.computerId)}/shares`)
  }
}

/** The Files stub (tools/files-stub) wraps relayed answers in `{status, headers, body, body_encoding}`; production streams them raw. */
function unwrapStubEnvelope(status: number, headers: Headers, bytes: Uint8Array): FileReply {
  const make = (s: number, b: Uint8Array): FileReply => ({
    status: s,
    headers,
    bytes: b,
    text: () => new TextDecoder().decode(b),
    json: <T>() => (b.length ? JSON.parse(new TextDecoder().decode(b)) : {}) as T,
  })
  if (status === 200 && bytes[0] === 0x7b /* { */ && bytes.length < 16 * 1024 * 1024) {
    try {
      const env = JSON.parse(new TextDecoder().decode(bytes))
      const keys = Object.keys(env).sort().join(",")
      if (keys === "body,body_encoding,headers,status" || keys === "body,body_encoding,status") {
        const body = env.body_encoding === "base64" ? new Uint8Array(Buffer.from(env.body, "base64")) : new TextEncoder().encode(env.body)
        return make(env.status, body)
      }
    } catch {}
  }
  return make(status, bytes)
}

export function enc(s: string): string {
  return encodeURIComponent(s)
}

/** This machine's UTC offset in minutes, so conflict copies carry the local time (SPEC section 4). */
export function utcOffsetMinutes(): number {
  return Math.max(-840, Math.min(840, -new Date().getTimezoneOffset()))
}

// ── picking a computer ────────────────────────────────────────────────────

const LOCAL_NAMES = new Set(["this", "local", "here", "this computer"])

async function pingLoopback(device: DeviceInfo): Promise<{ computer_id: string } | undefined> {
  try {
    const res = await fetch(`${ALLTERNIT_GATEWAY_BASE}${FILES_V2}/ping`, {
      headers: {
        ...(device.token ? { Authorization: `Bearer ${device.token}` } : {}),
        "x-allternit-user-id": device.userId,
      },
      signal: AbortSignal.timeout(1_500),
    })
    if (!res.ok) return undefined
    const body = (await res.json()) as { computer_id?: string; protocol?: string }
    return body.protocol === "files/2" && body.computer_id ? { computer_id: body.computer_id } : undefined
  } catch {
    return undefined
  }
}

async function pingMesh(baseUrl: string, token?: string): Promise<boolean> {
  try {
    const res = await fetch(`${baseUrl.replace(/\/+$/, "")}/ping`, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
      signal: AbortSignal.timeout(1_500),
    })
    return res.ok
  } catch {
    return false
  }
}

/**
 * The computer `key` names (runtime id, then name, from `gizzi computers ls`),
 * or this machine when `key` is empty, "this" or "local" and its runtime
 * answers. With no key and no local runtime, the only paired computer.
 */
export async function openDrive(key?: string): Promise<Drive> {
  const device = await thisDevice()
  const wanted = key?.trim() || process.env.ALLTERNIT_FILES_COMPUTER?.trim() || ""
  const local = !wanted || LOCAL_NAMES.has(wanted.toLowerCase()) ? await pingLoopback(device) : undefined
  const loopback = (id: string, name: string) =>
    new Drive(id, name, { kind: "loopback", baseUrl: `${ALLTERNIT_GATEWAY_BASE}${FILES_V2}`, token: device.token, userId: device.userId }, device, false)
  if (local) return loopback(local.computer_id, "This computer")
  if (wanted && LOCAL_NAMES.has(wanted.toLowerCase()))
    throw new FilesError("offline", 0, `This computer's Allternit runtime is not answering at ${ALLTERNIT_GATEWAY_BASE}. Open Allternit Desktop (or set ALLTERNIT_API_URL).`)

  let runtimes: Runtime[]
  try {
    runtimes = await listRuntimes()
  } catch (e) {
    throw new FilesError("offline", 0, `Couldn't list your computers: ${(e as Error).message}`)
  }
  let runtime: Runtime | undefined
  if (wanted) runtime = pick(runtimes, wanted)
  else if (runtimes.length === 1) runtime = runtimes[0]
  if (!runtime) {
    throw new FilesError(
      "not_found",
      404,
      wanted
        ? `No computer named or with id "${wanted}". Run \`gizzi computers ls\`.`
        : "Name a computer: `gizzi files ls \"My computer\":` or --computer. Run `gizzi computers ls` to see them.",
    )
  }
  // This machine, named explicitly: loopback when its runtime answers.
  if (runtime.id === device.id) {
    const self = await pingLoopback(device)
    if (self) return loopback(self.computer_id, runtime.name)
  }

  const wakeable = runtime.wakeable === true
  // cloud-api's endpoints list (SPEC section 10). Older servers have none; fall back to the relay.
  const endpoints = await cloudJson<EndpointsResponse>("GET", `/api/v1/computer-files/${encodeURIComponent(runtime.id)}/endpoints`).catch(() => undefined)
  const computerId = endpoints?.computer_id ?? runtime.id
  const candidates = [...(endpoints?.candidates ?? [])].sort((a, b) => a.priority - b.priority)
  const token = apiKey()
  for (const c of candidates) {
    if (c.kind === "mesh" && endpoints?.host === "own_device" && (await pingMesh(c.base_url, token ?? device.token))) {
      return new Drive(computerId, runtime.name, { kind: "mesh", baseUrl: c.base_url.replace(/\/+$/, ""), token: token ?? device.token }, device, false)
    }
  }
  if (!token) throw new FilesError("unauthorized", 401, RELAY_KEY_NEEDED)
  const relay = candidates.find((c) => c.kind === "relay")
  const proxyUrl = relay?.base_url || `${cloudBase()}/api/v1/runtime-devices/${encodeURIComponent(computerId)}/proxy`
  return new Drive(computerId, runtime.name, { kind: "relay", proxyUrl, token }, device, wakeable || endpoints?.host === "cloud_computer")
}

// ── paths ─────────────────────────────────────────────────────────────────

/**
 * A remote path: `[computer:]Root/sub/file`. The computer part is optional
 * (`--computer`, or this machine). `Inbox/x.pdf` and `Allternit/Inbox/x.pdf`
 * both work: the first segment names any root, and a folder that links to
 * another root (`root_link`) is followed.
 */
export type RemotePath = { computer?: string; segments: string[]; trailingSlash: boolean }

/** True when `arg` is written as remote: `computer:path` or `:path` (`cp` uses this to pick a direction). */
export function looksRemote(arg: string): boolean {
  return splitComputer(arg).computer !== undefined || arg.startsWith(":")
}

function splitComputer(arg: string): { computer?: string; rest: string } {
  const i = arg.indexOf(":")
  // `C:\…` (one letter) and paths whose first segment has a slash before the colon are local / plain.
  if (i <= 1 || arg.slice(0, i).includes("/")) return { rest: i === 0 ? arg.slice(1) : arg }
  return { computer: arg.slice(0, i), rest: arg.slice(i + 1) }
}

export function parseRemote(arg: string): RemotePath {
  const { computer, rest } = splitComputer(arg)
  return { computer, segments: rest.split("/").filter((s) => s && s !== "."), trailingSlash: rest.endsWith("/") }
}

/** Where a path landed: the root, the item (none for a root itself), and the folder to list for it. */
export type Resolved = { root: Root; item?: Item; folder: { rootId: string; itemId: string }; isFolder: boolean; name: string }

/** Walk `segments` from the roots. Throws `not_found` for a missing segment. */
export async function resolvePath(drive: Drive, segments: string[]): Promise<Resolved> {
  if (!segments.length) throw new FilesError("invalid_request", 400, "Give a folder or file, e.g. Inbox/notes.md. `gizzi files ls` shows the roots.")
  const roots = await drive.roots()
  const first = segments[0].toLowerCase()
  const root = roots.find((r) => !r.parent_root_id && r.name.toLowerCase() === first) ?? roots.find((r) => r.name.toLowerCase() === first)
  if (!root) {
    throw new FilesError("root_not_found", 404, `${drive.name} has no folder "${segments[0]}". Its folders: ${roots.filter((r) => !r.parent_root_id).map((r) => r.name).join(", ")}`)
  }
  let cur: Resolved = { root, folder: { rootId: root.id, itemId: root.root_item_id }, isFolder: true, name: root.name }
  for (const seg of segments.slice(1)) {
    if (!cur.isFolder) throw new FilesError("not_found", 404, `${cur.name} is a file, not a folder.`)
    const { items } = await drive.list(cur.folder.rootId, cur.folder.itemId)
    const hit = items.find((i) => i.name === seg) ?? items.find((i) => i.name.toLowerCase() === seg.toLowerCase())
    if (!hit) throw new FilesError("not_found", 404, `No "${seg}" in ${cur.name}.`)
    const linked = hit.root_link ? roots.find((r) => r.id === hit.root_link) : undefined
    if (linked) cur = { root: linked, item: hit, folder: { rootId: linked.id, itemId: linked.root_item_id }, isFolder: true, name: hit.name }
    else
      cur = {
        root: roots.find((r) => r.id === hit.root_id) ?? cur.root,
        item: hit,
        folder: hit.kind === "folder" ? { rootId: hit.root_id, itemId: hit.id } : cur.folder,
        isFolder: hit.kind === "folder",
        name: hit.name,
      }
  }
  return cur
}

/** The parent folder of a path that may not exist yet, plus its last segment. */
export async function resolveParent(drive: Drive, segments: string[]): Promise<{ parent: Resolved; name: string }> {
  if (segments.length < 2) throw new FilesError("invalid_request", 400, "Give a folder inside one of the computer's folders, e.g. Inbox/new-name.")
  const parent = await resolvePath(drive, segments.slice(0, -1))
  if (!parent.isFolder) throw new FilesError("not_found", 404, `${parent.name} is a file, not a folder.`)
  return { parent, name: segments[segments.length - 1] }
}

// ── formatting ────────────────────────────────────────────────────────────

export function formatBytes(n: number): string {
  if (!Number.isFinite(n)) return ""
  const units = ["B", "KB", "MB", "GB", "TB"]
  let v = n
  let u = 0
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000
    u++
  }
  return `${u === 0 ? v : v.toFixed(v < 10 ? 1 : 0)} ${units[u]}`
}

export function formatTime(iso: string | undefined): string {
  if (!iso) return ""
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ""
  const today = new Date()
  const sameDay = d.toDateString() === today.toDateString()
  const time = d.toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" })
  return sameDay ? `Today ${time}` : `${d.toLocaleDateString("en-US", { month: "short", day: "numeric" })} ${time}`
}

/** One line about where an answer came from, or undefined for a live answer from an awake computer. */
export function freshnessLine(f: Freshness | undefined): string | undefined {
  if (!f) return undefined
  if (f.served_by === "snapshot") return `asleep · saved list from ${formatTime(f.snapshot_at) || "earlier"} · browsing is free`
  if (f.served_by === "replica") return "asleep · served from a replica on another of your devices"
  if (f.served_by === "cache") return "offline · from this device's cache"
  if (f.computer_state === "waking") return "waking"
  return undefined
}

export function isConflictCopy(name: string): boolean {
  return /\(conflict .+ \d{4}-\d{2}-\d{2} \d{4}( \d+)?\)/.test(name)
}

export function formatItems(items: Item[], deviceId?: string): string {
  if (!items.length) return "  (empty)"
  const width = Math.min(48, Math.max(...items.map((i) => i.name.length + (i.kind === "folder" ? 1 : 0))))
  return items
    .slice()
    .sort((a, b) => Number(b.kind === "folder") - Number(a.kind === "folder") || a.name.localeCompare(b.name))
    .map((i) => {
      const name = i.kind === "folder" ? `${i.name}/` : i.name
      const bits = [i.kind === "folder" ? "" : formatBytes(i.size), formatTime(i.modified_at)]
      if (deviceId && i.pinned_on?.includes(deviceId)) bits.push("pinned")
      if (isConflictCopy(i.name)) bits.push("conflict")
      return `${name.padEnd(width)}  ${bits.filter(Boolean).join("  ")}`
    })
    .join("\n")
}

/** "2026-11-22T…" for "7d", "12h", "30m" or an ISO date. */
export function parseExpiry(s: string | undefined, now = Date.now()): string | undefined {
  if (!s) return undefined
  const m = /^(\d+)\s*([mhdw])$/i.exec(s.trim())
  if (m) {
    const mult = { m: 60_000, h: 3_600_000, d: 86_400_000, w: 604_800_000 }[m[2].toLowerCase() as "m" | "h" | "d" | "w"]
    return new Date(now + Number(m[1]) * mult).toISOString()
  }
  const d = new Date(s)
  if (Number.isNaN(d.getTime())) throw new FilesError("invalid_request", 400, `--expires "${s}": use 7d, 12h, 30m or a date.`)
  return d.toISOString()
}
