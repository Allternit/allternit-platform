import { randomUUID } from "node:crypto"
import { spawnSync } from "node:child_process"
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { tmpdir } from "node:os"
import { fileURLToPath } from "node:url"
import { encodeClaudeCwd, encodeGrokCwd, HARNESS_BY_ID, harnessHome } from "./harness.js"
import type { HarnessId, NativeExport, PortableEvent } from "./types.js"

export interface ExportInput {
  harness: HarnessId
  events: PortableEvent[]
  cwd: string
  title?: string
  home?: string
  /** Refuse to write this path (the origin file). */
  forbidPath?: string
}

function assertNewPath(path: string, forbidPath?: string) {
  if (forbidPath && resolveEq(path, forbidPath)) {
    throw new Error("refusing to overwrite origin native session")
  }
  if (existsSync(path)) {
    throw new Error(`export target already exists: ${path}`)
  }
}

function resolveEq(a: string, b: string) {
  return a.replace(/\\/g, "/").replace(/\/+$/, "") === b.replace(/\\/g, "/").replace(/\/+$/, "")
}

function writeJsonl(path: string, rows: unknown[]) {
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, rows.map((row) => JSON.stringify(row)).join("\n") + "\n", { flag: "wx" })
}

function claudeRecords(sessionId: string, cwd: string, events: PortableEvent[], title?: string) {
  const rows: unknown[] = []
  if (title) {
    rows.push({ type: "custom-title", customTitle: title, sessionId })
  }
  let parent: string | null = null
  for (const event of events) {
    if (event.kind !== "message" && event.kind !== "tool_call" && event.kind !== "tool_result") continue
    const uuid = randomUUID()
    const role = event.role === "assistant" || event.kind === "tool_call" ? "assistant" : "user"
    const text =
      event.kind === "tool_call"
        ? `[tool ${event.toolName ?? "call"}] ${event.text ?? ""}`.trim()
        : event.kind === "tool_result"
          ? `[tool result] ${event.text ?? ""}`.trim()
          : event.text ?? ""
    if (!text) continue
    rows.push({
      type: role,
      uuid,
      parentUuid: parent,
      sessionId,
      timestamp: new Date().toISOString(),
      cwd,
      message: { role, content: text },
    })
    parent = uuid
  }
  return rows
}

function rootFor(harness: HarnessId, input: ExportInput): string {
  if (input.home) return HARNESS_BY_ID.get(harness)!.defaultHome(input.home)
  return harnessHome(harness)
}

function exportClaudeLike(harness: "claude" | "gizzi" | "qwen", input: ExportInput, sessionId: string): NativeExport {
  const home = rootFor(harness, input)
  const encoded = encodeClaudeCwd(input.cwd)
  const file =
    harness === "qwen"
      ? join(home, "projects", encoded, "chats", `${sessionId}.jsonl`)
      : join(home, "projects", encoded, `${sessionId}.jsonl`)
  assertNewPath(file, input.forbidPath)
  writeJsonl(file, claudeRecords(sessionId, input.cwd, input.events, input.title))
  const hint = HARNESS_BY_ID.get(harness)?.resumeHint.replace("<id>", sessionId) ?? `${harness} --resume ${sessionId}`
  return { harness, sessionId, path: file, resumeHint: hint, at: Date.now() }
}

function exportCodex(input: ExportInput, sessionId: string): NativeExport {
  const now = new Date()
  const stamp = now.toISOString().replace(/:/g, "-").replace(/\.\d+Z$/, "")
  const home = rootFor("codex", input)
  const file = join(
    home,
    "sessions",
    String(now.getUTCFullYear()),
    String(now.getUTCMonth() + 1).padStart(2, "0"),
    String(now.getUTCDate()).padStart(2, "0"),
    `rollout-${stamp}-${sessionId}.jsonl`,
  )
  assertNewPath(file, input.forbidPath)
  const rows: unknown[] = [
    {
      timestamp: now.toISOString(),
      type: "session_meta",
      payload: {
        id: sessionId,
        session_id: sessionId,
        cwd: input.cwd,
        originator: "allternit-native-export",
        source: "cli",
      },
    },
  ]
  for (const event of input.events) {
    if (event.kind !== "message") continue
    const role = event.role === "assistant" ? "assistant" : "user"
    const text = event.text ?? ""
    if (!text) continue
    rows.push({
      timestamp: now.toISOString(),
      type: "response_item",
      payload: {
        type: "message",
        role,
        content: [{ type: role === "user" ? "input_text" : "output_text", text }],
      },
    })
  }
  writeJsonl(file, rows)
  return { harness: "codex", sessionId, path: file, resumeHint: `codex resume ${sessionId}`, at: Date.now() }
}

function exportGrok(input: ExportInput, sessionId: string): NativeExport {
  const home = rootFor("grok", input)
  const dir = join(home, "sessions", encodeGrokCwd(input.cwd), sessionId)
  const updates = join(dir, "updates.jsonl")
  assertNewPath(updates, input.forbidPath)
  mkdirSync(dir, { recursive: true })
  writeFileSync(
    join(dir, "summary.json"),
    JSON.stringify({ info: { id: sessionId, cwd: input.cwd }, session_summary: input.title ?? "" }),
    { flag: "wx" },
  )
  const rows: unknown[] = []
  let ts = Math.floor(Date.now() / 1000)
  for (const event of input.events) {
    if (event.kind !== "message" || !event.text) continue
    const kind = event.role === "assistant" ? "agent_message_chunk" : "user_message_chunk"
    rows.push({
      timestamp: ts++,
      method: "session/update",
      params: {
        sessionId,
        update: { sessionUpdate: kind, content: { type: "text", text: event.text } },
      },
    })
  }
  writeFileSync(updates, rows.map((row) => JSON.stringify(row)).join("\n") + "\n", { flag: "wx" })
  return { harness: "grok", sessionId, path: dir, resumeHint: `grok --resume ${sessionId}`, at: Date.now() }
}

function exportCopilot(input: ExportInput, sessionId: string): NativeExport {
  const home = rootFor("copilot", input)
  const events = join(home, "session-state", sessionId, "events.jsonl")
  assertNewPath(events, input.forbidPath)
  const rows = input.events
    .filter((e) => e.kind === "message" && e.text)
    .map((e) => ({ type: "message", role: e.role ?? "user", content: e.text }))
  writeJsonl(events, rows)
  return { harness: "copilot", sessionId, path: events, resumeHint: `copilot --resume=${sessionId}`, at: Date.now() }
}

function exportKimiCli(input: ExportInput, sessionId: string): NativeExport {
  const home = rootFor("kimi-cli", input)
  const file = join(home, "sessions", "allternit", sessionId, "context.jsonl")
  assertNewPath(file, input.forbidPath)
  const rows = input.events
    .filter((e) => e.kind === "message" && e.text)
    .map((e) => ({ role: e.role ?? "user", content: e.text }))
  writeJsonl(file, rows)
  return { harness: "kimi-cli", sessionId, path: file, resumeHint: "kimi", at: Date.now() }
}

function exportCline(input: ExportInput, sessionId: string): NativeExport {
  const home = rootFor("cline", input)
  const dir = join(home, "tasks", sessionId)
  const history = join(dir, "api_conversation_history.json")
  assertNewPath(history, input.forbidPath)
  const rows: unknown[] = []
  for (const event of input.events) {
    if (event.kind === "message" && event.text) {
      const role = event.role === "assistant" ? "assistant" : event.role === "system" ? "system" : "user"
      rows.push({ role, content: event.text })
    } else if (event.kind === "tool_call") {
      rows.push({ role: "assistant", content: "", tool_calls: [{ id: event.toolId ?? randomUUID(), type: "function", function: { name: event.toolName ?? "call", arguments: JSON.stringify(event.input ?? {}) } }] })
    } else if (event.kind === "tool_result") {
      rows.push({ role: "tool", tool_call_id: event.toolId ?? "", content: event.text ?? "" })
    }
  }
  mkdirSync(dir, { recursive: true })
  writeFileSync(history, JSON.stringify(rows, null, 2), { flag: "wx" })
  return { harness: "cline", sessionId, path: history, resumeHint: "cline (reopen the task in VS Code)", at: Date.now() }
}

function exportAmp(input: ExportInput, sessionId: string): NativeExport {
  const home = rootFor("amp", input)
  const threadId = `T-${sessionId}`
  const file = join(home, "threads", `${threadId}.json`)
  assertNewPath(file, input.forbidPath)
  const messages: unknown[] = []
  for (const event of input.events) {
    if (event.kind === "message" && event.text) {
      messages.push({ role: event.role === "assistant" ? "assistant" : "user", content: [{ type: "text", text: event.text }] })
    } else if (event.kind === "tool_call") {
      messages.push({ role: "assistant", content: [{ type: "tool_use", id: event.toolId ?? randomUUID(), name: event.toolName ?? "call", input: event.input ?? {} }] })
    } else if (event.kind === "tool_result") {
      messages.push({ role: "user", content: [{ type: "tool_result", tool_use_id: event.toolId ?? "", content: event.text ?? "" }] })
    }
  }
  mkdirSync(join(home, "threads"), { recursive: true })
  writeFileSync(file, JSON.stringify({ messages }, null, 2), { flag: "wx" })
  return { harness: "amp", sessionId: threadId, path: file, resumeHint: `amp threads continue ${threadId}`, at: Date.now() }
}

export const DIRECT_EXPORT_HARNESSES: HarnessId[] = ["claude", "gizzi", "qwen", "codex", "grok", "copilot", "kimi-cli", "cline", "amp"]

function exportViaSessionMigrate(input: ExportInput): NativeExport {
  const script = join(dirname(fileURLToPath(import.meta.url)), "..", "scripts", "export_native.py")
  if (!existsSync(script)) throw new Error("session-migrate export helper missing")
  const staging = mkdtempSync(join(tmpdir(), "allternit-native-export-"))
  const claudeFile = join(staging, `${randomUUID()}.jsonl`)
  writeFileSync(claudeFile, claudeRecords(randomUUID(), input.cwd, input.events, input.title).map((row) => JSON.stringify(row)).join("\n") + "\n")
  const dest = join(staging, "out", randomUUID())
  mkdirSync(dirname(dest), { recursive: true })
  const result = spawnSync("python3", [script, claudeFile, input.harness, dest, input.cwd], {
    encoding: "utf8",
    timeout: 30_000,
    maxBuffer: 16 * 1024 * 1024,
  })
  if (result.status !== 0) {
    let detail = result.stdout || result.stderr || `session-migrate export failed for ${input.harness}`
    try {
      const parsed = JSON.parse(result.stdout || "{}") as { error?: string }
      if (parsed.error) detail = parsed.error
    } catch { /* keep */ }
    throw new Error(detail)
  }
  const parsed = JSON.parse(result.stdout) as { error?: string; sessionId?: string; path?: string }
  if (parsed.error || !parsed.sessionId || !parsed.path) {
    throw new Error(parsed.error || "session-migrate export returned no session")
  }
  if (input.forbidPath && resolveEq(parsed.path, input.forbidPath)) {
    throw new Error("refusing to overwrite origin native session")
  }
  const hint = HARNESS_BY_ID.get(input.harness)?.resumeHint.replace("<id>", parsed.sessionId) ?? `${input.harness} --resume ${parsed.sessionId}`
  return { harness: input.harness, sessionId: parsed.sessionId, path: parsed.path, resumeHint: hint, at: Date.now() }
}

/** Write a NEW native session. Never overwrites an existing file or the origin path. */
export function exportPortableSession(input: ExportInput): NativeExport {
  const sessionId = randomUUID()
  switch (input.harness) {
    case "claude":
    case "gizzi":
    case "qwen":
      return exportClaudeLike(input.harness, input, sessionId)
    case "codex":
      return exportCodex(input, sessionId)
    case "grok":
      return exportGrok(input, sessionId)
    case "copilot":
      return exportCopilot(input, sessionId)
    case "kimi-cli":
      return exportKimiCli(input, sessionId)
    case "cline":
      return exportCline(input, sessionId)
    case "amp":
      return exportAmp(input, sessionId)
    default:
      return exportViaSessionMigrate(input)
  }
}
