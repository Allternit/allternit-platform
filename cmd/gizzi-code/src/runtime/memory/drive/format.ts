/**
 * Memory Drive file format (Agent Memory Repo SPEC.md, MIT —
 * https://github.com/AgentMemoryRepo/agentmemoryrepo/blob/main/SPEC.md),
 * with the Allternit extensions in docs/MEMORY_DRIVE_CONTRACT.md.
 *
 * This is a line-for-line port of the Rust core in
 * cmd/allternit-api/src/memory_drive.rs (Entry::render/parse, validate_*,
 * rebuild_index, remove_entry, apply_batch) so a tree gizzi commits is the
 * tree the server accepts on push. The server's push gate stays the final
 * boundary; this module only makes gizzi fail early, locally, with the same
 * plain reason.
 *
 * Pure: no git, no fs, no network.
 */
import { createHash } from "crypto"
import { scanDriveSecrets } from "./secrets"

export const ENTRYPOINT = "MEMORY.md"
export const MAX_FILE_BYTES = 64 * 1024
export const MAX_DRIVE_BYTES = 2 * 1024 * 1024
export const MAX_FILES = 256
export const MAX_INDEX_LINES = 200
export const MAX_INDEX_BYTES = 16 * 1024
export const EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
export const IMPORTED_UNKNOWN = "imported:unknown"
export const SEED_MEMORY = "# Memory\n\n## Index\n"
/** Folders the server manages (twin projection, cowork mirror): read-only to every writer. */
export const MANAGED_FOLDERS = ["twin/", "cowork/"]

export function isManagedPath(path: string): boolean {
  return MANAGED_FOLDERS.some((f) => path.startsWith(f))
}

/** Throw when `next` changes any file under a managed folder relative to `original`. */
export function assertManagedUnchanged(original: DriveFiles, next: DriveFiles): void {
  for (const p of new Set([...Object.keys(original), ...Object.keys(next)])) {
    if (isManagedPath(p) && original[p] !== next[p]) {
      throw new DriveFormatError(p, "twin/ and cowork/ are managed by Allternit and read-only")
    }
  }
}

/** A file or entry failed the format/path rules. `message` is a plain sentence. */
export class DriveFormatError extends Error {
  constructor(
    readonly field: string,
    readonly reason: string,
  ) {
    super(`${field}: ${reason}`)
    this.name = "DriveFormatError"
  }
}

/** The candidate contains something that looks like a credential. Never echoes the match. */
export class DriveSecretError extends Error {
  constructor(readonly where?: string) {
    super(
      where
        ? `${where} looks like it contains a credential, so it was not saved.`
        : "That memory looks like it contains a credential, so it was not saved.",
    )
    this.name = "DriveSecretError"
  }
}

export interface Entry {
  id: string
  text: string
  /** A session link, an external `scheme:label`, an https URL or `imported:unknown`. */
  source: string
  /** YYYY-MM-DD */
  added: string
  metadata: Record<string, string>
}

/** Server-compatible operation shape (serde externally tagged, see the contract). */
export type Operation =
  | { SetFile: { path: string; content: string } }
  | { DeleteFile: { path: string } }
  | { UpsertEntry: { path: string; entry: Entry } }
  | { DeleteEntry: { id: string } }

/** Path → content of every markdown file in a drive. */
export type DriveFiles = Record<string, string>

const invalid = (field: string, reason: string) => new DriveFormatError(field, reason)

// ── helpers ──────────────────────────────────────────────────────────────────

/** Rust `str::lines()`: split on \n, drop one trailing empty piece, strip a trailing \r. */
export function rustLines(content: string): string[] {
  if (content === "") return []
  const parts = content.split("\n")
  if (parts[parts.length - 1] === "") parts.pop()
  return parts.map((l) => (l.endsWith("\r") ? l.slice(0, -1) : l))
}

const bytes = (s: string) => Buffer.byteLength(s, "utf8")

/** Rust `char::is_control` (Unicode Cc). */
const isControl = (c: string) => {
  const code = c.codePointAt(0)!
  return code <= 0x1f || (code >= 0x7f && code <= 0x9f)
}

export function today(date: Date = new Date()): string {
  const y = date.getFullYear().toString().padStart(4, "0")
  const m = (date.getMonth() + 1).toString().padStart(2, "0")
  const d = date.getDate().toString().padStart(2, "0")
  return `${y}-${m}-${d}`
}

/** Deterministic identity for a bullet without an explicit id (matches the Rust core). */
export function deriveEntryId(text: string, source: string, added: string): string {
  return "entry-" + createHash("sha256").update(`${text}\0${source}\0${added}`).digest("hex")
}

export function sortedFiles(files: DriveFiles): string[] {
  // BTreeMap<String,_> order = byte order; for the ASCII-only paths the
  // validator admits this equals JS code-unit order.
  return Object.keys(files).sort()
}

export function sameFiles(a: DriveFiles, b: DriveFiles): boolean {
  const ka = sortedFiles(a)
  const kb = sortedFiles(b)
  return ka.length === kb.length && ka.every((k, i) => k === kb[i] && a[k] === b[k])
}

// ── validators ───────────────────────────────────────────────────────────────

export function validateIdentifier(value: string, field: string): void {
  if (!value || value.length > 128 || !/^[A-Za-z0-9_-]+$/.test(value)) {
    throw invalid(field, "use 1–128 letters, digits, underscores or hyphens")
  }
}

const FORBIDDEN_STEMS = ["log", "logs", "chat", "chats", "session", "sessions", "conversation", "conversations"]

export function validatePath(path: string): void {
  if (bytes(path) > 240 || !path || !path.endsWith(".md")) {
    throw invalid("path", "use a relative markdown path of at most 240 bytes")
  }
  for (const segment of path.split("/")) {
    if (
      !segment ||
      segment.startsWith(".") ||
      segment.startsWith("-") ||
      segment.includes("..") ||
      !/^[A-Za-z0-9_.-]+$/.test(segment)
    ) {
      throw invalid("path", "absolute, hidden, traversal and special paths are forbidden")
    }
    const stem = segment.replace(/(\.md)+$/, "").toLowerCase()
    if (
      stem.includes("transcript") ||
      FORBIDDEN_STEMS.includes(stem) ||
      stem.endsWith("-log") ||
      stem.endsWith("_log") ||
      stem.startsWith("session-") ||
      stem.startsWith("turn-")
    ) {
      throw invalid("path", "store durable notes, not transcripts or logs")
    }
  }
}

function validateMetadataValue(value: string): void {
  if (
    !value ||
    bytes(value) > 2048 ||
    value !== value.trim() ||
    [...value].some((c) => isControl(c) || c === ";" || c === "[" || c === "]" || c === "\\")
  ) {
    throw invalid("metadata", "empty, multiline or injected metadata")
  }
}

const UNSAFE_SCHEMES = new Set(["javascript", "data", "file", "vbscript", "blob"])
const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"])

/**
 * Contract SOURCE rules: `/?session=…` product links, `gizzi:session/<id>`,
 * https URLs (and loopback http), other `<scheme>:<opaque>` labels except
 * unsafe schemes, or exactly `imported:unknown`.
 */
export function validateSource(source: string): void {
  validateMetadataValue(source)
  if (source === IMPORTED_UNKNOWN) return
  const lower = source.toLowerCase()
  if (/\s/.test(source) || ["%00", "%0a", "%0d", "%5c"].some((s) => lower.includes(s))) {
    throw invalid("source", "unsafe URL")
  }
  const reject = () =>
    invalid("source", "use a session link, an https link, a scheme:label source or imported:unknown")
  if (source.startsWith("/") && !source.startsWith("//")) {
    try {
      const parsed = new URL(source, "https://ai.allternit.com/")
      if (parsed.host !== "ai.allternit.com") throw reject()
    } catch {
      throw reject()
    }
    return
  }
  const scheme = /^([A-Za-z][A-Za-z0-9+.-]*):/.exec(source)?.[1]?.toLowerCase()
  if (!scheme) throw reject()
  if (scheme === "https" || scheme === "http") {
    let parsed: URL
    try {
      parsed = new URL(source)
    } catch {
      throw reject()
    }
    if (
      parsed.username ||
      parsed.password ||
      !parsed.host ||
      (scheme === "http" && !LOOPBACK.has(parsed.hostname === "::1" ? "[::1]" : parsed.hostname))
    ) {
      throw invalid("source", "only HTTPS, loopback HTTP or product-relative session links are accepted")
    }
    return
  }
  if (UNSAFE_SCHEMES.has(scheme)) throw reject()
  const rest = source.slice(scheme.length + 1)
  // Opaque labels (gizzi:session/abc, claude-code:session/x) carry no authority.
  if (!rest || rest.startsWith("//") || rest.includes("@")) throw reject()
}

export function validateText(text: string): void {
  if (!text || bytes(text) > 4096 || text !== text.trim() || [...text].some(isControl)) {
    throw invalid("entry text", "use a nonempty single line of at most 4096 bytes")
  }
  if (/ \[[A-Za-z_][A-Za-z0-9_-]*:/.test(text)) throw invalid("entry text", "metadata injection")
  validateMarkdownLinks(text)
  scanSecrets(text)
}

function validateLabel(label: string, field: string): void {
  if (!label || bytes(label) > 240 || label !== label.trim() || [...label].some((c) => isControl(c) || c === "<" || c === ">")) {
    throw invalid(field, "use a short single-line label")
  }
  scanSecrets(label)
}

export function validateCommitMessage(message: string): void {
  validateLabel(message, "commit message")
}

export function scanSecrets(content: string, where?: string): void {
  if (scanDriveSecrets(content)) throw new DriveSecretError(where)
}

export function validateMarkdownLinks(content: string): void {
  if (content.includes("<") || content.includes(">") || content.includes("![")) {
    throw invalid("markdown", "HTML, images and autolinks are forbidden")
  }
  for (const m of content.matchAll(/\]\(([^)]*)\)/g)) validateSource(m[1]!)
}

function validateDate(added: string): void {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(added)) throw invalid("added", "use YYYY-MM-DD")
  const [y, m, d] = added.split("-").map(Number) as [number, number, number]
  const date = new Date(Date.UTC(y, m - 1, d))
  if (date.getUTCFullYear() !== y || date.getUTCMonth() !== m - 1 || date.getUTCDate() !== d) {
    throw invalid("added", "use YYYY-MM-DD")
  }
}

// ── entries ──────────────────────────────────────────────────────────────────

export function renderEntry(entry: Entry): string {
  validateIdentifier(entry.id, "entry id")
  validateText(entry.text)
  validateSource(entry.source)
  validateDate(entry.added)
  const parts = [`source: ${entry.source}`, `added: ${entry.added}`, `id: ${entry.id}`]
  for (const key of Object.keys(entry.metadata ?? {}).sort()) {
    const value = entry.metadata[key]!
    validateIdentifier(key, "metadata key")
    if (key === "id" || key === "source" || key === "added") throw invalid("metadata key", "reserved key")
    validateMetadataValue(value)
    parts.push(`${key}: ${value}`)
  }
  const line = `- ${entry.text} [${parts.join("; ")}]`
  scanSecrets(line)
  return line
}

export function parseEntry(line: string): Entry {
  if (!line.startsWith("- ")) throw invalid("entry", "use a one-line bullet")
  const body = line.slice(2)
  const cut = body.lastIndexOf(" [")
  if (cut < 0) throw invalid("entry", "missing provenance metadata")
  const text = body.slice(0, cut)
  let meta = body.slice(cut + 2)
  if (!meta.endsWith("]")) throw invalid("entry", "unterminated metadata")
  meta = meta.slice(0, -1)
  const fields = new Map<string, string>()
  for (const part of meta.split("; ")) {
    const colon = part.indexOf(": ")
    if (colon < 0) throw invalid("entry", "invalid metadata pair")
    const key = part.slice(0, colon)
    if (fields.has(key)) throw invalid("entry", "duplicate metadata key")
    fields.set(key, part.slice(colon + 2))
  }
  const explicitId = fields.get("id")
  fields.delete("id")
  const source = fields.get("source")
  if (source === undefined) throw invalid("entry", "missing source")
  fields.delete("source")
  const added = fields.get("added")
  if (added === undefined) throw invalid("entry", "missing added")
  fields.delete("added")
  const entry: Entry = {
    id: explicitId ?? deriveEntryId(text, source, added),
    text,
    source,
    added,
    metadata: Object.fromEntries(fields),
  }
  renderEntry(entry)
  return entry
}

/** True for a line the format treats as an entry bullet (not a `- [[link]]`). */
export function isEntryLine(line: string): boolean {
  return line.startsWith("- ") && !line.startsWith("- [[")
}

export interface LocatedEntry {
  path: string
  line: string
  entry: Entry
}

/** Every entry in the tree by id, in path then line order. Throws on a malformed bullet. */
export function collectEntries(files: DriveFiles): Map<string, LocatedEntry> {
  const out = new Map<string, LocatedEntry>()
  for (const path of sortedFiles(files)) {
    let inIndex = false
    for (const line of rustLines(files[path]!)) {
      if (path === ENTRYPOINT && line === "## Index") inIndex = true
      if (inIndex || !isEntryLine(line)) continue
      const entry = parseEntry(line)
      if (!out.has(entry.id)) out.set(entry.id, { path, line, entry })
    }
  }
  return out
}

// ── files and trees ──────────────────────────────────────────────────────────

export function validateFile(path: string, content: string): void {
  validatePath(path)
  if (bytes(content) > MAX_FILE_BYTES) throw invalid(path, "file is larger than 64 KiB")
  scanSecrets(content, path)
  validateMarkdownLinks(content)
  if ([...content].some((c) => isControl(c) && c !== "\n")) throw invalid(path, "control characters are forbidden")
  const lines = rustLines(content)
  if (path === ENTRYPOINT) {
    const title = lines[0] ?? ""
    if (!(title === "# Memory" || title.startsWith("# Memory: ")) || lines.filter((l) => l === "## Index").length !== 1) {
      throw invalid(ENTRYPOINT, "must start with # Memory and contain one ## Index")
    }
    if (lines.length > MAX_INDEX_LINES || bytes(content) > MAX_INDEX_BYTES) {
      throw invalid(ENTRYPOINT, "keep MEMORY.md short (200 lines, 16 KiB)")
    }
  }
  let index = false
  for (const line of lines) {
    if (line === "## Index" && path === ENTRYPOINT) {
      index = true
      continue
    }
    if (!line) continue
    if (line.startsWith("#") && !index) continue
    if (line.startsWith("- [[") && line.endsWith("]]")) {
      const target = line.slice(4, -2)
      if (target.endsWith(".md")) throw invalid("cross-link", "omit .md")
      validatePath(`${target}.md`)
    } else if (index) {
      throw invalid("Index", "only [[topic]] links go under ## Index")
    } else {
      try {
        parseEntry(line)
      } catch (error) {
        if (error instanceof DriveFormatError) throw invalid(path, `${error.reason} (line: ${preview(line)})`)
        throw error
      }
    }
  }
}

function preview(line: string): string {
  return line.length > 80 ? line.slice(0, 77) + "…" : line
}

export function validateFiles(files: DriveFiles): void {
  if (!(ENTRYPOINT in files)) throw invalid("tree", "MEMORY.md is required")
  const keys = sortedFiles(files)
  if (keys.length > MAX_FILES) throw invalid("tree", "a drive holds at most 256 files")
  if (keys.reduce((n, k) => n + bytes(files[k]!), 0) > MAX_DRIVE_BYTES) throw invalid("tree", "a drive holds at most 2 MiB")
  const ids = new Set<string>()
  for (const path of keys) {
    const content = files[path]!
    validateFile(path, content)
    for (const line of rustLines(content).filter(isEntryLine)) {
      const entry = parseEntry(line)
      if (ids.has(entry.id)) throw invalid("entry id", `duplicate id ${entry.id} across files`)
      ids.add(entry.id)
    }
    for (const m of content.matchAll(/\[\[([^[\]]+)\]\]/g)) {
      const target = m[1]!
      if (target.endsWith(".md")) throw invalid("cross-link", "omit .md")
      const filename = `${target}.md`
      validatePath(filename)
      if (!(filename in files)) throw invalid("cross-link", `missing topic ${filename}`)
    }
  }
}

/** Regenerate MEMORY.md's `## Index` from the topic files (Rust rebuild_index). */
export function rebuildIndex(files: DriveFiles): void {
  const memory = files[ENTRYPOINT]
  if (memory === undefined) throw invalid("tree", "MEMORY.md is required")
  const lines = rustLines(memory)
  const index = lines.indexOf("## Index")
  if (index < 0) throw invalid(ENTRYPOINT, "missing Index heading")
  let content = lines.slice(0, index).join("\n").trimEnd()
  content += "\n\n## Index\n"
  for (const filename of sortedFiles(files)) {
    if (filename === ENTRYPOINT) continue
    content += `- [[${filename.replace(/\.md$/, "")}]]\n`
  }
  files[ENTRYPOINT] = content
}

/** Remove (or replace in place, when `replacement.path` holds it) the entry with `id` (Rust remove_entry). */
export function removeEntry(files: DriveFiles, id: string, replacement?: { path: string; line: string }): void {
  for (const path of Object.keys(files)) {
    const updated: string[] = []
    for (const line of rustLines(files[path]!)) {
      if (isEntryLine(line) && parseEntry(line).id === id) {
        if (replacement && replacement.path === path) updated.push(replacement.line)
      } else {
        updated.push(line)
      }
    }
    files[path] = `${updated.join("\n")}\n`
  }
}

function upsertEntry(files: DriveFiles, path: string, entry: Entry): void {
  validatePath(path)
  const line = renderEntry(entry)
  removeEntry(files, entry.id, { path, line })
  let content = files[path] ?? `# ${path.replace(/\.md$/, "")}\n`
  if (!rustLines(content).some((existing) => existing === line)) {
    if (path === ENTRYPOINT) {
      const offset = content.indexOf("\n## Index\n")
      if (offset < 0) throw invalid(ENTRYPOINT, "missing Index heading")
      content = content.slice(0, offset + 1) + `${line}\n\n` + content.slice(offset + 1)
    } else {
      if (!content.endsWith("\n")) content += "\n"
      content += `${line}\n`
    }
  }
  files[path] = content
}

/**
 * gizzi-internal reconciliation ops used when replaying local work onto a
 * newer remote head. Not sent to the server.
 */
export type ReplayOperation =
  | Operation
  /** Create the file with this content only if the target lacks it. */
  | { EnsureFile: { path: string; content: string } }
  /** Delete the file only if the target still holds exactly `base`, or nothing but headings. */
  | { DeleteFileIfUnchanged: { path: string; base: string } }
  /** Replace the leading heading block when the target's block still equals `base`. */
  | { SetHeader: { path: string; base: string; header: string } }

/** Lines before the first entry bullet (headings and blanks). */
export function headerBlock(content: string): string {
  const lines = rustLines(content)
  const first = lines.findIndex(isEntryLine)
  return (first < 0 ? lines : lines.slice(0, first)).join("\n")
}

/**
 * Apply operations to a tree, regenerate the index and validate the whole
 * candidate (format, paths, ids, links, limits, secrets). Mirrors the Rust
 * apply_batch tree computation; the input is never mutated.
 */
export function applyOperations(original: DriveFiles, operations: ReplayOperation[]): DriveFiles {
  const files: DriveFiles = { ...original }
  if (!(ENTRYPOINT in files)) files[ENTRYPOINT] = SEED_MEMORY
  if (operations.length > 1000) throw invalid("batch", "at most 1,000 operations per write")
  for (const op of operations) {
    if ("SetFile" in op) {
      validatePath(op.SetFile.path)
      validateFile(op.SetFile.path, op.SetFile.content)
      files[op.SetFile.path] = op.SetFile.content
    } else if ("DeleteFile" in op) {
      validatePath(op.DeleteFile.path)
      if (op.DeleteFile.path === ENTRYPOINT) throw invalid("path", "MEMORY.md is required")
      delete files[op.DeleteFile.path]
    } else if ("UpsertEntry" in op) {
      upsertEntry(files, op.UpsertEntry.path, op.UpsertEntry.entry)
    } else if ("DeleteEntry" in op) {
      validateIdentifier(op.DeleteEntry.id, "entry id")
      removeEntry(files, op.DeleteEntry.id)
    } else if ("EnsureFile" in op) {
      validatePath(op.EnsureFile.path)
      if (!(op.EnsureFile.path in files)) files[op.EnsureFile.path] = op.EnsureFile.content
    } else if ("DeleteFileIfUnchanged" in op) {
      const { path, base } = op.DeleteFileIfUnchanged
      validatePath(path)
      const current = files[path]
      if (path !== ENTRYPOINT && current !== undefined) {
        if (current === base || !rustLines(current).some(isEntryLine)) delete files[path]
      }
    } else if ("SetHeader" in op) {
      const { path, base, header } = op.SetHeader
      const current = files[path]
      if (current !== undefined && path !== ENTRYPOINT && headerBlock(current) === base) {
        const lines = rustLines(current)
        const first = lines.findIndex(isEntryLine)
        const rest = first < 0 ? [] : lines.slice(first)
        files[path] = `${[...(header ? rustLines(header) : []), ...rest].join("\n")}\n`
      }
    }
  }
  rebuildIndex(files)
  assertManagedUnchanged(original, files)
  validateFiles(files)
  return files
}

/**
 * The stable-id intent that turns `base` into `ours`, replayable on any
 * later head: changed/added entries become upserts, removed ids deletes,
 * plus file creation/removal and heading edits. Used to re-apply local work
 * after a rejected (non-fast-forward) push and for the first-login merge.
 */
export function diffOperations(base: DriveFiles, ours: DriveFiles): ReplayOperation[] {
  const ops: ReplayOperation[] = []
  const baseEntries = collectEntries(base)
  const ourEntries = collectEntries(ours)
  for (const path of sortedFiles(ours)) {
    if (path === ENTRYPOINT || path in base) continue
    ops.push({ EnsureFile: { path, content: `${headerBlock(ours[path]!)}\n` } })
  }
  for (const path of sortedFiles(ours)) {
    if (path === ENTRYPOINT || !(path in base)) continue
    const before = headerBlock(base[path]!)
    const after = headerBlock(ours[path]!)
    if (before !== after) ops.push({ SetHeader: { path, base: before, header: after } })
  }
  for (const [id, mine] of ourEntries) {
    const theirs = baseEntries.get(id)
    if (!theirs || theirs.line !== mine.line || theirs.path !== mine.path) {
      ops.push({ UpsertEntry: { path: mine.path, entry: mine.entry } })
    }
  }
  for (const id of baseEntries.keys()) {
    if (!ourEntries.has(id)) ops.push({ DeleteEntry: { id } })
  }
  for (const path of sortedFiles(base)) {
    if (path === ENTRYPOINT || path in ours) continue
    ops.push({ DeleteFileIfUnchanged: { path, base: base[path]! } })
  }
  return ops
}

/**
 * Make hand- or model-written edits conform before validation:
 * - a bullet with no `[source: …]` metadata gets `defaultSource` and today's date
 * - entry bullets written under MEMORY.md's `## Index` move above it
 * Everything else is left for the validator to judge.
 */
export function normalizeFiles(files: DriveFiles, defaultSource: string, date = today()): DriveFiles {
  const out: DriveFiles = {}
  const fix = (line: string) => {
    if (!isEntryLine(line)) return line
    const trimmed = line.trimEnd()
    if (/ \[source: [^\]]*\]$/.test(trimmed) || / \[[^\]]*; source: [^\]]*\]$/.test(trimmed)) return trimmed
    if (/ \[[A-Za-z_][A-Za-z0-9_-]*: [^\]]*\]$/.test(trimmed)) return trimmed // has metadata; let the parser decide
    const text = trimmed.slice(2).trim()
    if (!text) return line
    return `- ${text} [source: ${defaultSource}; added: ${date}]`
  }
  for (const path of sortedFiles(files)) {
    const lines = rustLines(files[path]!)
    if (path !== ENTRYPOINT) {
      out[path] = `${lines.map(fix).join("\n")}\n`
      continue
    }
    const index = lines.indexOf("## Index")
    if (index < 0) {
      out[path] = `${lines.map(fix).join("\n")}\n`
      continue
    }
    const head = lines.slice(0, index).map(fix)
    const tail = lines.slice(index + 1)
    const moved = tail.filter(isEntryLine).map(fix)
    const kept = tail.filter((l) => !isEntryLine(l))
    while (head.length > 0 && head[head.length - 1] === "") head.pop()
    const pinned = moved.length > 0 ? [...head, ...moved] : head
    out[path] = `${[...pinned, "", "## Index", ...kept].join("\n")}\n`
  }
  return out
}

/** Bounded MEMORY.md text for prompt injection. */
export function boundedEntrypoint(content: string, maxLines = MAX_INDEX_LINES, maxBytes = MAX_INDEX_BYTES): string {
  const lines = rustLines(content)
  let out = lines.slice(0, maxLines).join("\n")
  if (bytes(out) > maxBytes) out = Buffer.from(out, "utf8").subarray(0, maxBytes).toString("utf8")
  const omitted = lines.length > maxLines || bytes(content) > maxBytes
  return omitted ? `${out}\n… (MEMORY.md truncated)` : out
}
