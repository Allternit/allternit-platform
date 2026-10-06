/**
 * A local git checkout of one Memory Drive.
 *
 * Writes are stable-id operations (see format.ts). Every write:
 *   1. serializes on the checkout (in-process chain + cross-process lock)
 *   2. folds any hand/tool edits in the working tree into a commit first
 *      (validated; anything invalid is moved aside, never silently dropped)
 *   3. fetches and integrates the remote when there is one
 *   4. applies the operations to the current files, regenerates MEMORY.md's
 *      index, validates and secret-scans the WHOLE candidate tree, commits
 *   5. pushes; on a non-fast-forward rejection it fetches, re-applies the
 *      local intent (entry-level diff since the common base) on top of the
 *      latest files as ONE commit and retries (bounded). Offline or still
 *      failing: the local commit is kept and the state file records a visible
 *      pending/error that the next session retries.
 * Nothing here ever force-pushes, runs `git reset`, or deletes history.
 */
import { existsSync } from "fs"
import { lstat, mkdir, readdir, readFile, rename, rm, rmdir, writeFile, copyFile } from "fs/promises"
import os from "os"
import path from "path"
import {
  applyOperations,
  diffOperations,
  DriveFormatError,
  DriveSecretError,
  ENTRYPOINT,
  isEntryLine,
  isManagedPath,
  normalizeFiles,
  parseEntry,
  rebuildIndex,
  rustLines,
  sameFiles,
  SEED_MEMORY,
  sortedFiles,
  validateCommitMessage,
  validateFile,
  validateFiles,
  validatePath,
  type DriveFiles,
  type Entry,
  type Operation,
} from "./format"
import { redactGitOutput, runGit, type GitAuthor, type GitResult } from "./git"
import { withDriveLock } from "./lock"

export interface DriveRemote {
  /** Credential-free clone URL (`/api/v1/brains/<id>/git`). */
  url: string
  /** Remote branch (contract `branch`, default main). */
  branch?: string
  /** The current credential; minted on demand. undefined = cannot authenticate. */
  token(): Promise<string | undefined>
  /** Mint a fresh credential after the server refused the current one. */
  refresh?(): Promise<string | undefined>
  /** Read-only mount: fetch only, never commit or push. */
  readOnly?: boolean
}

export interface CheckoutOptions {
  dir: string
  remote?: DriveRemote
  author: GitAuthor
  /** Where invalid hand edits are moved (outside the checkout). */
  quarantineDir?: string
  maxPushAttempts?: number
  /** Default SOURCE stamped on bullets written without provenance. */
  defaultSource?: string
}

export interface SyncState {
  lastSyncAt?: string
  lastError?: string
  lastErrorAt?: string
  mergedSignedOutFrom?: string
  quarantined?: string[]
  /** Visible until dismissed: edits that failed validation and were moved aside. */
  rejected?: string
  rejectedAt?: string
}

export interface DriveStatus {
  dir: string
  head?: string
  remoteHead?: string
  remote?: string
  readOnly: boolean
  /** Local commits not yet on the server. */
  ahead: number
  pending: boolean
  dirty: boolean
  lastSyncAt?: string
  lastError?: string
  lastErrorAt?: string
  quarantined?: string[]
  rejected?: string
  rejectedAt?: string
}

export interface WriteResult {
  revision?: string
  changed: boolean
  pushed: boolean
  pending: boolean
  error?: string
}

export interface CommitInfo {
  revision: string
  parents: string[]
  author: string
  timestamp: string
  message: string
}

export interface SearchHit {
  path: string
  line: number
  text: string
  entry?: Entry
}

export class DriveGitError extends Error {
  constructor(operation: string, result: GitResult) {
    super(`git ${operation} failed: ${redactGitOutput(result.stderr || result.stdout) || `exit ${result.code}`}`)
    this.name = "DriveGitError"
  }
}

export class DriveReadOnlyError extends Error {
  constructor() {
    super("This memory drive is mounted read-only for you, so gizzi cannot write to it.")
    this.name = "DriveReadOnlyError"
  }
}

type PushOutcome =
  | { kind: "ok" }
  | { kind: "rejected" }
  | { kind: "auth"; message: string }
  | { kind: "invalid"; message: string }
  | { kind: "offline"; message: string }

interface Tree {
  files: DriveFiles
  oids: Record<string, string>
}

const NETWORK_TIMEOUT_MS = 30_000
/** After a network failure, skip further network attempts in this process for a while. */
const OFFLINE_BACKOFF_MS = 60_000
/** Files operating systems drop into folders; never part of a drive, never "edits". */
const OS_JUNK = new Set([".DS_Store", "Thumbs.db", "desktop.ini", "Icon\r"])
const LOCAL_BRANCH = "main"
const LOCAL_REF = `refs/heads/${LOCAL_BRANCH}`
const REMOTE_REF = "refs/remotes/origin/main"

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function classifyPush(result: GitResult): PushOutcome {
  if (result.code === 0) return { kind: "ok" }
  const text = `${result.stdout}\n${result.stderr}`
  const refused = /Memory Drive refused this push:\s*([^\n]*)/i.exec(text)
  if (refused) {
    return { kind: "invalid", message: `The memory server refused the change: ${redactGitOutput(refused[1]!.trim()) || "no reason given"}` }
  }
  // A concurrent update of the same ref on the server is a race, not a refusal: retry.
  if (/remote rejected\][^\n]*\((cannot lock ref|failed to (update|lock) ref|incorrect old value|stale info|fetch first|non-fast-forward)/i.test(text)) {
    return { kind: "rejected" }
  }
  if (/remote rejected|pre-receive hook declined|\bdeclined\b/i.test(text)) {
    const reason =
      /\[remote rejected\][^\n]*\(([^)]*)\)/i.exec(text)?.[1] ??
      text
        .split("\n")
        .filter((l) => l.startsWith("remote:"))
        .map((l) => l.replace(/^remote:\s*/, ""))
        .join(" ")
        .trim()
    return { kind: "invalid", message: `The memory server refused the change: ${redactGitOutput(reason) || "no reason given"}` }
  }
  if (/\[rejected\]|non-fast-forward|fetch first|stale info|failed to update ref/i.test(text)) return { kind: "rejected" }
  if (/authentication failed|HTTP 401|HTTP 403|could not read (username|password)|terminal prompts disabled|returned error: 40[13]/i.test(text)) {
    return { kind: "auth", message: "The memory server did not accept gizzi's drive token." }
  }
  return {
    kind: "offline",
    message: `Could not reach the memory server (${redactGitOutput(result.stderr) || (result.timedOut ? "timed out" : `exit ${result.code}`)}).`,
  }
}

export class DriveCheckout {
  readonly dir: string
  private readonly remote?: DriveRemote
  private readonly author: GitAuthor
  private readonly quarantineDir: string
  private readonly maxPushAttempts: number
  readonly defaultSource: string
  private cachedToken?: string
  private offlineAt = 0

  constructor(options: CheckoutOptions) {
    this.dir = path.resolve(options.dir)
    this.remote = options.remote
    this.author = options.author
    this.quarantineDir = options.quarantineDir ?? `${this.dir}.rejected`
    this.maxPushAttempts = options.maxPushAttempts ?? 5
    this.defaultSource = options.defaultSource ?? "gizzi:session/unknown"
  }

  get readOnly(): boolean {
    return !!this.remote?.readOnly
  }

  get hasRemote(): boolean {
    return !!this.remote
  }

  // ── git plumbing ──────────────────────────────────────────────────────────

  private git(args: string[], extra: { token?: string; input?: string | Uint8Array; env?: Record<string, string>; timeoutMs?: number; cwd?: string } = {}) {
    return runGit(args, { cwd: extra.cwd ?? this.dir, author: this.author, ...extra })
  }

  private async gitOk(operation: string, args: string[], extra: Parameters<DriveCheckout["git"]>[1] = {}): Promise<string> {
    const result = await this.git(args, extra)
    if (result.code !== 0) throw new DriveGitError(operation, result)
    return result.stdout
  }

  private async rev(ref: string): Promise<string | undefined> {
    const r = await this.git(["rev-parse", "--verify", "--quiet", `${ref}^{commit}`])
    return r.code === 0 && r.stdout ? r.stdout : undefined
  }

  private async isAncestor(a: string, b: string): Promise<boolean> {
    return (await this.git(["merge-base", "--is-ancestor", a, b])).code === 0
  }

  private async mergeBase(a: string, b: string): Promise<string | undefined> {
    const r = await this.git(["merge-base", a, b])
    return r.code === 0 && r.stdout ? r.stdout : undefined
  }

  private async token(): Promise<string | undefined> {
    if (!this.remote) return undefined
    if (this.cachedToken) return this.cachedToken
    this.cachedToken = await this.remote.token().catch(() => undefined)
    return this.cachedToken
  }

  private async refreshToken(): Promise<boolean> {
    if (!this.remote?.refresh) return false
    const next = await this.remote.refresh().catch(() => undefined)
    if (!next || next === this.cachedToken) return false
    this.cachedToken = next
    return true
  }

  /** Read every markdown blob in `rev` (one ls-tree + one cat-file --batch). */
  async readTree(rev: string): Promise<Tree> {
    const listing = await this.git(["ls-tree", "-r", "-z", "--full-tree", rev])
    if (listing.code !== 0) throw new DriveGitError("ls-tree", listing)
    const records = listing.stdout
      .split("\0")
      .filter(Boolean)
      .map((rec) => {
        const tab = rec.indexOf("\t")
        const [mode, type, oid] = rec.slice(0, tab).split(" ") as [string, string, string]
        return { mode, type, oid, path: rec.slice(tab + 1) }
      })
      .filter((r) => r.type === "blob" && r.path.endsWith(".md"))
    const files: DriveFiles = {}
    const oids: Record<string, string> = {}
    if (records.length === 0) return { files, oids }
    const batch = await this.git(["cat-file", "--batch"], { input: records.map((r) => r.oid).join("\n") + "\n" })
    if (batch.code !== 0) throw new DriveGitError("cat-file", batch)
    const buf = Buffer.from(batch.stdoutBytes)
    let offset = 0
    for (const record of records) {
      const nl = buf.indexOf(0x0a, offset)
      const header = buf.subarray(offset, nl).toString("utf8").split(" ")
      const size = Number(header[2])
      const start = nl + 1
      files[record.path] = buf.subarray(start, start + size).toString("utf8")
      oids[record.path] = record.oid
      offset = start + size + 1
    }
    return { files, oids }
  }

  /** Write a commit whose tree is exactly `files`, reusing blob ids from `known`. */
  private async commitFiles(files: DriveFiles, parents: string[], message: string, known: Record<string, string> = {}, knownFiles: DriveFiles = {}): Promise<string> {
    validateCommitMessage(message)
    const scratch = path.join(this.dir, ".git", `allternit-tmp-${process.pid}-${Math.random().toString(36).slice(2)}`)
    const index = `${scratch}.index`
    const env = { GIT_INDEX_FILE: index }
    try {
      const oids: Record<string, string> = {}
      const fresh: string[] = []
      for (const p of sortedFiles(files)) {
        if (knownFiles[p] === files[p] && known[p]) oids[p] = known[p]!
        else fresh.push(p)
      }
      if (fresh.length > 0) {
        // One hash-object for every new blob (contents staged outside the tree).
        await mkdir(scratch, { recursive: true })
        const staged = await Promise.all(
          fresh.map(async (p, i) => {
            const file = path.join(scratch, String(i))
            await writeFile(file, files[p]!, "utf8")
            return file
          }),
        )
        const out = await this.gitOk("hash-object", ["hash-object", "-w", "--no-filters", "--stdin-paths"], { input: staged.join("\n") + "\n" })
        const hashed = out.split("\n")
        fresh.forEach((p, i) => (oids[p] = hashed[i]!))
      }
      const records = sortedFiles(files).map((p) => `100644 ${oids[p]}\t${p}\0`)
      if (records.length) await this.gitOk("update-index", ["update-index", "-z", "--index-info"], { env, input: records.join("") })
      else await this.gitOk("read-tree", ["read-tree", "--empty"], { env })
      const tree = await this.gitOk("write-tree", ["write-tree"], { env })
      const args = ["commit-tree", tree]
      for (const p of parents) args.push("-p", p)
      args.push("-F", "-")
      return await this.gitOk("commit-tree", args, { input: `${message}\n` })
    } finally {
      await rm(index, { force: true }).catch(() => {})
      await rm(scratch, { recursive: true, force: true }).catch(() => {})
    }
  }

  /**
   * Move the local branch to `commit` and make the working tree hold exactly
   * `files`. Only called once the working tree has no uncommitted edits.
   */
  private async materialize(commit: string, files: DriveFiles, expectedHead: string | undefined): Promise<void> {
    const update = ["update-ref", LOCAL_REF, commit]
    if (expectedHead) update.push(expectedHead)
    await this.gitOk("update-ref", update)
    const onDisk = await this.workingFiles()
    for (const p of sortedFiles(files)) {
      if (onDisk.files[p] === files[p]) continue
      const target = path.join(this.dir, p)
      await mkdir(path.dirname(target), { recursive: true })
      await writeFile(target, files[p]!, "utf8")
    }
    for (const p of Object.keys(onDisk.files)) {
      if (!(p in files)) await rm(path.join(this.dir, p), { force: true })
    }
    await this.pruneEmptyDirs(this.dir)
    await this.gitOk("read-tree", ["read-tree", commit])
  }

  private async pruneEmptyDirs(dir: string): Promise<boolean> {
    const entries = await readdir(dir, { withFileTypes: true }).catch(() => [])
    let empty = true
    for (const e of entries) {
      if (e.name === ".git" && dir === this.dir) {
        empty = false
        continue
      }
      if (e.isDirectory()) {
        const child = path.join(dir, e.name)
        if (await this.pruneEmptyDirs(child)) await rmdir(child).catch(() => {})
        else empty = false
      } else empty = false
    }
    return empty && dir !== this.dir
  }

  // ── state ─────────────────────────────────────────────────────────────────

  private get statePath(): string {
    return path.join(this.dir, ".git", "allternit-memory-sync.json")
  }

  async readState(): Promise<SyncState> {
    return readFile(this.statePath, "utf8")
      .then((t) => JSON.parse(t) as SyncState)
      .catch(() => ({}))
  }

  private async patchState(patch: Partial<SyncState>): Promise<void> {
    if (!existsSync(path.join(this.dir, ".git"))) return
    const next = { ...(await this.readState()), ...patch }
    for (const k of Object.keys(next) as (keyof SyncState)[]) if (next[k] === undefined) delete next[k]
    await writeFile(this.statePath, JSON.stringify(next, null, 2))
  }

  private async recordError(message: string): Promise<void> {
    await this.patchState({ lastError: message, lastErrorAt: new Date().toISOString() })
  }

  // ── lifecycle ─────────────────────────────────────────────────────────────

  /** Create or open the checkout (clone when a remote is reachable, else a local repo). */
  ensure(): Promise<void> {
    return withDriveLock(this.dir, () => this.ensureUnlocked())
  }

  private ensured = false

  private async ensureUnlocked(): Promise<void> {
    if (existsSync(path.join(this.dir, ".git"))) {
      if (this.ensured) return
      await this.configureRemote()
      if (!(await this.rev("HEAD"))) await this.seedUnlocked()
      this.ensured = true
      return
    }
    this.ensured = false
    if (existsSync(this.dir)) {
      const entries = await readdir(this.dir)
      if (entries.length === 0) await rmdir(this.dir)
      else {
        // A non-repo directory sits where the checkout belongs (e.g. created
        // by an older build). Keep it, out of the way; nothing is deleted.
        await rename(this.dir, `${this.dir}.orphaned-${Date.now()}`)
      }
    }
    const parent = path.dirname(this.dir)
    await mkdir(parent, { recursive: true, mode: 0o700 })
    const tmp = `${this.dir}.init-${process.pid}-${Date.now()}`
    await mkdir(tmp, { recursive: true, mode: 0o700 })
    try {
      let cloned = false
      if (this.remote) {
        const token = await this.token()
        const r = await runGit(["clone", "--quiet", "--no-tags", "--origin", "origin", "--", this.remote.url, tmp], {
          cwd: parent,
          token,
          author: this.author,
          timeoutMs: NETWORK_TIMEOUT_MS,
        })
        cloned = r.code === 0
        if (!cloned) {
          await rm(tmp, { recursive: true, force: true })
          await mkdir(tmp, { recursive: true, mode: 0o700 })
          await this.recordErrorLater(classifyPush(r))
        }
      }
      if (!cloned) {
        const init = await runGit(["init", "--quiet"], { cwd: tmp })
        if (init.code !== 0) throw new DriveGitError("init", init)
        if (this.remote) await runGit(["remote", "add", "origin", this.remote.url], { cwd: tmp })
      }
      await runGit(["symbolic-ref", "HEAD", LOCAL_REF], { cwd: tmp })
      if (cloned) {
        // Clone checked out the remote default branch; pin the local branch name.
        const head = await runGit(["rev-parse", "--verify", "--quiet", "refs/remotes/origin/HEAD^{commit}"], { cwd: tmp })
        const current = await runGit(["rev-parse", "--verify", "--quiet", `${LOCAL_REF}^{commit}`], { cwd: tmp })
        if (current.code !== 0 && head.code === 0) await runGit(["update-ref", LOCAL_REF, head.stdout], { cwd: tmp })
      }
      try {
        await rename(tmp, this.dir)
      } catch (error) {
        if (!existsSync(path.join(this.dir, ".git"))) throw error
        await rm(tmp, { recursive: true, force: true })
      }
    } catch (error) {
      await rm(tmp, { recursive: true, force: true }).catch(() => {})
      throw error
    }
    await this.flushPendingInitError()
    await this.configureRemote()
    if (!(await this.rev("HEAD"))) await this.seedUnlocked()
    this.ensured = true
  }

  private pendingInitError?: string
  private async recordErrorLater(outcome: PushOutcome): Promise<void> {
    if (outcome.kind !== "ok" && outcome.kind !== "rejected") this.pendingInitError = outcome.message
  }
  private async flushPendingInitError(): Promise<void> {
    if (this.pendingInitError) await this.recordError(this.pendingInitError)
    this.pendingInitError = undefined
  }

  private async configureRemote(): Promise<void> {
    const exclude = path.join(this.dir, ".git", "info", "exclude")
    const current = await readFile(exclude, "utf8").catch(() => "")
    if (!current.includes(".DS_Store")) {
      await mkdir(path.dirname(exclude), { recursive: true })
      await writeFile(exclude, `${current}${current && !current.endsWith("\n") ? "\n" : ""}.DS_Store\nThumbs.db\ndesktop.ini\n`)
    }
    if (!this.remote) return
    const origin = await this.git(["remote", "get-url", "origin"])
    if (origin.code !== 0) await this.gitOk("remote add", ["remote", "add", "origin", this.remote.url])
    else if (origin.stdout !== this.remote.url) await this.gitOk("remote set-url", ["remote", "set-url", "origin", this.remote.url])
  }

  /** First commit of a brand-new drive: the standard MEMORY.md. */
  private async seedUnlocked(): Promise<void> {
    const files: DriveFiles = { [ENTRYPOINT]: SEED_MEMORY }
    rebuildIndex(files)
    const commit = await this.commitFiles(files, [], "Initialize memory drive")
    await this.materialize(commit, files, undefined)
  }

  // ── reads ─────────────────────────────────────────────────────────────────

  async head(): Promise<string | undefined> {
    return this.rev(LOCAL_REF)
  }

  /** Markdown files in the working tree, plus anything that cannot be in a drive. */
  async workingFiles(): Promise<{ files: DriveFiles; problems: string[] }> {
    const files: DriveFiles = {}
    const problems: string[] = []
    const walk = async (dir: string, rel: string) => {
      const entries = await readdir(dir, { withFileTypes: true }).catch(() => [])
      for (const e of entries) {
        if (!rel && e.name === ".git") continue
        if (OS_JUNK.has(e.name)) continue
        const relPath = rel ? `${rel}/${e.name}` : e.name
        const abs = path.join(dir, e.name)
        const st = await lstat(abs).catch(() => undefined)
        if (!st) continue
        if (st.isSymbolicLink()) problems.push(relPath)
        else if (st.isDirectory()) await walk(abs, relPath)
        else if (!st.isFile() || !e.name.endsWith(".md") || (st.mode & 0o111) !== 0) problems.push(relPath)
        else files[relPath] = await readFile(abs, "utf8")
      }
    }
    await walk(this.dir, "")
    return { files, problems }
  }

  async readFile(relPath: string): Promise<string> {
    validatePath(relPath)
    return readFile(path.join(this.dir, relPath), "utf8")
  }

  async listFiles(): Promise<{ path: string; bytes: number; entries: number }[]> {
    const { files } = await this.workingFiles()
    return sortedFiles(files).map((p) => ({
      path: p,
      bytes: Buffer.byteLength(files[p]!, "utf8"),
      entries: rustLines(files[p]!).filter(isEntryLine).length,
    }))
  }

  async history(limit = 25, relPath?: string): Promise<CommitInfo[]> {
    if (!(await this.head())) return []
    const args = ["log", `-n${Math.max(1, Math.min(limit, 100))}`, "--format=%H%x1f%P%x1f%an%x1f%aI%x1f%s", LOCAL_REF]
    if (relPath) {
      validatePath(relPath)
      args.push("--", relPath)
    }
    const out = await this.gitOk("log", args)
    return out
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const [revision, parents, author, timestamp, message] = line.split("\x1f") as string[]
        return { revision: revision!, parents: parents ? parents.split(" ") : [], author: author!, timestamp: timestamp!, message: message ?? "" }
      })
  }

  /** Case-insensitive search over the working tree; every term must match the line. */
  async search(query: string, limit = 50): Promise<SearchHit[]> {
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean)
    const { files } = await this.workingFiles()
    const hits: SearchHit[] = []
    for (const p of sortedFiles(files)) {
      const lines = rustLines(files[p]!)
      let inIndex = false
      for (let i = 0; i < lines.length; i++) {
        const line = lines[i]!
        if (p === ENTRYPOINT && line === "## Index") inIndex = true
        if (inIndex || !line.trim() || line.startsWith("#")) continue
        const hay = line.toLowerCase()
        if (terms.length > 0 && !terms.every((t) => hay.includes(t))) continue
        let entry: Entry | undefined
        try {
          entry = isEntryLine(line) ? parseEntry(line) : undefined
        } catch {
          entry = undefined
        }
        hits.push({ path: p, line: i + 1, text: entry?.text ?? line, entry })
        if (hits.length >= limit) return hits
      }
    }
    return hits
  }

  async status(): Promise<DriveStatus> {
    const state = await this.readState()
    if (!existsSync(path.join(this.dir, ".git"))) {
      return { dir: this.dir, readOnly: this.readOnly, ahead: 0, pending: false, dirty: false, remote: this.remote?.url, ...state }
    }
    const head = await this.head()
    const remoteHead = this.remote ? await this.rev(REMOTE_REF) : undefined
    let ahead = 0
    if (this.remote && head) {
      const range = remoteHead ? `${remoteHead}..${head}` : head
      const r = await this.git(["rev-list", "--count", range])
      ahead = r.code === 0 ? Number(r.stdout) || 0 : 0
    }
    const dirty = !!(await this.git(["status", "--porcelain", "--untracked-files=all"])).stdout
    return {
      dir: this.dir,
      head,
      remoteHead,
      remote: this.remote?.url,
      readOnly: this.readOnly,
      ahead,
      pending: !!this.remote && !this.readOnly && ahead > 0,
      dirty,
      lastSyncAt: state.lastSyncAt,
      lastError: state.lastError,
      lastErrorAt: state.lastErrorAt,
      quarantined: state.quarantined,
      rejected: state.rejected,
      rejectedAt: state.rejectedAt,
    }
  }

  /** Hide the "edits were not saved" notice (the moved-aside files stay where they are). */
  async dismissRejected(): Promise<void> {
    await this.patchState({ rejected: undefined, rejectedAt: undefined, quarantined: undefined })
  }

  // ── writes ────────────────────────────────────────────────────────────────

  /** Apply stable-id operations as one commit, then publish. */
  write(operations: Operation[], options: { message: string; source?: string }): Promise<WriteResult> {
    return withDriveLock(this.dir, async () => {
      if (this.readOnly) throw new DriveReadOnlyError()
      await this.ensureUnlocked()
      await this.syncUnlocked(options.source)
      const head = await this.head()
      const current = head ? await this.readTree(head) : { files: {}, oids: {} }
      const next = applyOperations(current.files, operations)
      if (head && sameFiles(next, current.files)) {
        const status = await this.status()
        return { revision: head, changed: false, pushed: false, pending: status.pending, error: status.pending ? status.lastError : undefined }
      }
      const commit = await this.commitFiles(next, head ? [head] : [], options.message, current.oids, current.files)
      await this.materialize(commit, next, head)
      return this.publishUnlocked()
    })
  }

  /** Commit hand/tool edits in the working tree (validated), then publish. */
  commitWorkingTree(options: { message?: string; source?: string } = {}): Promise<WriteResult & { quarantined: string[] }> {
    return withDriveLock(this.dir, async () => {
      await this.ensureUnlocked()
      const outcome = await this.commitWorkingTreeUnlocked(options.source, options.message)
      if (this.readOnly || !this.remote) {
        return { revision: await this.head(), changed: outcome.committed, pushed: false, pending: false, error: outcome.error, quarantined: outcome.quarantined }
      }
      const status = await this.status()
      if (!outcome.committed && !status.pending) {
        return { revision: status.head, changed: false, pushed: false, pending: false, error: outcome.error, quarantined: outcome.quarantined }
      }
      const result = await this.publishUnlocked()
      return { ...result, changed: outcome.committed, error: outcome.error ?? result.error, quarantined: outcome.quarantined }
    })
  }

  /** Fetch, fold in remote changes, push anything pending. Safe to call often. */
  sync(options: { source?: string } = {}): Promise<DriveStatus> {
    return withDriveLock(this.dir, async () => {
      await this.ensureUnlocked()
      await this.syncUnlocked(options.source)
      return this.status()
    })
  }

  private async commitWorkingTreeUnlocked(
    source = this.defaultSource,
    message = "Save memory edits",
  ): Promise<{ committed: boolean; error?: string; quarantined: string[] }> {
    const porcelain = await this.git(["status", "--porcelain", "--untracked-files=all"])
    if (!porcelain.stdout) return { committed: false, quarantined: [] }
    if (this.readOnly) {
      return { committed: false, quarantined: [], error: "Edits in a read-only memory drive are not saved." }
    }
    const head = await this.head()
    const base = head ? await this.readTree(head) : { files: {}, oids: {} }
    const { files: onDisk, problems } = await this.workingFiles()
    const bad: { path: string; reason: string }[] = problems.map((p) => ({ path: p, reason: "only markdown files can live in a memory drive" }))
    const candidate = normalizeFiles(onDisk, source)
    if (!(ENTRYPOINT in candidate)) candidate[ENTRYPOINT] = base.files[ENTRYPOINT] ?? SEED_MEMORY
    for (const p of sortedFiles(base.files)) {
      // Managed folders: an on-disk copy that differs only by normalization is the server's file.
      if (isManagedPath(p) && onDisk[p] === base.files[p]) candidate[p] = base.files[p]!
      if (isManagedPath(p) && !(p in candidate)) {
        bad.push({ path: p, reason: "twin/ and cowork/ are managed by Allternit and read-only" })
        candidate[p] = base.files[p]!
      }
    }
    for (const p of sortedFiles(candidate)) {
      if (candidate[p] === base.files[p]) continue
      try {
        validatePath(p)
        if (isManagedPath(p)) throw new DriveFormatError(p, "twin/ and cowork/ are managed by Allternit and read-only")
        if (p === ENTRYPOINT) {
          const probe = { [p]: candidate[p]! }
          rebuildIndex(probe)
          validateFile(p, probe[p]!)
        } else validateFile(p, candidate[p]!)
      } catch (error) {
        bad.push({ path: p, reason: errorMessage(error) })
        if (p in base.files) candidate[p] = base.files[p]!
        else delete candidate[p]
      }
    }
    let next: DriveFiles = candidate
    try {
      rebuildIndex(next)
      validateFiles(next)
    } catch (error) {
      // A cross-file problem (duplicate id, size cap…): keep the last good tree.
      for (const p of sortedFiles(onDisk)) if (onDisk[p] !== base.files[p] && !bad.some((b) => b.path === p)) bad.push({ path: p, reason: errorMessage(error) })
      next = { ...base.files }
    }
    const quarantined = await this.quarantine(bad, onDisk)
    let error: string | undefined
    if (bad.length > 0) {
      error = `Some memory edits were not saved: ${bad
        .slice(0, 3)
        .map((b) => `${b.path} (${b.reason})`)
        .join("; ")}${bad.length > 3 ? ` and ${bad.length - 3} more` : ""}. Your text was kept in ${this.quarantineDir}.`
      const at = new Date().toISOString()
      await this.patchState({ quarantined, rejected: error, rejectedAt: at })
    }
    if (head && sameFiles(next, base.files)) {
      await this.materialize(head, base.files, head)
      return { committed: false, error, quarantined }
    }
    const commit = await this.commitFiles(next, head ? [head] : [], message, base.oids, base.files)
    await this.materialize(commit, next, head)
    return { committed: true, error, quarantined }
  }

  /** Copy rejected working-tree files outside the checkout (never deleted). */
  private async quarantine(bad: { path: string; reason: string }[], onDisk: DriveFiles): Promise<string[]> {
    if (bad.length === 0) return []
    const stamp = new Date().toISOString().replace(/[:.]/g, "-")
    const dest = path.join(this.quarantineDir, stamp)
    const kept: string[] = []
    for (const b of bad) {
      const source = path.join(this.dir, b.path)
      if (!existsSync(source) && !(b.path in onDisk)) continue
      const target = path.join(dest, b.path)
      await mkdir(path.dirname(target), { recursive: true, mode: 0o700 })
      const st = await lstat(source).catch(() => undefined)
      if (st?.isSymbolicLink() || (st && !st.isFile())) {
        await rename(source, target).catch(() => {})
      } else if (st) {
        await copyFile(source, target).catch(() => {})
        // Non-markdown files would block every later commit: move them out.
        if (!b.path.endsWith(".md") || (st.mode & 0o111) !== 0) await rm(source, { force: true })
      }
      kept.push(path.relative(this.quarantineDir, target))
    }
    await writeFile(
      path.join(dest, "REASONS.txt"),
      bad.map((b) => `${b.path}: ${b.reason}`).join("\n") + "\n",
    ).catch(() => {})
    return kept
  }

  private offline(): boolean {
    return Date.now() - this.offlineAt < OFFLINE_BACKOFF_MS
  }

  private async fetchUnlocked(): Promise<{ ok: boolean; message?: string }> {
    if (!this.remote) return { ok: true }
    if (this.offline()) return { ok: false, message: "Could not reach the memory server; changes are saved locally and will sync later." }
    const branch = this.remote.branch ?? "main"
    let refreshed = false
    for (;;) {
      const r = await this.git(["fetch", "--quiet", "--no-tags", "origin", `+refs/heads/${branch}:${REMOTE_REF}`], {
        token: await this.token(),
        timeoutMs: NETWORK_TIMEOUT_MS,
      })
      if (r.code === 0) return { ok: true }
      if (/couldn't find remote ref/i.test(r.stderr)) return { ok: true }
      const outcome = classifyPush(r)
      if (outcome.kind === "auth" && !refreshed && (await this.refreshToken())) {
        refreshed = true
        continue
      }
      if (outcome.kind === "offline") this.offlineAt = Date.now()
      return { ok: false, message: outcome.kind === "ok" || outcome.kind === "rejected" ? "fetch failed" : outcome.message }
    }
  }

  /** Bring the local branch up to date with the fetched remote head. */
  private async integrateUnlocked(): Promise<void> {
    const head = await this.head()
    const remote = await this.rev(REMOTE_REF)
    if (!remote || head === remote) return
    if (!head || (await this.isAncestor(head, remote))) {
      const tree = await this.readTree(remote)
      await this.materialize(remote, tree.files, head)
      return
    }
    if (await this.isAncestor(remote, head)) return
    await this.replayOnto(head, remote)
  }

  /** Re-apply local work since the common base on top of `onto` as one commit. */
  private async replayOnto(head: string, onto: string): Promise<void> {
    const base = await this.mergeBase(head, onto)
    const baseFiles = base ? (await this.readTree(base)).files : {}
    const ours = await this.readTree(head)
    const theirs = await this.readTree(onto)
    const operations = diffOperations(baseFiles, ours.files)
    const next = applyOperations(theirs.files, operations)
    if (sameFiles(next, theirs.files)) {
      await this.materialize(onto, theirs.files, head)
      return
    }
    const changes = operations.length
    const commit = await this.commitFiles(
      next,
      [onto],
      `Sync memory from ${os.hostname().slice(0, 60) || "gizzi"} (${changes} change${changes === 1 ? "" : "s"})`,
      theirs.oids,
      theirs.files,
    )
    await this.materialize(commit, next, head)
  }

  private async pushUnlocked(): Promise<PushOutcome> {
    const branch = this.remote?.branch ?? "main"
    const r = await this.git(["push", "--porcelain", "origin", `${LOCAL_REF}:refs/heads/${branch}`], {
      token: await this.token(),
      timeoutMs: NETWORK_TIMEOUT_MS,
    })
    const outcome = classifyPush(r)
    if (outcome.kind === "ok") {
      const head = await this.head()
      if (head) await this.git(["update-ref", REMOTE_REF, head])
    }
    return outcome
  }

  /** Push with bounded fetch/replay retries; leaves a visible pending state on failure. */
  private async publishUnlocked(): Promise<WriteResult> {
    const revision = await this.head()
    if (!this.remote || this.readOnly) return { revision, changed: true, pushed: false, pending: false }
    if (this.offline()) return this.pending("Could not reach the memory server; changes are saved locally and will sync later.")
    let refreshed = false
    for (let attempt = 0; attempt < this.maxPushAttempts; attempt++) {
      const outcome = await this.pushUnlocked()
      if (outcome.kind === "ok") {
        await this.patchState({ lastSyncAt: new Date().toISOString(), lastError: undefined, lastErrorAt: undefined })
        return { revision: await this.head(), changed: true, pushed: true, pending: false }
      }
      if (outcome.kind === "rejected") {
        const fetched = await this.fetchUnlocked()
        if (!fetched.ok) return this.pending(fetched.message ?? "Could not reach the memory server.")
        try {
          await this.integrateUnlocked()
        } catch (error) {
          return this.pending(`Could not combine this change with newer memory on the server: ${errorMessage(error)}`)
        }
        continue
      }
      if (outcome.kind === "auth" && !refreshed && (await this.refreshToken())) {
        refreshed = true
        attempt--
        continue
      }
      if (outcome.kind === "offline") this.offlineAt = Date.now()
      return this.pending(outcome.message)
    }
    return this.pending("The memory server kept changing while saving; gizzi will retry next session.")
  }

  private async pending(message: string): Promise<WriteResult> {
    await this.recordError(message)
    return { revision: await this.head(), changed: true, pushed: false, pending: true, error: message }
  }

  private async syncUnlocked(source?: string): Promise<void> {
    await this.commitWorkingTreeUnlocked(source)
    if (!this.remote) return
    const fetched = await this.fetchUnlocked()
    if (!fetched.ok) {
      await this.recordError(fetched.message ?? "Could not reach the memory server.")
      return
    }
    try {
      await this.integrateUnlocked()
    } catch (error) {
      await this.recordError(`Could not combine local memory with the server copy: ${errorMessage(error)}`)
      return
    }
    if (this.readOnly) {
      await this.patchState({ lastSyncAt: new Date().toISOString(), lastError: undefined, lastErrorAt: undefined })
      return
    }
    const head = await this.head()
    const remote = await this.rev(REMOTE_REF)
    if (head && head !== remote) await this.publishUnlocked()
    else await this.patchState({ lastSyncAt: new Date().toISOString(), lastError: undefined, lastErrorAt: undefined })
  }

  /**
   * First sign-in: merge the signed-out local drive's history into this
   * account's drive (real merge commit, both histories kept), then push.
   * Never resets or force-pushes. No-op once the local history is contained.
   */
  mergeFrom(localDir: string): Promise<{ merged: boolean; reason?: string; result?: WriteResult }> {
    return withDriveLock(this.dir, async () => {
      if (this.readOnly) throw new DriveReadOnlyError()
      await this.ensureUnlocked()
      await this.syncUnlocked()
      if (!existsSync(path.join(localDir, ".git"))) return { merged: false, reason: "no signed-out drive" }
      const fetched = await this.git(["fetch", "--quiet", "--no-tags", localDir, `+${LOCAL_REF}:refs/allternit/signed-out`])
      if (fetched.code !== 0) return { merged: false, reason: "signed-out drive has no history" }
      const local = await this.rev("refs/allternit/signed-out")
      const head = await this.head()
      if (!local || !head) return { merged: false, reason: "nothing to merge" }
      if (await this.isAncestor(local, head)) return { merged: false, reason: "already merged" }
      const base = await this.mergeBase(local, head)
      const baseFiles = base ? (await this.readTree(base)).files : {}
      const ours = await this.readTree(head)
      const operations = diffOperations(baseFiles, (await this.readTree(local)).files)
      const next = applyOperations(ours.files, operations)
      const commit = await this.commitFiles(next, [head, local], "Merge memory saved while signed out", ours.oids, ours.files)
      await this.materialize(commit, next, head)
      await this.patchState({ mergedSignedOutFrom: local })
      const result = await this.publishUnlocked()
      return { merged: true, result }
    })
  }
}

export { DriveFormatError, DriveSecretError }
