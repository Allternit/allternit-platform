/**
 * Session worktrees: the shared core behind the runtime worktree_enter /
 * worktree_exit tools and the TUI worktree tools.
 *
 * - create: `git worktree add` at `<repo>/.claude/worktrees/<name>` on branch
 *   `worktree-<name>` (the TUI's layout, so both surfaces find the same trees),
 *   or reuse a tree already at that path.
 * - A small JSON registry under Global.Path.data records which session created
 *   each tree, and which session is working inside one (so a restart or the
 *   next turn keeps the session there).
 * - status: uncommitted changes and unpushed commits.
 * - remove: refuses a tree another session created, and a tree with
 *   uncommitted or unpushed work unless the caller says to discard it.
 *
 * Every git failure fails closed: a status that can't be read counts as work.
 */
import { execFile } from "child_process"
import { readFileSync } from "fs"
import fs from "fs/promises"
import path from "path"
import { Global } from "@/runtime/context/global"

export namespace SessionWorktree {
  export const MAX_NAME_LENGTH = 64
  const SEGMENT = /^[a-zA-Z0-9._-]+$/
  const GIT_ENV = { GIT_TERMINAL_PROMPT: "0", GIT_ASKPASS: "" }

  export interface Entry {
    path: string
    branch: string
    /** The commit the tree started from (sha). */
    base: string
    /** The main checkout the tree belongs to. */
    repo: string
    /** The session that created the tree; only it may remove it. */
    created_by: string
    created_at: number
  }

  export interface Active {
    path: string
    /** The folder the session worked in before it entered the tree. */
    original: string
    branch?: string
  }

  interface Store {
    trees: Record<string, Entry>
    active: Record<string, Active>
  }

  export interface CreateResult {
    path: string
    branch: string
    base: string
    /** The creating session, from the registry; undefined for a tree made outside it. */
    created_by?: string
    /** True when a tree already at that path was reused. */
    existed: boolean
  }

  export interface Status {
    /** Changed or untracked paths (`git status --porcelain` lines). */
    uncommitted: number
    /** Commits not on the upstream, or since the base when there is none. */
    unpushed: number
    /** Set when git failed; the counts are then not to be trusted (treat as work). */
    error?: string
  }

  export type RemoveRefusal = "not_created_here" | "uncommitted_changes" | "unpushed_work" | "not_a_worktree" | "git_failed"
  /** removed: true carries the branch; removed: false carries the reason (and the status read, if any). */
  export interface RemoveResult {
    removed: boolean
    path: string
    branch?: string
    reason?: RemoveRefusal
    detail?: string
    status?: Status
  }

  // ---------------------------------------------------------------------------
  // registry
  // ---------------------------------------------------------------------------

  let fileOverride: string | undefined
  let cache: Store | undefined
  let writeQueue: Promise<void> = Promise.resolve()

  function registryFile() {
    return fileOverride ?? path.join(Global.Path.data, "session-worktrees.json")
  }

  /** Tests: point the registry at a scratch file and drop the in-memory copy. */
  export function setRegistryFileForTesting(file: string | undefined) {
    fileOverride = file
    cache = undefined
  }

  function parse(raw: string | undefined): Store {
    const store: Store = { trees: {}, active: {} }
    if (!raw) return store
    try {
      const json = JSON.parse(raw) as Partial<Store>
      if (json.trees && typeof json.trees === "object") store.trees = json.trees
      if (json.active && typeof json.active === "object") store.active = json.active
    } catch {
      // A broken file starts empty; trees then count as made elsewhere, so nothing is removed by mistake.
    }
    return store
  }

  async function load(): Promise<Store> {
    if (cache) return cache
    const raw = await fs.readFile(registryFile(), "utf8").catch(() => undefined)
    cache ??= parse(raw)
    return cache
  }

  function loadSync(): Store {
    if (cache) return cache
    let raw: string | undefined
    try {
      raw = readFileSync(registryFile(), "utf8")
    } catch {}
    cache = parse(raw)
    return cache
  }

  /**
   * The tree a session works in, for `Session.Info.worktree`. Synchronous
   * (session rows are mapped synchronously); reads the cached registry.
   */
  export function infoFor(sessionID: string): { path: string; branch?: string } | undefined {
    const store = loadSync()
    const current = store.active[sessionID]
    if (!current) return undefined
    return { path: current.path, branch: current.branch ?? store.trees[current.path]?.branch }
  }

  function save(store: Store): Promise<void> {
    const file = registryFile()
    const body = JSON.stringify(store, null, 2)
    writeQueue = writeQueue
      .catch(() => {})
      .then(async () => {
        await fs.mkdir(path.dirname(file), { recursive: true })
        const tmp = `${file}.${process.pid}.tmp`
        await fs.writeFile(tmp, body)
        await fs.rename(tmp, file)
      })
    return writeQueue
  }

  async function canonical(p: string) {
    return fs.realpath(p).catch(() => path.resolve(p))
  }

  /** The registry entry for a tree, if this runtime created it. */
  export async function owner(treePath: string): Promise<Entry | undefined> {
    const store = await load()
    return store.trees[await canonical(treePath)]
  }

  // ---------------------------------------------------------------------------
  // git
  // ---------------------------------------------------------------------------

  function git(cwd: string, args: string[]): Promise<{ code: number; stdout: string; stderr: string }> {
    return new Promise((resolve) => {
      execFile(
        "git",
        args,
        { cwd, env: { ...process.env, ...GIT_ENV }, maxBuffer: 16 * 1024 * 1024 },
        (error, stdout, stderr) => {
          const code = error ? (typeof (error as any).code === "number" ? (error as any).code : 1) : 0
          resolve({ code, stdout: String(stdout), stderr: String(stderr) })
        },
      )
    })
  }

  /** Throws when the name would escape the trees folder or isn't a safe branch segment. */
  export function validateName(name: string) {
    if (name.length > MAX_NAME_LENGTH) throw new Error(`Invalid worktree name: at most ${MAX_NAME_LENGTH} characters`)
    for (const segment of name.split("/")) {
      if (segment === "." || segment === "..") throw new Error(`Invalid worktree name "${name}": no "." or ".." parts`)
      if (!SEGMENT.test(segment))
        throw new Error(`Invalid worktree name "${name}": use letters, digits, dots, underscores and dashes`)
    }
  }

  // `a/b` → `a+b` for both the folder and the branch: nesting either one breaks git or nests trees.
  function flatten(name: string) {
    return name.replaceAll("/", "+")
  }

  export function branchName(name: string) {
    return `worktree-${flatten(name)}`
  }

  export function treesDir(repo: string) {
    return path.join(repo, ".claude", "worktrees")
  }

  export function pathFor(repo: string, name: string) {
    return path.join(treesDir(repo), flatten(name))
  }

  /** The main checkout for `cwd`, also when `cwd` is inside a linked worktree. */
  export async function repoRoot(cwd: string): Promise<string | undefined> {
    const common = await git(cwd, ["rev-parse", "--path-format=absolute", "--git-common-dir"])
    if (common.code === 0) {
      const dir = common.stdout.trim()
      if (path.basename(dir) === ".git") return canonical(path.dirname(dir))
    }
    const top = await git(cwd, ["rev-parse", "--show-toplevel"])
    return top.code === 0 ? canonical(top.stdout.trim()) : undefined
  }

  function generatedName() {
    const d = new Date()
    const day = `${d.getFullYear()}${String(d.getMonth() + 1).padStart(2, "0")}${String(d.getDate()).padStart(2, "0")}`
    return `wt-${day}-${Math.random().toString(16).slice(2, 6)}`
  }

  async function headOf(treePath: string): Promise<string | undefined> {
    const head = await git(treePath, ["rev-parse", "HEAD"])
    return head.code === 0 ? head.stdout.trim() : undefined
  }

  /**
   * Creates a worktree (or reuses the one already at its path) and records the
   * creating session. `base` defaults to the current HEAD of `cwd`.
   */
  export async function create(input: {
    cwd: string
    sessionID: string
    name?: string
    branch?: string
    base?: string
  }): Promise<CreateResult> {
    const name = input.name ?? generatedName()
    validateName(name)
    const repo = await repoRoot(input.cwd)
    if (!repo) throw new Error(`${input.cwd} is not inside a git repository`)
    const treePath = pathFor(repo, name)
    const branch = input.branch ?? branchName(name)
    const store = await load()

    // Reuse: a tree already at that path keeps its original creator.
    const existingHead = await headOf(treePath).catch(() => undefined)
    if (existingHead && (await fs.stat(path.join(treePath, ".git")).catch(() => undefined))) {
      const key = await canonical(treePath)
      const entry = store.trees[key]
      const current = await git(treePath, ["rev-parse", "--abbrev-ref", "HEAD"])
      return {
        path: key,
        branch: current.code === 0 ? current.stdout.trim() : (entry?.branch ?? branch),
        base: entry?.base ?? existingHead,
        created_by: entry?.created_by,
        existed: true,
      }
    }

    const baseRef = input.base ?? "HEAD"
    const resolved = await git(input.cwd, ["rev-parse", "--verify", `${baseRef}^{commit}`])
    if (resolved.code !== 0) throw new Error(`Unknown base "${baseRef}": ${resolved.stderr.trim()}`)
    const base = resolved.stdout.trim()

    await fs.mkdir(treesDir(repo), { recursive: true })
    // -B resets a branch left behind by a tree whose folder was deleted.
    const added = await git(repo, ["worktree", "add", "-B", branch, treePath, base])
    if (added.code !== 0) throw new Error(`git worktree add failed: ${added.stderr.trim()}`)

    const key = await canonical(treePath)
    store.trees[key] = { path: key, branch, base, repo, created_by: input.sessionID, created_at: Date.now() }
    await save(store)
    return { path: key, branch, base, created_by: input.sessionID, existed: false }
  }

  /** Uncommitted changes and unpushed commits in a tree. */
  export async function status(treePath: string, base?: string): Promise<Status> {
    const porcelain = await git(treePath, ["status", "--porcelain"])
    if (porcelain.code !== 0) return { uncommitted: 0, unpushed: 0, error: porcelain.stderr.trim() || "git status failed" }
    const uncommitted = porcelain.stdout.split("\n").filter((line) => line.trim()).length

    const recordedBase = base ?? (await owner(treePath))?.base
    const upstream = await git(treePath, ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
    const range =
      upstream.code === 0
        ? ["rev-list", "--count", "@{u}..HEAD"]
        : recordedBase
          ? ["rev-list", "--count", `${recordedBase}..HEAD`]
          : ["rev-list", "--count", "HEAD", "--not", "--remotes"]
    const counted = await git(treePath, range)
    if (counted.code !== 0) return { uncommitted, unpushed: 0, error: counted.stderr.trim() || "git rev-list failed" }
    return { uncommitted, unpushed: parseInt(counted.stdout.trim(), 10) || 0 }
  }

  /**
   * Removes a tree and its branch. Refuses a tree another session created (or
   * one the registry doesn't know), and one with uncommitted or unpushed work
   * unless `discard` is true. `discard` must only be set when the person chose it.
   */
  export async function remove(input: { path: string; sessionID: string; discard?: boolean }): Promise<RemoveResult> {
    const key = await canonical(input.path)
    const store = await load()
    const entry = store.trees[key]
    if (!entry || entry.created_by !== input.sessionID) {
      return { removed: false, path: key, reason: "not_created_here" }
    }
    if (!(await fs.stat(key).catch(() => undefined))) {
      delete store.trees[key]
      await save(store)
      return { removed: false, path: key, reason: "not_a_worktree", detail: "the folder no longer exists" }
    }
    if (!input.discard) {
      const s = await status(key, entry.base)
      if (s.error) return { removed: false, path: key, reason: "uncommitted_changes", detail: s.error, status: s }
      if (s.uncommitted > 0) return { removed: false, path: key, reason: "uncommitted_changes", status: s }
      if (s.unpushed > 0) return { removed: false, path: key, reason: "unpushed_work", status: s }
    }
    const args = ["worktree", "remove", ...(input.discard ? ["--force"] : []), key]
    const removed = await git(entry.repo, args)
    if (removed.code !== 0) return { removed: false, path: key, reason: "git_failed", detail: removed.stderr.trim() }
    // The branch has no work left to lose here (or the person chose to discard it).
    await git(entry.repo, ["branch", "-D", entry.branch])
    delete store.trees[key]
    for (const [sessionID, active] of Object.entries(store.active)) {
      if (active.path === key) delete store.active[sessionID]
    }
    await save(store)
    return { removed: true, path: key, branch: entry.branch }
  }

  // ---------------------------------------------------------------------------
  // which tree a session works in
  // ---------------------------------------------------------------------------

  /** Records that a session now works inside `treePath`, coming from `original`. */
  export async function enter(sessionID: string, treePath: string, original: string, branch?: string) {
    const store = await load()
    store.active[sessionID] = { path: await canonical(treePath), original, ...(branch ? { branch } : {}) }
    await save(store)
  }

  /** The tree a session works in, if any. A tree whose folder is gone is forgotten. */
  export async function active(sessionID: string): Promise<Active | undefined> {
    const store = await load()
    const current = store.active[sessionID]
    if (!current) return undefined
    if (!(await fs.stat(current.path).catch(() => undefined))) {
      delete store.active[sessionID]
      await save(store)
      return undefined
    }
    return current
  }

  /** Forgets the session's tree and returns where it came from. */
  export async function leave(sessionID: string): Promise<Active | undefined> {
    const store = await load()
    const current = store.active[sessionID]
    if (!current) return undefined
    delete store.active[sessionID]
    await save(store)
    return current
  }
}
