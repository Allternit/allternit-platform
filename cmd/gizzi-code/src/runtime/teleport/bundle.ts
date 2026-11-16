/**
 * Portable workspace bundle: what a Teleport (or any workspace move) sends
 * from this computer, with secrets left out (Files drive plan step 18, D15).
 *
 * The deny-list is the one allternit-api enforces on Files paths
 * (`cmd/allternit-api/assets/secrets-deny.json`), so there is one list to
 * edit and it only ever grows. A file is left out when its path contains a
 * secret location (`.ssh/`, `.aws/`, `.docker/config.json`, ...), its name
 * matches a secret file pattern (`.env`, `*.pem`, `token.json`, ...), or its
 * text holds a well-known key or token format.
 *
 * Left-out files are listed for the person, never sent. Free of CLI/UI imports.
 */
import { spawnSync } from "node:child_process"
import { lstatSync, openSync, readSync, closeSync } from "node:fs"
import { join } from "node:path"
import deny from "../../../../allternit-api/assets/secrets-deny.json"

/** Text files up to this size are scanned for key and token formats. */
const SCAN_BYTES = 1024 * 1024

const lower = (s: string) => s.toLowerCase()

const HOME_PATHS: string[][] = (deny.home_paths as string[][]).map((p) => p.map(lower))

const FILE_PATTERNS: RegExp[] = (deny.file_patterns as string[]).map(
  (glob) => new RegExp("^" + glob.split("*").map((part) => part.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$", "i"),
)

const CONTENT_PATTERNS: RegExp[] = (deny.content_patterns as string[]).map((re) => new RegExp(re))

/** Why a path is a secret by name alone, or undefined. `rel` uses `/`. */
export function secretByPath(rel: string): string | undefined {
  const parts = rel.split("/").filter((p) => p && p !== ".").map(lower)
  for (const entry of HOME_PATHS) {
    for (let i = 0; i + entry.length <= parts.length; i++) {
      if (entry.every((c, j) => parts[i + j] === c)) return `inside ${entry.join("/")}`
    }
  }
  const name = parts[parts.length - 1] ?? ""
  if (FILE_PATTERNS.some((re) => re.test(name))) return "secret file name"
  return undefined
}

/** True when `text` holds a private key or a well-known API token format. */
export function holdsSecret(text: string): boolean {
  return CONTENT_PATTERNS.some((re) => re.test(text))
}

function scanFile(abs: string): boolean {
  try {
    const st = lstatSync(abs)
    if (!st.isFile() || st.size === 0) return false
    const n = Math.min(st.size, SCAN_BYTES)
    const buf = Buffer.alloc(n)
    const fd = openSync(abs, "r")
    try {
      readSync(fd, buf, 0, n, 0)
    } finally {
      closeSync(fd)
    }
    // Binary files (a NUL in the first 8 KB) are not scanned.
    if (buf.subarray(0, 8192).includes(0)) return false
    return holdsSecret(buf.toString("utf8"))
  } catch {
    return false
  }
}

export interface LeftOut {
  path: string
  reason: string
}

/** Splits workspace-relative paths into the ones that may travel and the ones left out. */
export function filterPaths(cwd: string, paths: string[]): { send: string[]; leftOut: LeftOut[] } {
  const send: string[] = []
  const leftOut: LeftOut[] = []
  for (const p of paths) {
    const byName = secretByPath(p)
    if (byName) leftOut.push({ path: p, reason: byName })
    else if (scanFile(join(cwd, p))) leftOut.push({ path: p, reason: "holds a key or token" })
    else send.push(p)
  }
  return { send, leftOut }
}

function gitZ(cwd: string, args: string[]): string[] {
  const r = spawnSync("git", ["-C", cwd, ...args], { maxBuffer: 64 * 1024 * 1024 })
  if (r.status !== 0) return []
  return r.stdout.toString("utf8").split("\0").filter(Boolean)
}

export interface WorkspaceListing {
  /** Tracked files whose changes since `base` travel in the patch. */
  changed: string[]
  /** Untracked, not-ignored files that travel in the tar. */
  untracked: string[]
  /** Never sent. */
  leftOut: LeftOut[]
}

/**
 * What a move of `cwd` sends beyond the `base` commit, after the secrets
 * filter. This is the approval listing the person sees (`gizzi teleport --dry-run`).
 */
export function workspaceListing(cwd: string, base: string): WorkspaceListing {
  // Changed tracked files, including deletions (a deletion carries no content).
  const changed = filterPaths(cwd, gitZ(cwd, ["diff", "--no-renames", "--name-only", "-z", base]))
  const untracked = filterPaths(cwd, gitZ(cwd, ["ls-files", "--others", "--exclude-standard", "-z"]))
  return { changed: changed.send, untracked: untracked.send, leftOut: [...changed.leftOut, ...untracked.leftOut] }
}

/** git pathspecs that name exactly these files (no globbing). */
export function literalPathspecs(paths: string[]): string[] {
  return paths.map((p) => `:(literal)${p}`)
}
