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
import { appendFile, mkdir, readFile, rename, rm, stat, utimes, writeFile } from "node:fs/promises"
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

export async function markRead(key: string, ids: string[], kind: "delivered" | "read" = "read"): Promise<void> {
  if (!ids.length) return
  await mkdir(inboxDir(), { recursive: true })
  await appendFile(join(inboxDir(), `${key}.read`), ids.join("\n") + "\n")
  // check_inbox handing the text to the model is the receiver's confirmation and a read; a hook
  // printing it is a delivery (the next tool call or turn end upgrades it to read).
  await Promise.all(ids.map((id) => ackHop(id, kind, key)))
}

function safeId(id: string): string {
  return id.replace(/[^A-Za-z0-9_-]/g, "-")
}

/**
 * The receiver confirmed `hopId`. Stops the engine's re-pushing (removes
 * `live-pending/<hop>.json`) and leaves `live-acks/<hop>.json`, which the engine
 * reports to the cloud hop (`delivered`, or `read`). A later `read` upgrades a
 * `delivered` file; nothing ever downgrades one.
 */
export async function ackHop(hopId: string, kind: "delivered" | "read" = "delivered", key = ""): Promise<boolean> {
  const id = safeId(hopId)
  const dir = join(factoryHome(), "live-acks")
  const path = join(dir, `${id}.json`)
  const prior = JSON.parse(await readFile(path, "utf8").catch(() => "{}")) as { kind?: string; key?: string }
  const next = prior.kind === "read" ? "read" : kind
  await mkdir(dir, { recursive: true, mode: 0o700 })
  await writeFile(path, JSON.stringify({ hopId, kind: next, key: key || prior.key || "", at: new Date().toISOString() }), { mode: 0o600 })
  return rm(join(factoryHome(), "live-pending", `${id}.json`)).then(
    () => true,
    () => false,
  )
}

/** Confirm every `<allternit-message id="…">` the text carries (a pushed message reached the prompt): `delivered`. */
export async function ackEnvelopesIn(text: string, key = ""): Promise<string[]> {
  const ids = [...text.matchAll(/<allternit-message\b[^>]*?\bid="([^"]+)"/g)].map((m) => m[1])
  await Promise.all(ids.map((id) => ackHop(id, "delivered", key)))
  return ids
}

/**
 * The session kept working after a hop reached its prompt (next tool call or turn end): the
 * model has seen it, so every `delivered` hop of this session becomes `read`.
 */
export async function markDeliveredRead(key: string): Promise<number> {
  const dir = join(factoryHome(), "live-acks")
  const { readdir } = await import("node:fs/promises")
  let n = 0
  for (const f of await readdir(dir).catch(() => [] as string[])) {
    if (!f.endsWith(".json")) continue
    const row = JSON.parse(await readFile(join(dir, f), "utf8").catch(() => "{}")) as { hopId?: string; kind?: string; key?: string }
    if (row.kind === "delivered" && row.hopId && row.key === key) {
      await ackHop(row.hopId, "read", key)
      n++
    }
  }
  return n
}

/**
 * Register this Claude Code process's messaging socket + token with the engine
 * (`live-messaging/<claude pid>.json`, 0600). Claude hands both to the MCP and
 * hook processes it starts; the engine needs the token to push, and the folder
 * + permission mode to wake the session if it closes. Kept after exit on purpose.
 */
export async function registerMessaging(
  env: NodeJS.ProcessEnv = process.env,
  facts: { sessionId?: string; cwd?: string; permissionMode?: string } = {},
): Promise<boolean> {
  const socket = env.CLAUDE_CODE_MESSAGING_SOCKET?.trim()
  const token = env.CLAUDE_CODE_MESSAGING_TOKEN?.trim()
  if (!socket || !token) return false
  const dir = join(factoryHome(), "live-messaging")
  await mkdir(dir, { recursive: true, mode: 0o700 })
  const path = join(dir, `${process.ppid}.json`)
  const prior = JSON.parse(await readFile(path, "utf8").catch(() => "{}")) as Record<string, string>
  const row = {
    socket,
    token,
    sessionId: facts.sessionId ?? prior.sessionId ?? env.CLAUDE_SESSION_ID ?? "",
    cwd: facts.cwd ?? prior.cwd ?? process.cwd(),
    permissionMode: facts.permissionMode ?? prior.permissionMode ?? "",
    at: new Date().toISOString(),
  }
  const tmp = `${path}.${process.pid}.tmp`
  await writeFile(tmp, JSON.stringify(row), { mode: 0o600 })
  await rename(tmp, path)
  return true
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
