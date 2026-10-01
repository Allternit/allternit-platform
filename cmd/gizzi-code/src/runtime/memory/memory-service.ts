/**
 * MemoryService — Structured memory CRUD for the Gizzi platform
 *
 * Converged on the memdir auto-memory store (the same directory the TUI's
 * memory UX uses, resolved via getAutoMemPathFor in src/memdir/paths):
 * - Frontmatter format: name / description / type
 * - MEMORY.md always-loaded index (< 200 lines), updated line-preservingly
 * - Topic .md files discovered and injected into context
 * - Relevance scoring filters which topic files are loaded per session
 *
 * The allternit-api memory kernel is the canonical store (WP-M1d): saves and
 * deletes are mirrored to it, the memdir's existing files are imported once,
 * and search adds the kernel's hybrid recall to the local match. The files
 * stay the local working copy, so offline / signed-out behaviour is unchanged.
 *
 * Pre-convergence stores (.gizzi/L1-COGNITIVE/memory, .openclaw L1, the old
 * Global.Path.config per-project store) are read-only fallbacks and are
 * copied (never moved) into the memdir by a one-time best-effort import.
 */

import path from "path"
import { copyFile, unlink } from "fs/promises"
import z from "zod/v4"
import { Filesystem } from "@/shared/util/filesystem"
import { Instance } from "@/runtime/context/project/instance"
import { Global } from "@/runtime/context/global"
import { Glob } from "@/shared/util/glob"
import { Log } from "@/shared/util/log"
import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { getAutoMemPathFor } from "@/memdir/paths"
import { MemoryKernelAdapter } from "./kernel-adapter"

const log = Log.create({ service: "memory-service" })

export type MemoryType = "user" | "feedback" | "project" | "reference"

export interface MemoryFrontmatter {
  name: string
  description: string
  type: MemoryType
}

export interface MemoryEntry extends MemoryFrontmatter {
  filename: string   // e.g. "user_role.md"
  filepath: string   // absolute path
  body: string       // content after frontmatter
  mtime?: number
}

export interface MemoryIndexEntry {
  filename: string
  description: string
  type: MemoryType
}

// ── Bus event ────────────────────────────────────────────────────────────────

export namespace MemoryEvent {
  export const Updated = BusEvent.define(
    "memory.updated",
    z.object({ filepath: z.string(), action: z.enum(["save", "delete"]) }),
  )
}

// ── Frontmatter helpers ───────────────────────────────────────────────────────

const FM_OPEN = "---"
const FM_CLOSE = "---"

export function parseFrontmatter(content: string): { fm: Partial<MemoryFrontmatter>; body: string } {
  const lines = content.split("\n")
  if (lines[0]?.trim() !== FM_OPEN) return { fm: {}, body: content }

  const closeIdx = lines.findIndex((l, i) => i > 0 && l.trim() === FM_CLOSE)
  if (closeIdx < 0) return { fm: {}, body: content }

  const fmLines = lines.slice(1, closeIdx)
  const body = lines.slice(closeIdx + 1).join("\n").trimStart()

  const fm: Partial<MemoryFrontmatter> = {}
  for (const line of fmLines) {
    const colon = line.indexOf(":")
    if (colon < 0) continue
    const key = line.slice(0, colon).trim()
    const val = line.slice(colon + 1).trim()
    if (key === "name") fm.name = val
    else if (key === "description") fm.description = val
    else if (key === "type") fm.type = val as MemoryType
  }
  return { fm, body }
}

export function serializeFrontmatter(fm: MemoryFrontmatter, body: string): string {
  return [
    "---",
    `name: ${fm.name}`,
    `description: ${fm.description}`,
    `type: ${fm.type}`,
    "---",
    "",
    body,
  ].join("\n")
}

// ── Path resolution ───────────────────────────────────────────────────────────

function projectHash(directory: string): string {
  return directory.replace(/^\//, "").replace(/\//g, "-")
}

/** Pre-convergence global per-project store (old memory_write target) */
function legacyGlobalMemoryDir(): string {
  return path.join(Global.Path.config, "projects", projectHash(Instance.directory), "memory")
}

/**
 * Primary (and only) write target — the memdir auto-memory directory, resolved
 * through the same layer the TUI uses (`getAutoMemPathFor` in src/memdir/paths),
 * so memory_write / memory_recall and the TUI memory UX share one store.
 */
function primaryMemoryDir(): string {
  return getAutoMemPathFor(Instance.directory)
}

/**
 * Pre-convergence stores, kept as READ-ONLY fallbacks (and one-time import
 * sources) so memories saved before the memdir convergence stay visible.
 * New writes never go here; a best-effort import copies their contents into
 * the memdir (see ensureLegacyImport).
 */
function legacyMemoryDirs(): string[] {
  return [
    path.join(Instance.directory, ".gizzi", "L1-COGNITIVE", "memory"),
    path.join(Instance.directory, ".openclaw", "L1-COGNITIVE", "memory"),
    legacyGlobalMemoryDir(),
  ]
}

/** Memory directories to scan, in precedence order (memdir first — it wins dedupe) */
function allMemoryDirs(): string[] {
  return [primaryMemoryDir(), ...legacyMemoryDirs()]
}

function memoryIndexPath(dir: string): string {
  return path.join(dir, "MEMORY.md")
}

// ── Index management ──────────────────────────────────────────────────────────

async function readIndex(dir: string): Promise<MemoryIndexEntry[]> {
  const content = await Filesystem.readText(memoryIndexPath(dir)).catch(() => "")
  if (!content) return []
  const entries: MemoryIndexEntry[] = []
  // Parse lines like: - [name](filename.md) — description (type)
  for (const line of content.split("\n")) {
    const m = line.match(/^-\s+\[([^\]]+)\]\(([^)]+)\)\s*[—–-]\s*(.+?)\s*\((\w+)\)$/)
    if (!m) continue
    entries.push({
      filename: m[2],
      description: m[3],
      type: m[4] as MemoryType,
    })
  }
  return entries
}

const INDEX_HEADER = "# Memory Index"

function formatIndexLine(entry: MemoryIndexEntry): string {
  return `- [${entry.filename.replace(/\.md$/, "")}](${entry.filename}) — ${entry.description} (${entry.type})`
}

/** True when a raw MEMORY.md line links to the given topic file */
function indexLineTargets(line: string, filename: string): boolean {
  return line.includes(`](${filename})`)
}

/**
 * Line-preserving index update. The memdir MEMORY.md is shared with the TUI's
 * auto-memory UX (extract-memories, `#` quick-add, hand edits), whose lines
 * don't all match the strict readIndex() shape — regenerating the file from
 * parsed entries alone would silently drop them. Instead, drop only the lines
 * linking this filename, then append the fresh line.
 */
async function upsertIndex(dir: string, entry: MemoryIndexEntry): Promise<void> {
  await Filesystem.mkdir(dir)
  const existing = await Filesystem.readText(memoryIndexPath(dir)).catch(() => "")
  const lines = existing.split("\n").filter((l) => !indexLineTargets(l, entry.filename))
  while (lines.length > 0 && lines[lines.length - 1]!.trim() === "") lines.pop()
  if (lines.length === 0) lines.push(INDEX_HEADER, "")
  lines.push(formatIndexLine(entry))
  await Filesystem.write(memoryIndexPath(dir), lines.join("\n") + "\n")
}

async function removeFromIndex(dir: string, filename: string): Promise<void> {
  const existing = await Filesystem.readText(memoryIndexPath(dir)).catch(() => "")
  if (!existing) return
  const lines = existing.split("\n").filter((l) => !indexLineTargets(l, filename))
  await Filesystem.write(memoryIndexPath(dir), lines.join("\n"))
}

// ── Legacy import ─────────────────────────────────────────────────────────────

const IMPORT_MARKER = ".memdir-import-v1.json"
const importInflight = new Map<string, Promise<void>>()

/**
 * One-time best-effort import of pre-convergence memories (L1-COGNITIVE dirs,
 * old global per-project store) into the memdir. Copies — never deletes — so
 * the old stores stay intact; a marker file in the memdir makes it run once
 * per project. Legacy dirs also remain in the read path as fallbacks.
 */
async function ensureLegacyImport(dir: string): Promise<void> {
  let pending = importInflight.get(dir)
  if (!pending) {
    pending = runLegacyImport(dir).catch((error) => {
      log.warn("legacy memory import failed (best-effort, continuing)", {
        dir,
        error: error instanceof Error ? error.message : String(error),
      })
    })
    importInflight.set(dir, pending)
  }
  return pending
}

async function runLegacyImport(dir: string): Promise<void> {
  if (await Filesystem.exists(path.join(dir, IMPORT_MARKER))) return
  await Filesystem.mkdir(dir)

  const report: { source: string; copied: string[]; skipped: string[] }[] = []
  for (const source of legacyMemoryDirs()) {
    if (path.resolve(source) === path.resolve(dir)) continue
    if (!(await Filesystem.exists(source))) continue
    const files = await Glob.scan("*.md", { cwd: source, absolute: true, include: "file" }).catch(() => [])
    const copied: string[] = []
    const skipped: string[] = []
    for (const filepath of files) {
      const filename = path.basename(filepath)
      if (filename === "MEMORY.md") {
        if (path.resolve(source) === path.resolve(legacyGlobalMemoryDir())) {
          // Old global store used this same index format — merge its entries
          // for topic files that were copied/are already present.
          const merged = await mergeIndexEntries(dir, source)
          ;(merged ? copied : skipped).push(filename)
        } else {
          // L1-COGNITIVE MEMORY.md is a session-log index with different
          // semantics — leave it in place (still visible via the legacy read
          // fallback) rather than folding it into the memdir index.
          skipped.push(filename)
        }
        continue
      }
      const dest = path.join(dir, filename)
      if (await Filesystem.exists(dest)) {
        skipped.push(filename)
        continue
      }
      try {
        await copyFile(filepath, dest)
        copied.push(filename)
      } catch {
        skipped.push(filename)
      }
    }
    if (copied.length > 0 || skipped.length > 0) report.push({ source, copied, skipped })
  }

  await Filesystem.write(
    path.join(dir, IMPORT_MARKER),
    JSON.stringify({ importedAt: new Date().toISOString(), sources: report }, null, 2),
  )
  const totalCopied = report.reduce((n, r) => n + r.copied.length, 0)
  if (totalCopied > 0) {
    log.info("imported pre-convergence memories into memdir", { dir, sources: report })
  } else {
    log.info("legacy memory import: nothing to import", { dir })
  }
}

/** Merge index entries from a same-format source MEMORY.md into the memdir's */
async function mergeIndexEntries(dir: string, source: string): Promise<boolean> {
  const entries = await readIndex(source)
  if (entries.length === 0) return false
  const existing = await readIndex(dir)
  const have = new Set(existing.map((e) => e.filename))
  let merged = false
  for (const entry of entries) {
    if (have.has(entry.filename)) continue
    if (!(await Filesystem.exists(path.join(dir, entry.filename)))) continue
    await upsertIndex(dir, entry)
    merged = true
  }
  return merged
}

// ── MemoryService ─────────────────────────────────────────────────────────────

export namespace MemoryService {
  /** List all memory entries across all dirs (deduplicated by filename, memdir wins) */
  export async function list(): Promise<MemoryEntry[]> {
    await ensureLegacyImport(primaryMemoryDir())
    const results = await listLocal()
    // One-time import into the canonical kernel (background, best-effort).
    void MemoryKernelAdapter.importOnce(primaryMemoryDir(), async () => results)
    return results
  }

  async function listLocal(): Promise<MemoryEntry[]> {
    const seen = new Set<string>()
    const results: MemoryEntry[] = []

    for (const dir of allMemoryDirs()) {
      if (!(await Filesystem.exists(dir))) continue
      const files = await Glob.scan("*.md", { cwd: dir, absolute: true, include: "file" }).catch(() => [])
      for (const filepath of files) {
        const filename = path.basename(filepath)
        if (filename === "MEMORY.md") continue
        if (seen.has(filename)) continue
        seen.add(filename)
        const entry = await readFile(filepath)
        if (entry) results.push(entry)
      }
    }
    return results
  }

  /** Read a memory entry by filename (searches memdir first, then legacy dirs) */
  export async function get(filename: string): Promise<MemoryEntry | null> {
    await ensureLegacyImport(primaryMemoryDir())
    for (const dir of allMemoryDirs()) {
      const filepath = path.join(dir, filename)
      if (await Filesystem.exists(filepath)) {
        return readFile(filepath)
      }
    }
    return null
  }

  /** Save a memory entry to the memdir and update its index */
  export async function save(fm: MemoryFrontmatter, body: string): Promise<MemoryEntry> {
    const dir = primaryMemoryDir()
    await ensureLegacyImport(dir)
    const filename = fm.name.replace(/\s+/g, "_").replace(/[^a-zA-Z0-9_-]/g, "") + ".md"
    await Filesystem.mkdir(dir)
    const filepath = path.join(dir, filename)

    const content = serializeFrontmatter(fm, body)
    await Filesystem.write(filepath, content)
    await upsertIndex(dir, { filename, description: fm.description, type: fm.type })

    log.info("memory saved", { filename, type: fm.type })
    void MemoryKernelAdapter.upsert([{ ...fm, filename, filepath, body }])
    await Bus.publish(MemoryEvent.Updated, { filepath, action: "save" as const }).catch(() => {})

    return { ...fm, filename, filepath, body }
  }

  /** Delete a memory entry by filename */
  export async function remove(filename: string): Promise<boolean> {
    for (const dir of allMemoryDirs()) {
      const filepath = path.join(dir, filename)
      if (await Filesystem.exists(filepath)) {
        await unlink(filepath).catch(() => {})
        await removeFromIndex(dir, filename)
        log.info("memory deleted", { filename })
        void MemoryKernelAdapter.remove([filepath])
        await Bus.publish(MemoryEvent.Updated, { filepath, action: "delete" as const }).catch(() => {})
        return true
      }
    }
    return false
  }

  /** Full-text search across all memories */
  export async function search(query: string): Promise<MemoryEntry[]> {
    const all = await list()
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean)
    const local = all.filter((e) => {
      const haystack = [e.name, e.description, e.type, e.body].join(" ").toLowerCase()
      return terms.every((t) => haystack.includes(t))
    })
    // Canonical recall: memdir files the kernel ranks relevant that the
    // literal match missed (e.g. paraphrases), after the local matches.
    const byId = new Map(all.map((e) => [MemoryKernelAdapter.externalId(e.filepath), e]))
    const seen = new Set(local.map((e) => e.filepath))
    for (const hit of await MemoryKernelAdapter.search(query)) {
      const e = hit.source === "gizzi.memdir" && hit.external_id ? byId.get(hit.external_id) : undefined
      if (e && !seen.has(e.filepath)) {
        seen.add(e.filepath)
        local.push(e)
      }
    }
    return local
  }

  /**
   * Select topic files relevant to a session title using TF-IDF-inspired scoring.
   * Returns absolute file paths sorted by relevance (highest first).
   * Only files with score > 0 are returned.
   */
  export async function selectForContext(sessionTitle: string, maxFiles = 10): Promise<string[]> {
    const all = await list()
    if (all.length === 0) return []

    const queryTerms = tokenize(sessionTitle)
    if (queryTerms.length === 0) {
      // No query — return all up to max
      return all.slice(0, maxFiles).map((e) => e.filepath)
    }

    const scored = all.map((e) => {
      const text = [e.name, e.description, e.type].join(" ")
      const score = queryTerms.filter((t) => text.toLowerCase().includes(t)).length
      return { filepath: e.filepath, score }
    })

    return scored
      .filter((s) => s.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, maxFiles)
      .map((s) => s.filepath)
  }

  // ── Private helpers ─────────────────────────────────────────────────────────

  async function readFile(filepath: string): Promise<MemoryEntry | null> {
    const content = await Filesystem.readText(filepath).catch(() => "")
    if (!content) return null
    const { fm, body } = parseFrontmatter(content)
    if (!fm.name || !fm.description || !fm.type) return null
    const filename = path.basename(filepath)
    return {
      name: fm.name,
      description: fm.description,
      type: fm.type,
      filename,
      filepath,
      body,
    }
  }

  function tokenize(text: string): string[] {
    return text
      .toLowerCase()
      .split(/[\s\-_/\\.,;:!?()[\]{}'"]+/)
      .filter((t) => t.length > 2)
  }
}
