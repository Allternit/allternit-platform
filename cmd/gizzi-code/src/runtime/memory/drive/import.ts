/**
 * One-time import of gizzi's legacy frontmatter memdir files
 * (`<config>/projects/<project>/memory/*.md`, the pre-drive auto-memory
 * store) into the personal Memory Drive.
 *
 * - Dry run first (`gizzi memory import --dry-run`): a plan, nothing written.
 * - Apply (`--apply`) is one commit, "Import existing gizzi memory".
 * - Idempotent: stable ids per source file, plus a receipt file
 *   `imports/gizzi-memdir.md` (headings only, so it is never indexed as a
 *   fact); a second apply is a no-op.
 * - Provenance is honest: no session is invented, every row gets
 *   `source: imported:unknown`; the original date is the file's mtime.
 * - Sources are read, never modified or deleted.
 */
import { createHash } from "crypto"
import { existsSync } from "fs"
import { readdir, readFile, stat } from "fs/promises"
import os from "os"
import path from "path"
import { GlobalPaths } from "@/runtime/context/global/paths"
import type { DriveCheckout, WriteResult } from "./checkout"
import {
  DriveFormatError,
  DriveSecretError,
  IMPORTED_UNKNOWN,
  renderEntry,
  today,
  validateSource,
  type Entry,
  type Operation,
} from "./format"
import { memoryTypeFor, topicPath, topicSlug } from "./drive"

export const RECEIPT_PATH = "imports/gizzi-memdir.md"
const MAX_TEXT_BYTES = 3800
const MAX_TOPIC_BYTES = 56 * 1024

export interface ImportRow {
  file: string
  project: string
  name?: string
  type?: string
  topic?: string
  entries: number
  status: "convert" | "skip"
  reason?: string
}

export interface ImportPlan {
  total: number
  converted: number
  skipped: number
  topic_files: string[]
  already_imported: boolean
  rows: ImportRow[]
}

interface PlannedEntry {
  topic: string
  entry: Entry
}

/** Default legacy roots: the gizzi config home's projects/ and the old global per-project store. */
export function legacyMemdirRoots(): string[] {
  const roots: string[] = []
  const gizziHome = process.env.GIZZI_CONFIG_DIR ?? path.join(GlobalPaths.home, ".gizzi")
  roots.push(path.join(gizziHome, "projects"))
  roots.push(path.join(GlobalPaths.config, "projects"))
  const remote = process.env.GIZZI_CODE_REMOTE_MEMORY_DIR ?? process.env.GIZZI_REMOTE_MEMORY_DIR
  if (remote) roots.push(path.join(remote, "projects"))
  return [...new Set(roots.map((r) => path.resolve(r)))]
}

function parseFrontmatter(content: string): { fm: Record<string, string>; body: string } | undefined {
  const lines = content.split("\n")
  if (lines[0]?.trim() !== "---") return undefined
  const close = lines.findIndex((l, i) => i > 0 && l.trim() === "---")
  if (close < 0) return undefined
  const fm: Record<string, string> = {}
  for (const line of lines.slice(1, close)) {
    const colon = line.indexOf(":")
    if (colon > 0) fm[line.slice(0, colon).trim()] = line.slice(colon + 1).trim()
  }
  return { fm, body: lines.slice(close + 1).join("\n").trim() }
}

/** Make legacy prose fit the one-line format without changing its words. */
export function sanitizeText(raw: string): string {
  let text = raw
    .replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .replace(/</g, "‹")
    .replace(/>/g, "›")
    .replace(/!\[/g, "! [")
    .replace(/ \[([A-Za-z_][A-Za-z0-9_-]*):/g, " ($1:")
  // Markdown links whose target is not an allowed source become plain text.
  text = text.replace(/\]\(([^)]*)\)/g, (match, target: string) => {
    try {
      validateSource(target)
      return match
    } catch {
      return `] (${target})`
    }
  })
  return text
}

function splitText(text: string): string[] {
  if (Buffer.byteLength(text) <= MAX_TEXT_BYTES) return [text]
  const parts: string[] = []
  let current = ""
  for (const word of text.split(" ")) {
    const next = current ? `${current} ${word}` : word
    if (Buffer.byteLength(next) > MAX_TEXT_BYTES && current) {
      parts.push(current)
      current = word
    } else current = next
  }
  if (current) parts.push(current)
  return parts.map((p) => (Buffer.byteLength(p) > MAX_TEXT_BYTES ? p.slice(0, 1200) : p))
}

const shortHash = (s: string, n = 32) => createHash("sha256").update(s).digest("hex").slice(0, n)

async function memoryFiles(roots: string[]): Promise<{ file: string; project: string }[]> {
  const out: { file: string; project: string }[] = []
  for (const root of roots) {
    if (!existsSync(root)) continue
    const projects = await readdir(root, { withFileTypes: true }).catch(() => [])
    for (const p of projects) {
      if (!p.isDirectory()) continue
      const memdir = path.join(root, p.name, "memory")
      const files = await readdir(memdir, { withFileTypes: true }).catch(() => [])
      for (const f of files) {
        if (f.isFile() && f.name.endsWith(".md")) out.push({ file: path.join(memdir, f.name), project: p.name })
      }
    }
  }
  return out.sort((a, b) => a.file.localeCompare(b.file))
}

async function buildPlan(roots: string[], receiptExists: boolean): Promise<{ plan: ImportPlan; entries: PlannedEntry[] }> {
  const rows: ImportRow[] = []
  const entries: PlannedEntry[] = []
  for (const { file, project } of await memoryFiles(roots)) {
    const base = path.basename(file)
    if (base === "MEMORY.md") {
      rows.push({ file, project, entries: 0, status: "skip", reason: "index file (rebuilt in the drive)" })
      continue
    }
    const content = await readFile(file, "utf8").catch(() => "")
    const parsed = parseFrontmatter(content)
    if (!parsed || !parsed.fm.name) {
      rows.push({ file, project, entries: 0, status: "skip", reason: "no name/description/type frontmatter" })
      continue
    }
    const { fm, body } = parsed
    const type = fm.type || "user"
    const topic = topicPath(undefined, type, project.replace(/^-+/, "").split("-").slice(-3).join("-") || project)
    const head = [fm.name, fm.description].filter(Boolean).join(": ")
    const text = sanitizeText(body ? `${head} — ${body}` : head)
    const mtime = await stat(file).then((s) => s.mtime).catch(() => new Date())
    const added = today(mtime)
    const originHash = shortHash(content, 16)
    const fileKey = `${project}/${base}`
    const planned: PlannedEntry[] = []
    let reason: string | undefined
    for (const [i, part] of splitText(text).entries()) {
      const entry: Entry = {
        id: `memdir-${shortHash(`${fileKey}#${i}`)}`,
        text: part,
        source: IMPORTED_UNKNOWN,
        added,
        metadata: {
          memory_type: memoryTypeFor(type),
          origin: "gizzi-memdir",
          origin_hash: originHash,
          ...(splitText(text).length > 1 ? { part: String(i + 1) } : {}),
        },
      }
      try {
        renderEntry(entry)
        planned.push({ topic, entry })
      } catch (error) {
        reason =
          error instanceof DriveSecretError
            ? "looks like it contains a credential (not imported)"
            : error instanceof DriveFormatError
              ? error.message
              : String(error)
        break
      }
    }
    if (reason) {
      rows.push({ file, project, name: fm.name, type, topic, entries: 0, status: "skip", reason })
      continue
    }
    entries.push(...planned)
    rows.push({ file, project, name: fm.name, type, topic, entries: planned.length, status: "convert" })
  }
  const packed = pack(entries)
  const converted = rows.filter((r) => r.status === "convert").length
  return {
    plan: {
      total: rows.length,
      converted,
      skipped: rows.length - converted,
      topic_files: [...new Set(packed.map((e) => e.topic))].sort(),
      already_imported: receiptExists,
      rows,
    },
    entries: packed,
  }
}

/** Keep every topic file under the 64 KiB cap by spilling into `<topic>-2.md`, … */
function pack(entries: PlannedEntry[]): PlannedEntry[] {
  const size = new Map<string, number>()
  return entries.map((e) => {
    const line = Buffer.byteLength(renderEntry(e.entry)) + 1
    let n = 1
    let topic = e.topic
    while ((size.get(topic) ?? 0) + line > MAX_TOPIC_BYTES) {
      n++
      topic = e.topic.replace(/\.md$/, `-${n}.md`)
    }
    size.set(topic, (size.get(topic) ?? 0) + line)
    return { ...e, topic }
  })
}

export async function planMemdirImport(drive: DriveCheckout, roots = legacyMemdirRoots()): Promise<ImportPlan> {
  await drive.ensure()
  const receipt = existsSync(path.join(drive.dir, RECEIPT_PATH))
  return (await buildPlan(roots, receipt)).plan
}

export async function applyMemdirImport(
  drive: DriveCheckout,
  roots = legacyMemdirRoots(),
): Promise<{ plan: ImportPlan; applied: boolean; result?: WriteResult }> {
  await drive.ensure()
  const receipt = existsSync(path.join(drive.dir, RECEIPT_PATH))
  const { plan, entries } = await buildPlan(roots, receipt)
  if (plan.already_imported || entries.length === 0) return { plan, applied: false }
  const host = topicSlug(os.hostname() || "this-computer")
  const operations: Operation[] = entries.map((e) => ({ UpsertEntry: { path: e.topic, entry: e.entry } }))
  operations.push({
    SetFile: {
      path: RECEIPT_PATH,
      content: `# Import: gizzi memdir\n\n## ${today()} from ${host}: ${entries.length} entries from ${plan.converted} files, ${plan.skipped} skipped\n`,
    },
  })
  const result = await drive.write(operations, { message: "Import existing gizzi memory", source: IMPORTED_UNKNOWN })
  return { plan, applied: true, result }
}
