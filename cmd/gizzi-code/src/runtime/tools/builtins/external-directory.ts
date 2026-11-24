import fs, { type Stats } from "fs"
import os from "os"
import path from "path"
import type { Tool } from "@/runtime/tools/builtins/tool"
import { Instance } from "@/runtime/context/project/instance"
import { SessionSandbox } from "@/runtime/context/sandbox/session-sandbox"
import { Sandbox } from "@/runtime/integrations/shell/sandbox"
import { Database, eq } from "@/runtime/session/storage/db"
import { SessionTable } from "@/runtime/session/session.sql"

type Kind = "file" | "directory"

type Options = {
  bypass?: boolean
  kind?: Kind
}

export async function assertExternalDirectory(ctx: Tool.Context, target?: string, options?: Options) {
  if (!target) return

  if (options?.bypass) return

  if (Instance.containsPath(target)) return
  // A folder the person granted with folder_access for this session.
  if (FolderGrants.find(ctx.sessionID, target)) return

  const kind = options?.kind ?? "file"
  const parentDir = kind === "directory" ? target : path.dirname(target)
  const glob = path.join(parentDir, "*")

  await ctx.ask({
    permission: "external_directory",
    patterns: [glob],
    always: [glob],
    metadata: {
      filepath: target,
      parentDir,
    },
  })
}

/**
 * Hard rejection for file-write tools (write/edit/multiedit/apply_patch) when
 * the session has sandboxing enabled -- these tools call Filesystem/fs APIs
 * directly and never go through bwrap/Seatbelt the way the Bash tool does, so
 * this is the only enforcement they get. It's an application-layer path
 * check, not kernel isolation: still worth having, but it doesn't stop a
 * write issued through some other unsandboxed code path the way bwrap's
 * write-bind allowlist stops the Bash tool.
 */
export function assertSandboxWriteAllowed(ctx: Tool.Context, target?: string) {
  if (!target) return
  const state = SessionSandbox.ensureDefault(ctx.sessionID, [Instance.workdir])
  if (!state?.enabled) return
  if (Sandbox.isWriteAllowed(target, Instance.workdir, state.policy)) return
  // A folder granted with folder_access (by this session or its root session).
  if (FolderGrants.find(ctx.sessionID, target)) return
  throw new Error(
    `Sandbox policy blocked this write: ${target} is outside the allowed write paths for this session.`,
  )
}

/**
 * Session folder grants (Wave C decision 10; the `folder_access` tool).
 *
 * The person grants a folder outside the project for one session: the
 * external_directory ask is skipped inside it, and the session's sandbox may
 * write there. A subagent shares its root session's grants. Grants are held
 * in memory only and die with the process.
 *
 * Deletes in a granted folder move the file to `<root>/_to_delete/<relative
 * path>` instead, unless the person also approved `request_delete` for that
 * folder. The move never follows a symlink out of the root: a target whose
 * parent resolves outside the root, or a `_to_delete` (or a folder inside it)
 * that is a symlink, is refused and nothing moves. Deletes inside the project
 * worktree are not affected.
 */
export namespace FolderGrants {
  export const TRASH_DIR = "_to_delete"

  export interface Grant {
    /** Real path of the granted folder. */
    root: string
    /** request_delete was approved: deletes in this folder are real. */
    delete: boolean
    reason?: string
    at: number
  }

  const sessions = new Map<string, Map<string, Grant>>()
  const roots = new Map<string, string>()

  /** The session a person types in: a subagent's session walks up to its root. */
  export function rootSession(sessionID: string): string {
    const hit = roots.get(sessionID)
    if (hit) return hit
    let current = sessionID
    try {
      for (let depth = 0; depth < 6; depth++) {
        const id = current
        const row = Database.use((db) =>
          db.select({ parent: SessionTable.parent_id }).from(SessionTable).where(eq(SessionTable.id, id)).get(),
        )
        if (!row?.parent) break
        current = row.parent
      }
    } catch {
      return sessionID
    }
    roots.set(sessionID, current)
    if (roots.size > 2000) roots.delete(roots.keys().next().value!)
    return current
  }

  /** `~` and relative paths (against `base`) to an absolute path. */
  export function absolute(p: string, base: string): string {
    if (p === "~") return os.homedir()
    if (p.startsWith("~/")) return path.join(os.homedir(), p.slice(2))
    return path.resolve(base, p)
  }

  /** Real path of the nearest existing ancestor, with the missing tail appended. */
  function realExisting(p: string): string {
    const tail: string[] = []
    let current = p
    for (;;) {
      try {
        return path.join(fs.realpathSync(current), ...tail)
      } catch {
        const up = path.dirname(current)
        if (up === current) return p
        tail.unshift(path.basename(current))
        current = up
      }
    }
  }

  /**
   * Where `target` really lives: its parent resolved through symlinks, the
   * last component kept as is (deleting a symlink removes the link, not what
   * it points to). The target itself need not exist.
   */
  export function locate(target: string): string {
    const abs = path.resolve(target)
    const parent = path.dirname(abs)
    if (parent === abs) return realExisting(abs)
    return path.join(realExisting(parent), path.basename(abs))
  }

  export function within(root: string, p: string): boolean {
    if (p === root) return true
    const prefix = root.endsWith(path.sep) ? root : root + path.sep
    return p.startsWith(prefix)
  }

  /** Grant a folder (already a real path) for the session. request_delete adds to an existing grant. */
  export function grant(sessionID: string, root: string, options: { delete?: boolean; reason?: string } = {}): Grant {
    const key = rootSession(sessionID)
    const map = sessions.get(key) ?? new Map<string, Grant>()
    const prior = map.get(root)
    const next: Grant = {
      root,
      delete: (prior?.delete ?? false) || options.delete === true,
      reason: options.reason ?? prior?.reason,
      at: Date.now(),
    }
    map.set(root, next)
    sessions.set(key, map)
    if (sessions.size > 500) sessions.delete(sessions.keys().next().value!)
    return next
  }

  export function list(sessionID: string): Grant[] {
    return [...(sessions.get(rootSession(sessionID))?.values() ?? [])]
  }

  export function clear(sessionID: string) {
    sessions.delete(rootSession(sessionID))
    sessions.delete(sessionID)
  }

  /** The most specific grant covering `target`, if any. */
  export function find(sessionID: string, target: string): Grant | undefined {
    const grants = list(sessionID)
    if (!grants.length) return undefined
    const where = locate(target)
    let best: Grant | undefined
    for (const g of grants) {
      if (within(g.root, where) && (!best || g.root.length > best.root.length)) best = g
    }
    return best
  }

  export type Disposition =
    | { kind: "project" }
    | { kind: "outside" }
    | { kind: "delete"; grant: Grant }
    | { kind: "trash"; grant: Grant }

  /** What a delete of `target` does: unchanged (project, ungranted, request_delete) or moved to `_to_delete`. */
  export function disposition(sessionID: string, target: string): Disposition {
    if (Instance.containsPath(path.dirname(path.resolve(target))) && Instance.containsPath(target)) return { kind: "project" }
    const g = find(sessionID, target)
    if (!g) return { kind: "outside" }
    return g.delete ? { kind: "delete", grant: g } : { kind: "trash", grant: g }
  }

  export class TrashRefused extends Error {
    constructor(message: string) {
      super(message)
      this.name = "TrashRefused"
    }
  }

  export interface TrashPlan {
    root: string
    /** The entry to move (its parent resolved, inside root). */
    from: string
    /** Path relative to root, kept under `_to_delete`. */
    relative: string
  }

  /** Check a move to `<root>/_to_delete/` without touching anything. Throws TrashRefused. */
  export function planTrash(root: string, target: string): TrashPlan {
    const realRoot = fs.realpathSync(root)
    const from = locate(target)
    if (!within(realRoot, from)) {
      throw new TrashRefused(`${target} resolves to ${from}, outside ${realRoot} (a symlink leads out of the folder); nothing was moved.`)
    }
    if (from === realRoot) throw new TrashRefused(`${target} is the granted folder itself; it can't be deleted.`)
    const relative = path.relative(realRoot, from)
    if (relative === TRASH_DIR || relative.startsWith(TRASH_DIR + path.sep)) {
      throw new TrashRefused(
        `${target} is already in ${TRASH_DIR}. Deleting it for real needs folder_access with action request_delete for ${realRoot}.`,
      )
    }
    checkTrashDir(realRoot)
    checkTrashChain(realRoot, path.dirname(relative))
    return { root: realRoot, from, relative }
  }

  /** Every folder that already exists on the way to the destination must be a plain folder. */
  function checkTrashChain(realRoot: string, relDir: string) {
    let current = path.join(realRoot, TRASH_DIR)
    for (const part of relDir.split(path.sep).filter((p) => p && p !== ".")) {
      current = path.join(current, part)
      let st: Stats
      try {
        st = fs.lstatSync(current)
      } catch {
        return
      }
      if (st.isSymbolicLink() || !st.isDirectory()) {
        throw new TrashRefused(`${current} is not a plain folder (a symlink or a file), so nothing was moved there.`)
      }
    }
  }

  /** `_to_delete` (when it exists) must be a real folder, not a symlink. */
  function checkTrashDir(realRoot: string) {
    const dir = path.join(realRoot, TRASH_DIR)
    let st: Stats
    try {
      st = fs.lstatSync(dir)
    } catch {
      return
    }
    if (st.isSymbolicLink() || !st.isDirectory()) {
      throw new TrashRefused(`${dir} is not a plain folder (a symlink or a file), so nothing was moved there.`)
    }
  }

  /** Make `<root>/_to_delete/<dir>` one level at a time, refusing any symlink on the way. */
  function makeTrashParent(realRoot: string, relDir: string): string {
    let current = realRoot
    for (const part of [TRASH_DIR, ...relDir.split(path.sep).filter((p) => p && p !== ".")]) {
      current = path.join(current, part)
      let st: Stats | undefined
      try {
        st = fs.lstatSync(current)
      } catch {
        st = undefined
      }
      if (!st) {
        fs.mkdirSync(current)
        continue
      }
      if (st.isSymbolicLink() || !st.isDirectory()) {
        throw new TrashRefused(`${current} is not a plain folder (a symlink or a file), so nothing was moved there.`)
      }
    }
    if (!within(path.join(realRoot, TRASH_DIR), fs.realpathSync(current))) {
      throw new TrashRefused(`${current} resolves outside ${realRoot}; nothing was moved.`)
    }
    return current
  }

  /** Move a planned entry under `_to_delete`, keeping its relative path. Returns where it went. */
  export function applyTrash(plan: TrashPlan): string {
    checkTrashDir(plan.root)
    const parent = makeTrashParent(plan.root, path.dirname(plan.relative))
    const base = path.basename(plan.relative)
    let dest = path.join(parent, base)
    for (let n = 2; exists(dest); n++) {
      const ext = path.extname(base)
      dest = path.join(parent, `${path.basename(base, ext)} (${n})${ext}`)
    }
    fs.renameSync(plan.from, dest)
    return dest
  }

  function exists(p: string): boolean {
    try {
      fs.lstatSync(p)
      return true
    } catch {
      return false
    }
  }

  /** plan + apply in one step. */
  export function moveToTrash(root: string, target: string): string {
    return applyTrash(planTrash(root, target))
  }

  /**
   * Let the session's sandbox write in a granted folder (bash and the file
   * tools check the same policy). Never touches the process-wide default:
   * a session without its own sandbox state gets a copy with the folder added.
   */
  export function allowSandboxWrites(sessionID: string, root: string) {
    const state = SessionSandbox.ensureDefault(sessionID, [Instance.workdir])
    if (!state?.enabled) return
    if (state.policy.allowWritePaths.includes(root)) return
    const own = SessionSandbox.get(sessionID)
    if (own === state) {
      state.policy.allowWritePaths.push(root)
      return
    }
    SessionSandbox.enable(sessionID, { ...state.policy, allowWritePaths: [...state.policy.allowWritePaths, root] })
  }
}
