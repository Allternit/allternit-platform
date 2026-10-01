/**
 * Memory kernel adapter (WP-M1d, decision M1).
 *
 * The canonical memory store is the allternit-api memory kernel. The memdir
 * files stay as gizzi's local working copy (the TUI and MEMORY.md index read
 * them), and every write is mirrored to the kernel through
 * `/api/v1/memory/adapters/*`: a file maps to one canonical fact (source
 * `gizzi.memdir`, external id = memory dir key + filename), so re-sending a
 * file is an update and the one-time import is idempotent.
 *
 * Best-effort by design: signed out, offline or an old API means the call is
 * skipped and local behaviour is unchanged. `GIZZI_MEMORY_KERNEL=0` turns it off.
 */
import path from "path"
import { createHash } from "crypto"
import { existsSync } from "fs"
import { writeFile } from "fs/promises"
import { platformRequest } from "@/runtime/bots/platform-api"

export const KERNEL_SOURCE = "gizzi.memdir"
export const IMPORT_MARKER = ".kernel-import-v1"
const CHUNK = 200
const TIMEOUT_MS = 5_000

export interface KernelMemoryEntry {
  name: string
  description: string
  type: string
  filename: string
  filepath: string
  body: string
}

export interface KernelItem {
  external_id: string
  text: string
  memory_type: string
}

export interface KernelHit {
  id: string
  type: string
  text: string
  score: number
  source: string | null
  external_id: string | null
}

export namespace MemoryKernelAdapter {
  export function enabled(): boolean {
    return process.env.GIZZI_MEMORY_KERNEL !== "0"
  }

  /** Stable per-directory key so two projects' `notes.md` stay distinct. */
  export function externalId(filepath: string): string {
    const dir = createHash("sha256").update(path.dirname(filepath)).digest("hex").slice(0, 12)
    return `${dir}/${path.basename(filepath)}`
  }

  /** memdir type → kernel memory type (the V208 closed set). */
  export function memoryType(type: string): string {
    switch (type) {
      case "feedback":
        return "preference"
      case "project":
        return "task_state"
      default:
        return "fact"
    }
  }

  export function toItem(entry: KernelMemoryEntry): KernelItem {
    const head = entry.description ? `${entry.name}: ${entry.description}` : entry.name
    return {
      external_id: externalId(entry.filepath),
      text: `${head}\n\n${entry.body}`.trim().slice(0, 8000),
      memory_type: memoryType(entry.type),
    }
  }

  /** Mirror entries into the kernel. Returns false when the call didn't land. */
  export async function upsert(entries: KernelMemoryEntry[]): Promise<boolean> {
    if (!enabled() || entries.length === 0) return true
    try {
      for (let i = 0; i < entries.length; i += CHUNK) {
        const items = entries.slice(i, i + CHUNK).map(toItem)
        await platformRequest("POST", "/api/v1/memory/adapters/upsert", { source: KERNEL_SOURCE, items }, { timeoutMs: TIMEOUT_MS })
      }
      return true
    } catch {
      return false
    }
  }

  export async function remove(filepaths: string[]): Promise<boolean> {
    if (!enabled() || filepaths.length === 0) return true
    try {
      await platformRequest(
        "POST",
        "/api/v1/memory/adapters/delete",
        { source: KERNEL_SOURCE, external_ids: filepaths.map(externalId) },
        { timeoutMs: TIMEOUT_MS },
      )
      return true
    } catch {
      return false
    }
  }

  /** Canonical recall over the user's memory (all sources). Empty on failure. */
  export async function search(query: string, limit = 10): Promise<KernelHit[]> {
    if (!enabled() || !query.trim()) return []
    try {
      const r = await platformRequest<{ items: KernelHit[] }>(
        "POST",
        "/api/v1/memory/adapters/search",
        { query, limit },
        { timeoutMs: TIMEOUT_MS },
      )
      return r.items ?? []
    } catch {
      return []
    }
  }

  const imported = new Set<string>()

  /**
   * One-time import of a memory dir's existing files. A marker file records
   * success; a failed import retries on the next process.
   */
  export async function importOnce(dir: string, entries: () => Promise<KernelMemoryEntry[]>): Promise<boolean> {
    if (!enabled() || imported.has(dir)) return true
    imported.add(dir)
    const marker = path.join(dir, IMPORT_MARKER)
    if (existsSync(marker)) return true
    const all = await entries().catch(() => [] as KernelMemoryEntry[])
    if (!(await upsert(all))) {
      imported.delete(dir)
      return false
    }
    if (existsSync(dir)) await writeFile(marker, new Date().toISOString()).catch(() => {})
    return true
  }

  /** Tests: forget which dirs this process imported. */
  export function resetForTests(): void {
    imported.clear()
  }
}
