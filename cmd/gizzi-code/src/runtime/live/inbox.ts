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
import { appendFile, mkdir, readFile, stat, utimes, writeFile } from "node:fs/promises"
import { homedir } from "node:os"
import { basename, join } from "node:path"

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
  return name.replace(/[^A-Za-z0-9._-]/g, "-")
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
