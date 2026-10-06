/**
 * MemoryService — gizzi's memory CRUD, backed by the user's Memory Drive
 * (src/runtime/memory/drive). The drive checkout is the only store: one-line
 * spec bullets in topic files, MEMORY.md regenerated as the index, every
 * write a git commit that syncs to the server (which reindexes after push).
 *
 * The older file-per-memory shape (name/description/type/body) is kept as a
 * bridge for the runtime HTTP routes and SDK: `list()`/`get()` return one
 * MemoryEntry per drive topic file, `save()` turns a legacy memory into one
 * bullet with a stable `legacy-<name>` id (so saving the same name updates
 * it), and `remove()` deletes a topic file or that legacy id.
 *
 * Legacy frontmatter memdir files are no longer read here; they come into the
 * drive once through `gizzi memory import` (dry run first).
 */
import path from "path"
import z from "zod/v4"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { MemoryDrive, sessionSource, topicPath } from "@/runtime/memory/drive/drive"
import { ENTRYPOINT, isEntryLine, rustLines, type Entry } from "@/runtime/memory/drive/format"
import type { WriteResult } from "@/runtime/memory/drive/checkout"

const log = Log.create({ service: "memory-service" })

export type MemoryType = "user" | "feedback" | "project" | "reference"

export interface MemoryFrontmatter {
  name: string
  description: string
  type: MemoryType
}

export interface MemoryEntry extends MemoryFrontmatter {
  filename: string // drive-relative topic path, e.g. "preferences.md"
  filepath: string // absolute path in the checkout
  body: string
  mtime?: number
}

export interface DriveHit {
  ref: string
  path: string
  filepath: string
  line: number
  text: string
  entry?: Entry
}

// ── Bus event ────────────────────────────────────────────────────────────────

export namespace MemoryEvent {
  export const Updated = BusEvent.define(
    "memory.updated",
    z.object({ filepath: z.string(), action: z.enum(["save", "delete"]) }),
  )
}

// ── Frontmatter helpers (legacy files; used by the importer and old callers) ──

export function parseFrontmatter(content: string): { fm: Partial<MemoryFrontmatter>; body: string } {
  const lines = content.split("\n")
  if (lines[0]?.trim() !== "---") return { fm: {}, body: content }
  const closeIdx = lines.findIndex((l, i) => i > 0 && l.trim() === "---")
  if (closeIdx < 0) return { fm: {}, body: content }
  const fm: Partial<MemoryFrontmatter> = {}
  for (const line of lines.slice(1, closeIdx)) {
    const colon = line.indexOf(":")
    if (colon < 0) continue
    const key = line.slice(0, colon).trim()
    const val = line.slice(colon + 1).trim()
    if (key === "name") fm.name = val
    else if (key === "description") fm.description = val
    else if (key === "type") fm.type = val as MemoryType
  }
  return { fm, body: lines.slice(closeIdx + 1).join("\n").trimStart() }
}

export function serializeFrontmatter(fm: MemoryFrontmatter, body: string): string {
  return ["---", `name: ${fm.name}`, `description: ${fm.description}`, `type: ${fm.type}`, "---", "", body].join("\n")
}

// ── bridge helpers ───────────────────────────────────────────────────────────

function typeForPath(rel: string): MemoryType {
  if (rel.startsWith("preferences")) return "feedback"
  if (rel.startsWith("projects")) return "project"
  if (rel.startsWith("references")) return "reference"
  return "user"
}

function legacyId(name: string): string {
  const slug = name.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 100)
  return `legacy-${slug || "memory"}`
}

function topicEntry(dir: string, rel: string, content: string): MemoryEntry {
  const lines = rustLines(content)
  const heading = lines.find((l) => l.startsWith("# "))?.slice(2).trim()
  const count = lines.filter(isEntryLine).length
  return {
    name: rel.replace(/\.md$/, ""),
    description: `${heading ?? rel} — ${count} memor${count === 1 ? "y" : "ies"}`,
    type: typeForPath(rel),
    filename: rel,
    filepath: path.join(dir, rel),
    body: content,
  }
}

// ── MemoryService ─────────────────────────────────────────────────────────────

export namespace MemoryService {
  /** One entry per topic file of the personal drive (MEMORY.md excluded). */
  export async function list(): Promise<MemoryEntry[]> {
    const drive = await MemoryDrive.open("personal")
    await drive.ensure()
    const { files } = await drive.workingFiles()
    return Object.keys(files)
      .filter((p) => p !== ENTRYPOINT)
      .sort()
      .map((p) => topicEntry(drive.dir, p, files[p]!))
  }

  /** Read one topic file by its drive-relative path (".md" optional). */
  export async function get(filename: string): Promise<MemoryEntry | null> {
    const rel = filename.endsWith(".md") ? filename : `${filename}.md`
    const drive = await MemoryDrive.open("personal")
    await drive.ensure()
    try {
      return topicEntry(drive.dir, rel, await drive.readFile(rel))
    } catch {
      return null
    }
  }

  /**
   * Legacy save: one bullet, id `legacy-<name>`, in the topic for its type.
   * Prefer MemoryDrive.remember for new code.
   */
  export async function save(fm: MemoryFrontmatter, body: string, options: { sessionId?: string } = {}): Promise<MemoryEntry & { result: WriteResult }> {
    const text = [fm.description, body].map((s) => s?.replace(/\s+/g, " ").trim()).filter(Boolean).join(" — ")
    const saved = await MemoryDrive.remember({
      text: text || fm.name,
      type: fm.type,
      id: legacyId(fm.name),
      topic: topicPath(undefined, fm.type, Instance.directory).replace(/\.md$/, ""),
      sessionId: options.sessionId,
      projectDir: Instance.directory,
    })
    const drive = await MemoryDrive.open("personal")
    const filepath = path.join(drive.dir, saved.path)
    log.info("memory saved", { path: saved.path, id: saved.entry.id })
    await Bus.publish(MemoryEvent.Updated, { filepath, action: "save" as const }).catch(() => {})
    return { ...fm, filename: saved.path, filepath, body: saved.entry.text, result: saved.result }
  }

  /** Delete a topic file (drive-relative path) or a legacy memory by name. */
  export async function remove(filename: string): Promise<boolean> {
    const drive = await MemoryDrive.open("personal")
    await drive.ensure()
    const rel = filename.endsWith(".md") ? filename : `${filename}.md`
    const { files } = await drive.workingFiles()
    if (rel !== ENTRYPOINT && rel in files) {
      await drive.write([{ DeleteFile: { path: rel } }], { message: `Delete memory topic ${rel}`, source: sessionSource() })
      await Bus.publish(MemoryEvent.Updated, { filepath: path.join(drive.dir, rel), action: "delete" as const }).catch(() => {})
      return true
    }
    const name = filename.replace(/\.md$/, "")
    const forgotten = await MemoryDrive.forget(name.startsWith("legacy-") || name.startsWith("entry-") ? name : legacyId(name))
    if (forgotten.found) {
      await Bus.publish(MemoryEvent.Updated, { filepath: drive.dir, action: "delete" as const }).catch(() => {})
    }
    return forgotten.found
  }

  /** Line-level search across every mounted drive (personal first). */
  export async function searchEntries(query: string, limit = 50): Promise<DriveHit[]> {
    const hits = await MemoryDrive.search(query, limit)
    return hits.map((h) => ({ ref: h.ref, path: h.path, filepath: path.join(h.dir, h.path), line: h.line, text: h.text, entry: h.entry }))
  }

  /** Legacy shape: one MemoryEntry per matching line. */
  export async function search(query: string): Promise<MemoryEntry[]> {
    const hits = await searchEntries(query)
    return hits.map((h) => ({
      name: h.entry?.id ?? `${h.path}:${h.line}`,
      description: h.text,
      type: typeForPath(h.path),
      filename: h.path,
      filepath: h.filepath,
      body: h.text,
    }))
  }
}
