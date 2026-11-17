/**
 * Deletes in a granted outside folder (Ink side of folder_access rule 10).
 *
 * In a folder the person granted with `folder_access request`, a delete moves
 * the file to `<folder>/_to_delete/` unless they also approved
 * `folder_access request_delete` for that folder. Deletes inside the project
 * and in folders that were never granted are untouched here (the normal
 * permission path handles those).
 *
 * A plain `rm` / `rmdir` / `unlink` with literal operands is rewritten into
 * `mkdir -p … && mv …`. Any other command that deletes and names such a folder
 * (pipes, `find -delete`, `git clean`, wrappers, `$(…)`) is refused with a
 * message, since its targets cannot be moved reliably.
 *
 * Same rules as the runtime (lane C, runtime/tools/builtins/external-directory.ts
 * `FolderGrants`): delete detection is the runtime floor's `Catastrophic.deletes`,
 * and each move is checked by `FolderGrants.planTrash` (symlink escape, the
 * granted folder itself, entries already in `_to_delete`, a `_to_delete` that
 * is not a plain folder). The entry keeps its path inside the folder, and a
 * name clash gets " (2)", " (3)", … like `FolderGrants.applyTrash`.
 */
import { lstatSync, realpathSync, statSync } from 'fs'
import { basename, dirname, extname, join, resolve } from 'path'
import { homedir } from 'os'
import { Catastrophic } from '@/runtime/tools/guard/permission/catastrophic'
import { FolderGrants } from '@/runtime/tools/builtins/external-directory'
import { pathInWorkingPath } from '../../utils/permissions/pathInWorkingPath.js'
import { isDeleteGranted } from './folderGrants.js'

export type DeleteRedirect =
  | { kind: 'rewrite'; command: string; moved: string[] }
  | { kind: 'refuse'; message: string }

export interface RedirectContext {
  /** Directory the command runs in. */
  cwd: string
  /** The project root: deletes inside it are unchanged. */
  projectRoot: string
  /** Granted folders outside the project (session working directories). */
  grantedFolders: string[]
}

const SIMPLE_DELETE = new Set(['rm', 'rmdir', 'unlink'])
const SHELL_META = /[;&|<>`\n]|\$\(/

/** Quote-aware split of a simple command. Undefined when it has shell syntax we don't rewrite. */
function splitSimple(command: string): string[] | undefined {
  const words: string[] = []
  let word = ''
  let inWord = false
  let quote: string | undefined
  for (let i = 0; i < command.length; i++) {
    const c = command[i]
    if (quote) {
      if (c === quote) quote = undefined
      else if (c === '\\' && quote === '"' && i + 1 < command.length) word += command[++i]
      else word += c
      continue
    }
    if (c === "'" || c === '"') {
      quote = c
      inWord = true
      continue
    }
    if (c === '\\' && i + 1 < command.length) {
      word += command[++i]
      inWord = true
      continue
    }
    if (/\s/.test(c)) {
      if (inWord) words.push(word)
      word = ''
      inWord = false
      continue
    }
    if (SHELL_META.test(c) || (c === '$' && command[i + 1] === '(')) return undefined
    word += c
    inWord = true
  }
  if (quote) return undefined
  if (inWord) words.push(word)
  return words
}

function expandHome(p: string): string {
  if (p === '~') return homedir()
  if (p.startsWith('~/')) return homedir() + p.slice(1)
  return p
}

function quote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`
}

function hasGlob(s: string): boolean {
  return /[*?[]/.test(s)
}

function real(p: string): string {
  try {
    return realpathSync(p)
  } catch {
    return p
  }
}

function inProject(abs: string, ctx: RedirectContext): boolean {
  return pathInWorkingPath(abs, ctx.projectRoot) || pathInWorkingPath(abs, real(ctx.projectRoot))
}

/** The granted outside folder holding `abs` (longest match, real paths), if any. */
function grantedRoot(abs: string, ctx: RedirectContext): string | undefined {
  let best: string | undefined
  for (const folder of ctx.grantedFolders) {
    const root = real(folder)
    if (pathInWorkingPath(abs, root) && (!best || root.length > best.length)) best = root
  }
  return best
}

/**
 * The granted folder a delete of `abs` must move into, if any: the entry is
 * located like the runtime does (parent resolved through symlinks), so a link
 * inside the project that leads into a granted folder is caught too.
 */
function mustMove(abs: string, ctx: RedirectContext): string | undefined {
  const located = FolderGrants.locate(abs)
  const root = grantedRoot(located, ctx)
  if (!root) return undefined
  if (inProject(located, ctx)) return undefined
  if (isDeleteGranted(located) || isDeleteGranted(abs)) return undefined
  return root
}

function exists(p: string): boolean {
  try {
    lstatSync(p)
    return true
  } catch {
    return false
  }
}

function refuse(root: string, why: string): DeleteRedirect {
  return {
    kind: 'refuse',
    message:
      `Not run: ${why}. Deletes in ${root} move to ${root}/${FolderGrants.TRASH_DIR}/ because the user has not approved ` +
      `folder_access request_delete for it. Use a plain rm with each path (it is moved for you), ` +
      `or ask with folder_access action "request_delete".`,
  }
}

/**
 * Decide what a shell command that may delete becomes. Undefined = run as is.
 */
export function redirectDeletes(command: string, ctx: RedirectContext): DeleteRedirect | undefined {
  const outside = ctx.grantedFolders.filter(f => !inProject(f, ctx) && !isDeleteGranted(f))
  if (!outside.length) return undefined
  if (!Catastrophic.deletes(command)) return undefined

  const words = splitSimple(command.trim())
  const prog = words?.length ? basename(words[0]) : undefined
  if (!words || !prog || !SIMPLE_DELETE.has(prog)) {
    // Not a plain rm: refuse only when it can reach a folder that must keep its files.
    const cwdRoot = mustMove(resolve(ctx.cwd, '.__probe__'), ctx)
    const named = outside.find(f => command.includes(f))
    const root = named ?? cwdRoot
    if (!root) return undefined
    return refuse(root, 'this command deletes in a way that cannot be moved')
  }

  const flags: string[] = []
  const operands: string[] = []
  let endOfFlags = false
  for (const w of words.slice(1)) {
    if (!endOfFlags && w === '--') {
      endOfFlags = true
      continue
    }
    if (!endOfFlags && w.startsWith('-') && w !== '-') flags.push(w)
    else operands.push(w)
  }
  if (!operands.length) return undefined
  const force = flags.some(f => /^-[a-zA-Z]*f/.test(f) || f === '--force')

  const keep: string[] = []
  const steps: string[] = []
  const moved: string[] = []
  const taken = new Set<string>()
  for (const raw of operands) {
    if (hasGlob(raw)) {
      // A glob can't be checked entry by entry before the shell expands it.
      const abs = resolve(ctx.cwd, expandHome(raw))
      const root = mustMove(dirname(abs) + '/x', ctx)
      if (root) return refuse(root, `${raw} is a pattern; name each file`)
      keep.push(raw)
      continue
    }
    const abs = resolve(ctx.cwd, expandHome(raw))
    const root = mustMove(abs, ctx)
    if (!root) {
      keep.push(raw)
      continue
    }
    if (!exists(abs)) {
      if (force) continue // rm -f of a missing file: nothing to move
      return refuse(root, `${raw} does not exist`)
    }
    const recursive = flags.some(f => /^-[a-zA-Z]*[rR]/.test(f) || f === '--recursive')
    let isDir = false
    try {
      isDir = statSync(abs).isDirectory()
    } catch {}
    if (prog === 'rm' && isDir && !recursive && !flags.some(f => /^-[a-zA-Z]*d/.test(f) || f === '--dir')) {
      return refuse(root, `${raw} is a folder (rm needs -r)`)
    }
    let plan
    try {
      plan = FolderGrants.planTrash(root, abs)
    } catch (e) {
      return { kind: 'refuse', message: (e as Error).message }
    }
    const parent = join(plan.root, FolderGrants.TRASH_DIR, dirname(plan.relative))
    const base = basename(plan.relative)
    let dest = join(parent, base)
    for (let n = 2; exists(dest) || taken.has(dest); n++) {
      const ext = extname(base)
      dest = join(parent, `${basename(base, ext)} (${n})${ext}`)
    }
    taken.add(dest)
    steps.push(`mkdir -p ${quote(parent)} && mv -- ${quote(plan.from)} ${quote(dest)}`)
    moved.push(abs)
  }
  if (!moved.length && keep.length === operands.length) return undefined
  if (keep.length) steps.push([words[0], ...flags, '--', ...keep.map(k => (hasGlob(k) ? k : quote(k)))].join(' '))
  if (!steps.length) return { kind: 'rewrite', command: 'true', moved }
  return { kind: 'rewrite', command: steps.join(' && '), moved }
}
