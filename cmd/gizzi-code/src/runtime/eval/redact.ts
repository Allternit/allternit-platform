import os from "os"
import { spawn } from "child_process"
import { SessionSupportBundle } from "@/runtime/session/support-bundle"
import { redactSecrets } from "@/shared/util/redact"

// Minimal local shapes. The eval schema lives in a parallel PR, so nothing here imports it.
export type TraceEntry = {
  sequence: number
  kind: string
  data?: unknown
  [key: string]: unknown
}
export type Turn = { role: string; text: string }

export type PrivateReport = { secrets: number; paths: number; entries_dropped: number }
export type ExportReport = PrivateReport & { emails: number; hosts: number; truncated: number }

export const MAX_ENTRIES = 5_000
const KEEP_TAIL_REQUESTS = 50
const MAX_TOOL_OUTPUT = 2_000
const MAX_TAR_FILE = 1_000_000
const HOST_ALLOWLIST = ["github.com", "npmjs.com", "pypi.org", "localhost"]

// Token shapes the shared helpers do not cover (GitHub tokens).
const GITHUB_TOKEN = /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g
const SK_KEY = /\b(?:sk-(?:ant|proj)-[A-Za-z0-9_-]{8,}|sk-[A-Za-z0-9]{16,})\b/
const JWT = /\beyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\b/
const BEARER = /\bBearer\s+[A-Za-z0-9._~+/=-]{8,}\b/i

const EMAIL = /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+\b/g
const USER_PATH = /\/(?:Users|home)\/[A-Za-z0-9._-]+/g
const URL_HOST = /(\bhttps?:\/\/)([^\s/:?#"'<>)\]]+)/gi

type Counts = { secrets: number; paths: number; emails: number; hosts: number; truncated: number }

function countOf(text: string, needle: string) {
  return needle ? text.split(needle).length - 1 : 0
}

function hostAllowed(host: string) {
  const h = host.toLowerCase()
  return HOST_ALLOWLIST.some((ok) => h === ok || h.endsWith(`.${ok}`))
}

function scrubString(value: string, key: string, strict: boolean, c: Counts): string {
  let text = value
  const home = os.homedir()
  if (strict) {
    if (home && home !== "/") {
      c.paths += countOf(text, home)
      text = text.replaceAll(home, "~")
    }
    text = text.replace(USER_PATH, () => {
      c.paths++
      return "~"
    })
    text = text.replace(EMAIL, () => {
      c.emails++
      return "<email>"
    })
    text = text.replace(URL_HOST, (m, lead: string, host: string) => {
      if (hostAllowed(host)) return m
      c.hosts++
      return `${lead}<host>`
    })
  } else {
    // The bundle redactor swaps the home path for <HOME>; count it as a path.
    c.paths += countOf(text, os.homedir())
  }
  const before = text.replaceAll(os.homedir(), "<HOME>")
  let out = SessionSupportBundle.redact(text, key) as string
  out = redactSecrets(out)
  out = out.replace(GITHUB_TOKEN, "<REDACTED>")
  if (out !== before) c.secrets++
  return out
}

function walk(input: unknown, key: string, strict: boolean, c: Counts): unknown {
  if (typeof input === "string") {
    let out = scrubString(input, key, strict, c)
    if (strict && key === "output" && out.length > MAX_TOOL_OUTPUT) {
      out = out.slice(0, MAX_TOOL_OUTPUT)
      c.truncated++
    }
    return out
  }
  if (Array.isArray(input)) return input.map((item) => walk(item, "", strict, c))
  if (input && typeof input === "object") {
    return Object.fromEntries(Object.entries(input).map(([k, v]) => [k, walk(v, k, strict, c)]))
  }
  if (input !== undefined && input !== null && key) {
    // Non-string values under a secret-shaped key are blanked by the bundle redactor.
    const out = SessionSupportBundle.redact(input, key)
    if (out !== input) {
      c.secrets++
      return out
    }
  }
  return input
}

function capEntries<E extends TraceEntry>(entries: E[]): E[] {
  if (entries.length <= MAX_ENTRIES) return entries
  const reserved = new Set<E>(
    entries.filter((e) => e.kind.startsWith("request.")).slice(-KEEP_TAIL_REQUESTS),
  )
  const head: E[] = []
  for (const e of entries) {
    if (head.length + reserved.size >= MAX_ENTRIES) break
    if (!reserved.has(e)) head.push(e)
  }
  const keep = new Set<E>([...head, ...reserved])
  return entries.filter((e) => keep.has(e))
}

function run(entries: TraceEntry[], turns: Turn[], strict: boolean) {
  const c: Counts = { secrets: 0, paths: 0, emails: 0, hosts: 0, truncated: 0 }
  const noDelta = entries.filter((e) => e.kind !== "part.delta")
  const capped = capEntries(noDelta)
  const dropped = entries.length - capped.length
  return {
    entries: capped.map((e) => walk(e, "", strict, c) as TraceEntry),
    turns: turns.map((t) => ({ ...t, text: scrubString(t.text, "", strict, c) })),
    counts: c,
    dropped,
  }
}

/** Redaction for a case saved on this machine. Prompts stay; secrets go. */
export function redactPrivate(entries: TraceEntry[], turns: Turn[]) {
  const r = run(entries, turns, false)
  const report: PrivateReport = {
    secrets: r.counts.secrets,
    paths: r.counts.paths,
    entries_dropped: r.dropped,
  }
  return { entries: r.entries, turns: r.turns, report }
}

/** Stricter pass for a shared suite: emails, home paths, outside hosts, long tool output. */
export function redactForExport(entries: TraceEntry[], turns: Turn[]) {
  const r = run(entries, turns, true)
  const report: ExportReport = {
    secrets: r.counts.secrets,
    paths: r.counts.paths,
    entries_dropped: r.dropped,
    emails: r.counts.emails,
    hosts: r.counts.hosts,
    truncated: r.counts.truncated,
  }
  return { entries: r.entries, turns: r.turns, report }
}

function tarList(file: string): Promise<string[]> {
  return new Promise((resolve, reject) => {
    const p = spawn("tar", ["-tzf", file])
    let out = ""
    p.stdout.on("data", (d) => (out += d))
    p.on("error", reject)
    p.on("close", (code) =>
      code === 0 ? resolve(out.split("\n").filter((l) => l && !l.endsWith("/"))) : reject(new Error(`tar -tzf failed (${code})`)),
    )
  })
}

function tarRead(file: string, entry: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const p = spawn("tar", ["-xOzf", file, entry])
    const chunks: Buffer[] = []
    let size = 0
    p.stdout.on("data", (d: Buffer) => {
      if (size >= MAX_TAR_FILE) return
      chunks.push(d.subarray(0, MAX_TAR_FILE - size))
      size += d.length
      if (size >= MAX_TAR_FILE) p.kill()
    })
    p.on("error", reject)
    p.on("close", () => resolve(Buffer.concat(chunks).toString("latin1")))
  })
}

function secretKind(text: string): string | undefined {
  if (GITHUB_TOKEN.test(text)) return (GITHUB_TOKEN.lastIndex = 0, "github_token")
  if (SK_KEY.test(text)) return "api_key"
  if (JWT.test(text)) return "jwt"
  if (BEARER.test(text)) return "bearer"
  if (redactSecrets(text) !== text) return "secret"
  return undefined
}

/** Scan a workspace tar.gz for secret-shaped text. Export refuses when there are hits. */
export async function scanWorkspaceTar(file: string): Promise<{ hits: { file: string; kind: string }[] }> {
  const hits: { file: string; kind: string }[] = []
  for (const entry of await tarList(file)) {
    const kind = secretKind(await tarRead(file, entry))
    if (kind) hits.push({ file: entry, kind })
  }
  return { hits }
}
