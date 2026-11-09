/**
 * Teleport client: move a live coding session between this Mac and a connected
 * cloud computer. The conversation travels as the harness's own native file
 * (Claude jsonl, Codex rollout), the code travels through git, and the session
 * resumes natively with `--resume` on the other side.
 *
 * Calls go through the local allternit-api, which relays to the connected
 * server with that server's stored token (`/api/v1/teleports/to/:target/...`),
 * so this process never holds the server's credentials. Free of CLI/UI imports.
 */
import { createHash, randomUUID } from "node:crypto"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs"
import { basename, dirname, join } from "node:path"
import { encodeClaudeCwd, getNativeSession, harnessHome, type HarnessId } from "@allternit/native-sessions"
import { platformApiBase, platformToken, PlatformApiError, PlatformSignedOutError } from "@/runtime/bots/platform-api"

export type TeleportHarness = "claude" | "codex"
export type TeleportDirection = "cloud" | "local"

export interface TeleportInput {
  /** The gizzi session whose placement flips (falls back to the native id). */
  sessionID: string
  harness: TeleportHarness
  nativeId: string
  cwd: string
  model: string
  reasoning?: string
  /** `remote_backend_targets` id of the cloud computer. */
  targetId: string
  queued?: string[]
}

export interface TeleportResult {
  direction: TeleportDirection
  nativeId: string
  resume: string
  workspace?: string
  patched?: boolean
}

export interface Manifest {
  request_id: string
  harness: TeleportHarness
  native_id: string
  model: string
  reasoning?: string
  workspace_name: string
  direction: "local_to_cloud" | "cloud_to_local"
  files: { name: string; size: number; sha256: string }[]
  git_remote?: string
  git_base?: string
  handoff: string
  queued: string[]
}

const sha256 = (buf: Uint8Array) => createHash("sha256").update(buf).digest("hex")

function git(cwd: string, args: string[]): { ok: boolean; out: string } {
  const r = spawnSync("git", ["-C", cwd, ...args], { encoding: "utf8" })
  return { ok: r.status === 0, out: (r.stdout ?? "").trim() }
}

/** A folder name the cloud computer can use for this project. */
export function workspaceName(cwd: string): string {
  return basename(cwd).replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 64) || "workspace"
}

export function handoffText(direction: TeleportDirection, from: string): string {
  const where = direction === "cloud" ? "a cloud computer" : "the user's Mac"
  return (
    `This session was moved from ${from} to ${where} with Teleport. The conversation above is complete and unchanged. ` +
    `The code is the same commit plus any changes already made. Continue where you left off; re-check the ` +
    `working tree before editing, because absolute paths and installed tools may differ here.`
  )
}

/**
 * The manifest for a Mac -> cloud move. `request_id` is derived from the session
 * and file content, so a retry of the same move is the same transfer, and a move
 * after the conversation grew is a new one.
 */
export function buildManifest(input: TeleportInput, file: { name: string; bytes: Uint8Array }): Manifest {
  const digest = sha256(file.bytes)
  const remote = git(input.cwd, ["remote", "get-url", "origin"])
  const head = git(input.cwd, ["rev-parse", "HEAD"])
  return {
    request_id: `tp-${input.sessionID.replace(/[^A-Za-z0-9_-]/g, "").slice(-24)}-${digest.slice(0, 16)}`,
    harness: input.harness,
    native_id: input.nativeId,
    model: input.model,
    reasoning: input.reasoning,
    workspace_name: workspaceName(input.cwd),
    direction: "local_to_cloud",
    files: [{ name: file.name, size: file.bytes.byteLength, sha256: digest }],
    git_remote: remote.ok && remote.out ? remote.out : undefined,
    git_base: head.ok ? head.out : undefined,
    handoff: handoffText("cloud", "the user's Mac"),
    queued: input.queued ?? [],
  }
}

/** Where this Mac keeps the native file for a session. */
export function localNativePath(harness: TeleportHarness, nativeId: string, cwd: string): string | undefined {
  const found = getNativeSession(harness as HarnessId, nativeId, { cwd })
  return found?.path
}

function relay(targetId: string, path: string): string {
  return `${platformApiBase()}/api/v1/teleports/to/${encodeURIComponent(targetId)}/${path}`
}

async function call(method: string, url: string, body?: unknown | Uint8Array): Promise<Response> {
  const token = await platformToken()
  if (!token) throw new PlatformSignedOutError()
  const raw = body instanceof Uint8Array
  const res = await fetch(url, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { "Content-Type": raw ? "application/octet-stream" : "application/json" }),
    },
    body: body === undefined ? undefined : raw ? body : JSON.stringify(body),
  })
  if (!res.ok) {
    const text = await res.text().catch(() => "")
    let message = text
    try {
      const parsed = JSON.parse(text) as { message?: string; error?: string }
      message = parsed.message || parsed.error || text
    } catch {}
    throw new PlatformApiError(res.status, message || `HTTP ${res.status}`)
  }
  return res
}

async function setPlacement(sessionID: string, targetId: string | null) {
  await call("PUT", `${platformApiBase()}/api/v1/teleports/placement`, { sessionId: sessionID, targetId })
}

/** Mac -> cloud: stage, upload, activate, then flip placement so the apps follow the session. */
export async function teleportToCloud(input: TeleportInput): Promise<TeleportResult> {
  const path = localNativePath(input.harness, input.nativeId, input.cwd)
  if (!path || !existsSync(path)) throw new Error(`No ${input.harness} session file for ${input.nativeId} on this Mac.`)
  if (git(input.cwd, ["status", "--porcelain"]).out) {
    throw new Error("The workspace has uncommitted changes. Commit and push them first: Teleport moves the conversation, git moves the code.")
  }
  const bytes = readFileSync(path) // frozen here: the manifest hash covers exactly what is sent
  const manifest = buildManifest(input, { name: basename(path), bytes })
  const staged = (await (await call("POST", relay(input.targetId, "teleports"), manifest)).json()) as {
    id: string
    state: string
    workspace?: string
    files_pending: number[]
  }
  if (staged.state === "staged") {
    for (const index of staged.files_pending) {
      await call("PUT", relay(input.targetId, `teleports/${staged.id}/files/${index}`), bytes)
    }
    await call("POST", relay(input.targetId, `teleports/${staged.id}/activate`), {})
  }
  await setPlacement(input.sessionID, input.targetId)
  return { direction: "cloud", nativeId: input.nativeId, resume: resumeHint(input.harness, input.nativeId), workspace: staged.workspace }
}

export function resumeHint(harness: TeleportHarness, nativeId: string): string {
  return harness === "claude" ? `claude --resume ${nativeId}` : `codex resume ${nativeId}`
}

/** Cloud -> Mac: download the native file and apply the git diff since the base commit. */
export async function teleportToLocal(input: TeleportInput & { gitBase?: string }): Promise<TeleportResult> {
  const res = await call("GET", relay(input.targetId, `teleports/export/${input.harness}/${encodeURIComponent(input.nativeId)}`))
  const [name, remoteSha] = (res.headers.get("x-teleport-file") ?? "").split(";")
  const bytes = new Uint8Array(await res.arrayBuffer())
  if (!name || sha256(bytes) !== remoteSha) throw new Error("The downloaded session file failed its checksum; nothing was installed.")

  const existing = localNativePath(input.harness, input.nativeId, input.cwd)
  const target =
    existing ??
    (input.harness === "claude"
      ? join(harnessHome("claude"), "projects", encodeClaudeCwd(input.cwd), `${input.nativeId}.jsonl`)
      : join(harnessHome("codex"), "sessions", new Date().toISOString().slice(0, 10).replace(/-/g, "/"), name))
  if (existing) {
    // The Mac copy froze when it left, so the cloud copy must begin with it; anything else is a fork.
    const before = readFileSync(existing)
    if (before.byteLength > bytes.byteLength || sha256(before) !== sha256(bytes.subarray(0, before.byteLength))) {
      throw new Error("This Mac's copy of the session changed after it was teleported. Not overwriting it; fork instead.")
    }
  }
  mkdirSync(dirname(target), { recursive: true })
  const tmp = `${target}.${randomUUID().slice(0, 8)}.tmp`
  writeFileSync(tmp, bytes)
  renameSync(tmp, target)

  let patched = false
  if (input.gitBase) {
    const q = new URLSearchParams({ workspace_name: workspaceName(input.cwd), base: input.gitBase })
    const diff = new Uint8Array(await (await call("GET", relay(input.targetId, `teleports/export/${input.harness}/${encodeURIComponent(input.nativeId)}/diff?${q}`))).arrayBuffer())
    if (diff.byteLength > 0) {
      const r = spawnSync("git", ["-C", input.cwd, "apply", "--binary", "--index", "-"], { input: Buffer.from(diff) })
      if (r.status !== 0) throw new Error(`The conversation came back, but its code changes did not apply cleanly: ${r.stderr?.toString().trim()}`)
      patched = true
    }
  }
  await setPlacement(input.sessionID, null)
  return { direction: "local", nativeId: input.nativeId, resume: resumeHint(input.harness, input.nativeId), patched }
}
