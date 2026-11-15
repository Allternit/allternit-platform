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
import { resumeNative } from "@/runtime/teleport/resume"
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
  /** Uncommitted work that did not fit the cloud copy (kept as the cloud had it). */
  conflicts?: { patch: number; untracked: number }
}

export interface Manifest {
  request_id: string
  harness: TeleportHarness
  native_id: string
  model: string
  reasoning?: string
  workspace_name: string
  direction: "local_to_cloud" | "cloud_to_local"
  session_id?: string
  files: { name: string; size: number; sha256: string; kind: "native" | "patch" | "untracked" }[]
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

/** The newest commit on this branch that the remote already has, or undefined. */
export function reachableBase(cwd: string): string | undefined {
  const reach = spawnSync("git", ["-C", cwd, "ls-remote", "--exit-code", "origin", "HEAD"], { timeout: 15_000 })
  if (reach.status !== 0) return undefined
  for (const ref of ["@{u}", "origin/HEAD", "origin/main", "origin/master"]) {
    const r = git(cwd, ["merge-base", "HEAD", ref])
    if (r.ok && r.out) return r.out
  }
  return undefined
}

export interface ProjectFiles {
  /** `git diff --binary <base>`: unpushed commits plus staged and unstaged edits. */
  patch?: Uint8Array
  /** tar of untracked files that git does not ignore. */
  untracked?: Uint8Array
}

const MAX_UNTRACKED_BYTES = 64 * 1024 * 1024

/** What the Mac has beyond the base commit, so the cloud copy matches the working tree. */
export function collectProject(cwd: string, base: string): ProjectFiles {
  const out: ProjectFiles = {}
  const diff = spawnSync("git", ["-C", cwd, "diff", "--binary", base], { maxBuffer: 256 * 1024 * 1024 })
  if (diff.status === 0 && diff.stdout.byteLength > 0) out.patch = new Uint8Array(diff.stdout)
  const names = spawnSync("git", ["-C", cwd, "ls-files", "--others", "--exclude-standard", "-z"], { maxBuffer: 64 * 1024 * 1024 })
  if (names.status === 0 && names.stdout.byteLength > 0) {
    const tar = spawnSync("tar", ["-cf", "-", "--null", "-T", "-"], { cwd, input: names.stdout, maxBuffer: 256 * 1024 * 1024 })
    if (tar.status !== 0) throw new Error("Could not pack the untracked files.")
    if (tar.stdout.byteLength > MAX_UNTRACKED_BYTES) throw new Error("Untracked files are over 64 MB. Commit them or add them to .gitignore, then retry.")
    out.untracked = new Uint8Array(tar.stdout)
  }
  return out
}

/**
 * The manifest for a Mac -> cloud move, and the bytes of each file in manifest order.
 * `request_id` is derived from the session and the content, so a retry of the same move is
 * the same transfer, and a move after the conversation or the code changed is a new one.
 */
export function buildManifest(
  input: TeleportInput,
  file: { name: string; bytes: Uint8Array },
  base: string,
  project: ProjectFiles = {},
): { manifest: Manifest; payloads: Uint8Array[] } {
  const entries: Manifest["files"] = []
  const payloads: Uint8Array[] = []
  const add = (name: string, bytes: Uint8Array, kind: Manifest["files"][number]["kind"]) => {
    entries.push({ name, size: bytes.byteLength, sha256: sha256(bytes), kind })
    payloads.push(bytes)
  }
  add(file.name, file.bytes, "native")
  if (project.patch) add("project.patch", project.patch, "patch")
  if (project.untracked) add("untracked.tar", project.untracked, "untracked")
  const all = sha256(Buffer.from(entries.map((e) => e.sha256).join(":")))
  const remote = git(input.cwd, ["remote", "get-url", "origin"])
  return {
    manifest: {
      request_id: `tp-${input.sessionID.replace(/[^A-Za-z0-9_-]/g, "").slice(-24)}-${all.slice(0, 16)}`,
      harness: input.harness,
      native_id: input.nativeId,
      session_id: input.sessionID,
      model: input.model,
      reasoning: input.reasoning,
      workspace_name: workspaceName(input.cwd),
      direction: "local_to_cloud",
      files: entries,
      git_remote: remote.ok && remote.out ? remote.out : undefined,
      git_base: base,
      handoff: handoffText("cloud", "the user's Mac"),
      queued: input.queued ?? [],
    },
    payloads,
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
    body: body === undefined ? undefined : raw ? (body as Uint8Array<ArrayBuffer>) : JSON.stringify(body),
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

/** Wait for the cloud to resume the session: `active` means it produced new output. */
async function waitActive(targetId: string, id: string, timeoutMs = 180_000): Promise<void> {
  const until = Date.now() + timeoutMs
  while (Date.now() < until) {
    const t = (await (await call("GET", relay(targetId, `teleports/${id}`))).json()) as { state: string; error?: string }
    if (t.state === "active") return
    if (t.state === "resume_failed") throw new Error(`The cloud copy was installed but could not resume: ${t.error ?? "unknown error"}. Run the teleport again to retry the resume.`)
    await new Promise((r) => setTimeout(r, 1500))
  }
  throw new Error("The cloud copy was installed but has not produced output yet. Check it with the same command; placement was not changed.")
}

/**
 * Mac -> cloud: stage, upload, activate (the cloud installs the file, applies the code and
 * resumes the session), and only then flip placement so the apps follow the session.
 */
export async function teleportToCloud(input: TeleportInput): Promise<TeleportResult> {
  const path = localNativePath(input.harness, input.nativeId, input.cwd)
  if (!path || !existsSync(path)) throw new Error(`No ${input.harness} session file for ${input.nativeId} on this Mac.`)
  // The cloud gets the code by cloning, so it needs a commit the remote has. Whatever the Mac has
  // beyond that commit (unpushed commits, edits, new files) travels as a patch and a tar.
  const base = reachableBase(input.cwd)
  if (!base) {
    throw new Error("This workspace has no reachable git remote with your current branch on it. Push it (or add an origin) so the cloud computer can clone it, then retry.")
  }
  const project = collectProject(input.cwd, base)
  const bytes = readFileSync(path) // frozen here: the manifest hash covers exactly what is sent
  const { manifest, payloads } = buildManifest(input, { name: basename(path), bytes }, base, project)
  const staged = (await (await call("POST", relay(input.targetId, "teleports"), manifest)).json()) as {
    id: string
    state: string
    workspace?: string
    files_pending: number[]
  }
  if (staged.state === "staged") {
    for (const index of staged.files_pending) {
      await call("PUT", relay(input.targetId, `teleports/${staged.id}/files/${index}`), payloads[index]!)
    }
  }
  const activated = (await (await call("POST", relay(input.targetId, `teleports/${staged.id}/activate`), {})).json()) as {
    project?: { patch_conflicts?: number; untracked_conflicts?: number } | null
  }
  await waitActive(input.targetId, staged.id)
  await setPlacement(input.sessionID, input.targetId)
  const pc = activated.project?.patch_conflicts ?? 0
  const uc = activated.project?.untracked_conflicts ?? 0
  return {
    direction: "cloud",
    nativeId: input.nativeId,
    resume: resumeHint(input.harness, input.nativeId),
    workspace: staged.workspace,
    conflicts: pc || uc ? { patch: pc, untracked: uc } : undefined,
  }
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
  // The cloud diffs against its own snapshot taken after it applied the Mac's work, so only
  // what happened in the cloud comes back; `base` is the fallback for sessions born there.
  const gitBase = input.gitBase ?? reachableBase(input.cwd)
  if (gitBase) {
    const q = new URLSearchParams({ workspace_name: workspaceName(input.cwd), base: gitBase })
    const diff = new Uint8Array(await (await call("GET", relay(input.targetId, `teleports/export/${input.harness}/${encodeURIComponent(input.nativeId)}/diff?${q}`))).arrayBuffer())
    if (diff.byteLength > 0) {
      const r = spawnSync("git", ["-C", input.cwd, "apply", "--binary", "-"], { input: Buffer.from(diff) })
      if (r.status !== 0) throw new Error(`The conversation came back, but its code changes did not apply cleanly: ${r.stderr?.toString().trim()}`)
      patched = true
    }
  }
  // Mirror of activation: resume natively here, handoff first, and flip placement only after the
  // first new output.
  const started = await resumeNative({
    sessionID: input.sessionID,
    harness: input.harness,
    nativeId: input.nativeId,
    cwd: input.cwd,
    model: input.model,
    effort: input.reasoning,
    handoff: handoffText("local", "the cloud computer"),
    queued: input.queued ?? [],
  })
  started.done.catch(() => {})
  await setPlacement(input.sessionID, null)
  return { direction: "local", nativeId: input.nativeId, resume: resumeHint(input.harness, input.nativeId), patched }
}
