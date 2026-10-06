/**
 * Plain-text views of a Memory Drive, shared by the TUI (/memory,
 * /memory-search) and the CLI (`gizzi memory …`). No UI imports, so the
 * exact strings are unit-testable.
 */
import type { CommitInfo, DriveStatus, SearchHit } from "./checkout"
import type { Dream } from "./platform"

export interface DriveOverview {
  ref: string
  name: string
  signedIn: boolean
  status: DriveStatus
  files: { path: string; bytes: number; entries: number }[]
  history: CommitInfo[]
}

const short = (rev?: string) => (rev ? rev.slice(0, 7) : "—")

function when(iso?: string): string {
  if (!iso) return "never"
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}

export function syncLine(status: DriveStatus, signedIn: boolean): string {
  if (!status.remote) {
    return signedIn
      ? "Sync: not connected to the server yet — saved on this computer; gizzi connects next session."
      : "Sync: local only (signed out). Run `gizzi login` to sync it to your Allternit account."
  }
  if (status.readOnly) return `Sync: read-only mount · last sync ${when(status.lastSyncAt)}${status.lastError ? ` · ${status.lastError}` : ""}`
  if (status.pending) {
    return `Sync: ${status.ahead} change${status.ahead === 1 ? "" : "s"} waiting to sync${status.lastError ? ` — ${status.lastError}` : ""}`
  }
  if (status.lastError) return `Sync problem: ${status.lastError}`
  return `Sync: up to date · last sync ${when(status.lastSyncAt)}`
}

export function formatOverview(o: DriveOverview): string {
  const lines: string[] = []
  lines.push(`Memory Drive — ${o.name}${o.ref === "personal" ? "" : ` (${o.ref})`}`)
  lines.push(`Checkout: ${o.status.dir}`)
  lines.push(syncLine(o.status, o.signedIn))
  if (o.status.rejected) lines.push(`Not saved: ${o.status.rejected}`)
  if (o.status.dirty) lines.push("Unsaved edits in the checkout will be committed at the end of the next turn (or run `gizzi memory sync`).")
  lines.push("")
  lines.push(`Files (${o.files.length}):`)
  if (o.files.length === 0) lines.push("  (none yet)")
  for (const f of o.files) {
    lines.push(`  ${f.path}${f.path === "MEMORY.md" ? "  (index)" : `  ${f.entries} memor${f.entries === 1 ? "y" : "ies"}`}`)
  }
  if (o.history.length > 0) lines.push("", formatHistory(o.history))
  return lines.join("\n")
}

export function formatHistory(history: CommitInfo[]): string {
  const lines = ["Recent history:"]
  if (history.length === 0) lines.push("  (no commits yet)")
  for (const c of history) {
    lines.push(`  ${short(c.revision)}  ${c.timestamp.slice(0, 16).replace("T", " ")}  ${c.message}  — ${c.author}`)
  }
  return lines.join("\n")
}

export function formatFileView(path: string, content: string): string {
  return `${path}\n${"─".repeat(Math.min(60, Math.max(8, path.length)))}\n${content.trimEnd()}`
}

export function formatSearch(query: string, hits: (SearchHit & { ref: string })[]): string {
  if (hits.length === 0) return query ? `No memories match "${query}".` : "No memories saved yet."
  const lines = [`${hits.length} match${hits.length === 1 ? "" : "es"}${query ? ` for "${query}"` : ""}:`]
  for (const h of hits) {
    const meta = h.entry ? `  (${h.entry.added}; ${h.entry.source}; id ${h.entry.id})` : ""
    lines.push(`  ${h.ref === "personal" ? "" : `${h.ref}:`}${h.path}:${h.line}  ${h.text}${meta}`)
  }
  return lines.join("\n")
}

export function formatDreams(dreams: Dream[]): string {
  if (dreams.length === 0) return "No Dreams yet. The nightly Dream runs on the server when Dreaming is on for this drive."
  const lines = ["Dreams (newest first):"]
  for (const d of dreams) {
    const s = d.summary ?? {}
    const counts = `merged ${s.merged ?? 0}, resolved ${s.resolved ?? 0}, lessons ${s.lessons ?? 0}, pruned ${s.pruned ?? 0}, proposals ${s.proposals ?? 0}`
    lines.push(`  ${d.id}  ${d.date}  ${d.status}  ${counts}${d.revision ? `  ${short(d.revision)}` : ""}${d.error ? `  — ${d.error}` : ""}`)
  }
  lines.push("", "Undo one with: gizzi memory undo-dream <id>")
  return lines.join("\n")
}
