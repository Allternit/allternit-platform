// Portions adapted from Reason Network (reason-machines/reason-network), Apache-2.0. Copyright 2026 Reason Machines, Inc.
/**
 * Live-session inbox (agent network C2 steps 6–7).
 *
 * The Factory engine (`live_delivery.rs`) appends one NDJSON line per message
 * to `<factory home>/live-inbox/<session>.ndjson`; `gizzi hook` prints them at
 * a hook, and `check_inbox` returns them. Read marks go to a sibling
 * `<session>.read` file (ids, append-only), so a read never races the
 * engine's appends. Messages are marked read only after they were written to
 * stdout.
 *
 * Free of CLI imports so bun tests can drive it against a temp factory home.
 */
import { appendFile, mkdir, readFile, rename, stat, utimes, writeFile } from "node:fs/promises"
import { homedir } from "node:os"
import { basename, dirname, join } from "node:path"

export interface InboxMessage {
  id: string
  from: string
  envelope: string
  at: string
}

/** `$ALLTERNIT_FACTORY_HOME`, else `~/.allternit/factory` (same rule as the engine). */
export function factoryHome(): string {
  const env = process.env.ALLTERNIT_FACTORY_HOME?.trim()
  return env ? env : join(homedir(), ".allternit/factory")
}

export function inboxDir(): string {
  return join(factoryHome(), "live-inbox")
}

/** File-safe key of a session name (the part of an address after `/`); same rule as the engine's `inbox_key`. */
export function inboxKey(addressOrName: string): string {
  const name = addressOrName.split("/").pop() ?? addressOrName
  return name.toLowerCase().replace(/[^a-z0-9._-]/g, "-")
}

/** Where hook runs are recorded for live-session discovery (the engine's `live_sessions.rs` reads it). */
export function hooksFile(): string {
  return process.env.ALLTERNIT_HOOKS_FILE || join(homedir(), ".allternit/live-sessions/hooks.json")
}

/** Record one hook run: `{sessions: {<id>: {harness, cwd, transcript_path, pid, last_hook}}}`. Drops entries older than 6 h. */
export async function recordHook(entry: { session: string; harness: string; cwd: string; transcript?: string; pid?: number }): Promise<void> {
  const path = hooksFile()
  let doc: { sessions?: Record<string, Record<string, unknown>> } = {}
  try {
    doc = JSON.parse(await readFile(path, "utf8"))
  } catch {}
  const sessions = doc.sessions ?? {}
  const cutoff = Date.now() - 6 * 3600_000
  for (const [id, v] of Object.entries(sessions)) {
    if (Date.parse(String(v.last_hook ?? "")) < cutoff) delete sessions[id]
  }
  sessions[entry.session] = {
    harness: entry.harness,
    cwd: entry.cwd,
    ...(entry.transcript ? { transcript_path: entry.transcript } : {}),
    ...(entry.pid ? { pid: entry.pid } : {}),
    last_hook: new Date().toISOString(),
  }
  await mkdir(dirname(path), { recursive: true })
  const tmp = `${path}.${process.pid}.tmp`
  await writeFile(tmp, JSON.stringify({ sessions }), { mode: 0o600 })
  await rename(tmp, path)
}

/** This session's inbox key: `$ALLTERNIT_LIVE_SESSION`, else `<dir>-<harness>`. */
export function sessionKey(harness: string, cwd: string = process.cwd()): string {
  const env = process.env.ALLTERNIT_LIVE_SESSION?.trim()
  return inboxKey(env || `${basename(cwd) || "session"}-${harness}`)
}

async function readLines(path: string): Promise<string[]> {
  return (await readFile(path, "utf8").catch(() => "")).split("\n").filter((l) => l.trim())
}

export async function readPending(key: string): Promise<InboxMessage[]> {
  const base = join(inboxDir(), key)
  const read = new Set(await readLines(`${base}.read`))
  const out: InboxMessage[] = []
  for (const line of await readLines(`${base}.ndjson`)) {
    try {
      const m = JSON.parse(line) as InboxMessage
      if (m.id && m.envelope && !read.has(m.id)) out.push(m)
    } catch {
      // a torn line is skipped, never fatal
    }
  }
  return out
}

export async function markRead(key: string, ids: string[]): Promise<void> {
  if (!ids.length) return
  await mkdir(inboxDir(), { recursive: true })
  await appendFile(join(inboxDir(), `${key}.read`), ids.join("\n") + "\n")
}

/** `N unread from a, b.` or undefined when the inbox is empty. */
export function unreadNotice(pending: InboxMessage[]): string | undefined {
  if (!pending.length) return undefined
  const from = [...new Set(pending.map((m) => m.from))].slice(0, 5).join(", ")
  return `[Allternit] ${pending.length} unread from ${from}. Call check_inbox.`
}

/** Heartbeat file the engine reads to know a session is parked in `check_inbox` (fresh within 3 s). */
export async function touchWaiting(key: string): Promise<void> {
  const path = join(inboxDir(), `${key}.waiting`)
  await mkdir(inboxDir(), { recursive: true })
  const now = new Date()
  await utimes(path, now, now).catch(() => writeFile(path, "1"))
}

export async function clearWaiting(key: string): Promise<void> {
  await writeFile(join(inboxDir(), `${key}.waiting`), "").catch(() => {})
  await utimes(join(inboxDir(), `${key}.waiting`), new Date(0), new Date(0)).catch(() => {})
}

export async function isWaiting(key: string): Promise<boolean> {
  const s = await stat(join(inboxDir(), `${key}.waiting`)).catch(() => undefined)
  return !!s && Date.now() - s.mtimeMs < 3000
}
