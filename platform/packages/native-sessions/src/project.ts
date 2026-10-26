import { existsSync, readFileSync, readdirSync } from "node:fs"
import { join } from "node:path"
import { getNativeSession } from "./catalog.js"
import { asString, contentText, readJsonlObjects, toolInput } from "./jsonl.js"
import { projectViaSessionMigrate } from "./migrate-bridge.js"
import type { CatalogOptions, HarnessId, NativeTranscript, PortableEvent, Role } from "./types.js"

function inert(partial: Omit<PortableEvent, "inert">): PortableEvent {
  return { ...partial, inert: true }
}

function pushMessage(events: PortableEvent[], role: Role, text: string, recordIndex: number, sourceId?: string) {
  const trimmed = text.trim()
  if (!trimmed) return
  events.push(inert({ kind: "message", role, text: trimmed, recordIndex, sourceId }))
}

function projectClaudeLike(path: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"]; lastEventId?: string } {
  const { records, malformed } = readJsonlObjects(path)
  const warnings: NativeTranscript["warnings"] = []
  if (malformed) warnings.push({ code: "malformed_jsonl", message: `${malformed} malformed lines skipped` })
  const events: PortableEvent[] = []
  let lastEventId: string | undefined
  for (let i = 0; i < records.length; i++) {
    const rec = records[i]!
    const type = asString(rec.type)
    const uuid = asString(rec.uuid)
    if (uuid) lastEventId = uuid
    if (rec.isSidechain === true || rec.isMeta === true) {
      events.push(inert({ kind: "opaque", recordIndex: i, sourceId: uuid }))
      continue
    }
    if (type === "user" || type === "assistant") {
      const message = rec.message as Record<string, unknown> | undefined
      const role: Role = type
      const text = contentText(message?.content ?? rec.content)
      const blocks = Array.isArray(message?.content) ? message.content : []
      for (const block of blocks) {
        if (typeof block === "string") pushMessage(events, role, block, i, uuid)
        if (!block || typeof block !== "object") continue
        const b = block as Record<string, unknown>
        if (b.type === "text") {
          pushMessage(events, role, asString(b.text) ?? "", i, uuid)
        } else if (b.type === "tool_use") {
          events.push(inert({ kind: "tool_call", role: "assistant", toolName: asString(b.name), toolId: asString(b.id), input: toolInput(b.input), recordIndex: i, sourceId: uuid }))
        } else if (b.type === "tool_result") {
          events.push(inert({ kind: "tool_result", role: "tool", toolId: asString(b.tool_use_id), text: contentText(b.content ?? b), recordIndex: i, sourceId: uuid }))
        } else if (b.type === "thinking") {
          events.push(inert({ kind: "thinking", role: "assistant", recordIndex: i, sourceId: uuid }))
        }
      }
      // Block content keeps its text in place between tool calls (above).
      if (blocks.length === 0) pushMessage(events, role, text, i, uuid)
    } else if (type === "system" && rec.subtype === "compact_boundary") {
      events.push(inert({ kind: "compaction", recordIndex: i, sourceId: uuid }))
    }
  }
  return { events, warnings, lastEventId }
}

function projectCodex(path: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"]; lastEventId?: string } {
  const { records, malformed } = readJsonlObjects(path)
  const warnings: NativeTranscript["warnings"] = []
  if (malformed) warnings.push({ code: "malformed_jsonl", message: `${malformed} malformed lines skipped` })
  const events: PortableEvent[] = []
  let lastEventId: string | undefined
  for (let i = 0; i < records.length; i++) {
    const rec = records[i]!
    const type = asString(rec.type)
    const payload = (rec.payload && typeof rec.payload === "object" ? rec.payload : rec) as Record<string, unknown>
    if (type === "session_meta") {
      lastEventId = asString(payload.id) ?? asString(payload.session_id)
      continue
    }
    if (type === "response_item") {
      const itemType = asString(payload.type)
      if (itemType === "message") {
        const role = (asString(payload.role) as Role | undefined) ?? "assistant"
        const text = contentText(payload.content)
        pushMessage(events, role === "system" ? "system" : role, text, i)
      } else if (itemType === "function_call") {
        events.push(inert({ kind: "tool_call", role: "assistant", toolName: asString(payload.name), toolId: asString(payload.call_id), input: toolInput(payload.arguments), recordIndex: i }))
      } else if (itemType === "function_call_output") {
        events.push(inert({ kind: "tool_result", role: "tool", toolId: asString(payload.call_id), text: asString(payload.output), recordIndex: i }))
      } else if (itemType === "reasoning") {
        events.push(inert({ kind: "thinking", recordIndex: i }))
      }
    } else if (type === "compacted") {
      events.push(inert({ kind: "compaction", recordIndex: i }))
    }
  }
  return { events, warnings, lastEventId }
}

function projectGrok(dir: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"]; lastEventId?: string } {
  const updates = join(dir, "updates.jsonl")
  const { records, malformed } = readJsonlObjects(updates)
  const warnings: NativeTranscript["warnings"] = []
  if (malformed) warnings.push({ code: "malformed_jsonl", message: `${malformed} malformed lines skipped` })
  const events: PortableEvent[] = []
  let lastEventId: string | undefined
  for (let i = 0; i < records.length; i++) {
    const rec = records[i]!
    const params = rec.params as Record<string, unknown> | undefined
    const update = params?.update as Record<string, unknown> | undefined
    const kind = asString(update?.sessionUpdate)
    const content = contentText(update?.content)
    const sid = asString(params?.sessionId)
    if (sid) lastEventId = sid
    if (kind === "user_message_chunk") pushMessage(events, "user", content, i)
    else if (kind === "agent_message_chunk") pushMessage(events, "assistant", content, i)
    else if (kind === "agent_thought_chunk") events.push(inert({ kind: "thinking", recordIndex: i }))
    else if (kind?.includes("tool")) events.push(inert({ kind: "tool_call", recordIndex: i, toolName: kind }))
  }
  return { events, warnings, lastEventId }
}

function projectKimi(dir: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"]; lastEventId?: string } {
  const wire = join(dir, "agents", "main", "wire.jsonl")
  const warnings: NativeTranscript["warnings"] = []
  if (!existsSync(wire)) return { events: [], warnings: [{ code: "missing_wire", message: "kimi wire.jsonl not found" }] }
  const { records, malformed } = readJsonlObjects(wire)
  if (malformed) warnings.push({ code: "malformed_jsonl", message: `${malformed} malformed lines skipped` })
  const events: PortableEvent[] = []
  let lastEventId: string | undefined
  for (let i = 0; i < records.length; i++) {
    const rec = records[i]!
    const type = asString(rec.type)
    if (type === "context.append_message") {
      const message = rec.message as Record<string, unknown> | undefined
      const role = (asString(message?.role) as Role | undefined) ?? "user"
      const text = contentText(message?.content)
      pushMessage(events, role, text, i)
      lastEventId = asString(rec.id) ?? lastEventId
    } else if (type === "context.append_loop_event") {
      const inner = rec.event as Record<string, unknown> | undefined
      const innerType = asString(inner?.type)
      if (innerType === "tool.call") events.push(inert({ kind: "tool_call", toolName: asString(inner?.name), recordIndex: i }))
      else if (innerType === "think") events.push(inert({ kind: "thinking", recordIndex: i }))
    }
  }
  return { events, warnings, lastEventId }
}

function projectRoleJsonl(path: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"] } {
  const { records, malformed } = readJsonlObjects(path)
  const warnings: NativeTranscript["warnings"] = []
  if (malformed) warnings.push({ code: "malformed_jsonl", message: `${malformed} malformed lines skipped` })
  const events: PortableEvent[] = []
  for (let i = 0; i < records.length; i++) {
    const rec = records[i]!
    const role = (asString(rec.role) as Role | undefined) ?? "user"
    if (role.startsWith("_")) {
      events.push(inert({ kind: "opaque", recordIndex: i }))
      continue
    }
    pushMessage(events, role, contentText(rec.content ?? rec.message), i)
  }
  return { events, warnings }
}

function projectOpenHands(dir: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"] } {
  const eventsDir = join(dir, "events")
  const warnings: NativeTranscript["warnings"] = []
  const events: PortableEvent[] = []
  if (!existsSync(eventsDir)) return { events, warnings: [{ code: "missing_events", message: "openhands events/ not found" }] }
  const files = readdirSync(eventsDir).filter((n) => n.endsWith(".json")).sort()
  files.forEach((name, i) => {
    try {
      const rec = JSON.parse(readFileSync(join(eventsDir, name), "utf8")) as Record<string, unknown>
      const kind = asString(rec.type) ?? asString(rec.event_type)
      const text = contentText(rec.content ?? rec.message ?? rec.args)
      if (kind?.toLowerCase().includes("user")) pushMessage(events, "user", text, i)
      else if (kind?.toLowerCase().includes("agent") || kind?.toLowerCase().includes("assistant")) pushMessage(events, "assistant", text, i)
      else events.push(inert({ kind: "opaque", recordIndex: i }))
    } catch {
      warnings.push({ code: "malformed_event", message: name })
    }
  })
  return { events, warnings }
}

function projectOpenCode(path: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"] } {
  const warnings: NativeTranscript["warnings"] = []
  const [dbPath, sessionId] = path.split("#") as [string, string | undefined]
  if (!sessionId) {
    return { events: [], warnings: [{ code: "need_session_id", message: "opencode path must be db#sessionId" }] }
  }
  try {
    const { Database } = require("bun:sqlite") as { Database: new (p: string, o?: { readonly?: boolean }) => { query: (s: string) => { all: (a?: string) => Record<string, unknown>[] } } }
    const db = new Database(dbPath, { readonly: true })
    const parts = db.query(`SELECT data FROM part WHERE session_id = ? ORDER BY time_created`).all(sessionId)
    const events: PortableEvent[] = []
    parts.forEach((row, i) => {
      let data: Record<string, unknown> = {}
      try {
        data = JSON.parse(String(row.data ?? "{}")) as Record<string, unknown>
      } catch {
        warnings.push({ code: "malformed_part", message: String(i) })
        return
      }
      const type = asString(data.type)
      if (type === "text") pushMessage(events, "user", asString(data.text) ?? "", i)
      else if (type === "tool") events.push(inert({ kind: "tool_call", toolName: asString(data.tool) ?? asString(data.name), recordIndex: i }))
      else if (type === "reasoning") events.push(inert({ kind: "thinking", recordIndex: i }))
      else if (asString(data.text)) pushMessage(events, "assistant", asString(data.text)!, i)
    })
    return { events, warnings }
  } catch (err) {
    return { events: [], warnings: [{ code: "sqlite_unavailable", message: String(err) }] }
  }
}

function projectCline(dir: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"]; lastEventId?: string } {
  // api_conversation_history.json: OpenAI-style message array. Content may be a
  // plain string or an array of {type: "text", ...} parts; assistant tool calls
  // ride in tool_calls, results as role "tool" messages.
  const history = join(dir, "api_conversation_history.json")
  const warnings: NativeTranscript["warnings"] = []
  if (!existsSync(history)) return { events: [], warnings: [{ code: "missing_history", message: "cline api_conversation_history.json not found" }] }
  let parsed: unknown
  try {
    parsed = JSON.parse(readFileSync(history, "utf8"))
  } catch {
    return { events: [], warnings: [{ code: "malformed_json", message: "cline api_conversation_history.json is not valid JSON" }] }
  }
  if (!Array.isArray(parsed)) return { events: [], warnings: [{ code: "malformed_json", message: "cline api_conversation_history.json is not an array" }] }
  const events: PortableEvent[] = []
  let lastEventId: string | undefined
  parsed.forEach((raw, i) => {
    const rec = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>
    const role = (asString(rec.role) as Role | undefined) ?? "user"
    lastEventId = String(i)
    if (role === "tool") {
      events.push(inert({ kind: "tool_result", role: "tool", toolId: asString(rec.tool_call_id), text: contentText(rec.content), recordIndex: i }))
      return
    }
    if (role !== "user" && role !== "assistant" && role !== "system") {
      events.push(inert({ kind: "opaque", recordIndex: i }))
      return
    }
    if (Array.isArray(rec.tool_calls)) {
      for (const call of rec.tool_calls) {
        const c = (call && typeof call === "object" ? call : {}) as Record<string, unknown>
        const fn = (c.function && typeof c.function === "object" ? c.function : {}) as Record<string, unknown>
        events.push(inert({ kind: "tool_call", role: "assistant", toolName: asString(fn.name) ?? asString(c.name), toolId: asString(c.id), input: toolInput(fn.arguments), recordIndex: i }))
      }
    }
    pushMessage(events, role, contentText(rec.content), i)
  })
  return { events, warnings, lastEventId }
}

function projectAmp(path: string): { events: PortableEvent[]; warnings: NativeTranscript["warnings"]; lastEventId?: string } {
  // threads/T-*.json: { messages: [{role, content: [{type: "text"|"tool_use"|"tool_result", ...}]}] }
  const warnings: NativeTranscript["warnings"] = []
  let parsed: Record<string, unknown>
  try {
    parsed = JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>
  } catch {
    return { events: [], warnings: [{ code: "malformed_json", message: "amp thread is not valid JSON" }] }
  }
  if (!Array.isArray(parsed.messages)) return { events: [], warnings: [{ code: "missing_messages", message: "amp thread has no messages array" }] }
  const events: PortableEvent[] = []
  let lastEventId: string | undefined
  parsed.messages.forEach((raw, i) => {
    const rec = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>
    const role = (asString(rec.role) as Role | undefined) ?? "user"
    lastEventId = asString(rec.id) ?? String(i)
    const blocks = Array.isArray(rec.content) ? rec.content : []
    if (blocks.length === 0) {
      if (typeof rec.content === "string") pushMessage(events, role, rec.content, i)
      else events.push(inert({ kind: "opaque", recordIndex: i }))
      return
    }
    for (const blockRaw of blocks) {
      const block = (blockRaw && typeof blockRaw === "object" ? blockRaw : {}) as Record<string, unknown>
      const type = asString(block.type)
      if (type === "text") pushMessage(events, role, asString(block.text) ?? "", i)
      else if (type === "tool_use") events.push(inert({ kind: "tool_call", role: "assistant", toolName: asString(block.name), toolId: asString(block.id), input: toolInput(block.input), recordIndex: i }))
      else if (type === "tool_result")
        events.push(inert({ kind: "tool_result", role: "tool", toolId: asString(block.tool_use_id), text: contentText(block.content), recordIndex: i }))
      else if (type === "thinking") events.push(inert({ kind: "thinking", role: "assistant", recordIndex: i }))
    }
  })
  return { events, warnings, lastEventId }
}

export function showNativeSession(harness: HarnessId, sessionId: string, opts: CatalogOptions = {}): NativeTranscript {
  const session = getNativeSession(harness, sessionId, opts)
  if (!session) {
    return {
      session: {
        harness,
        sessionId,
        path: "",
        updatedAt: 0,
        fingerprint: "",
        installed: false,
        reader: "jsonl",
        projectable: false,
      },
      events: [],
      warnings: [{ code: "not_found", message: `no ${harness} session ${sessionId}` }],
    }
  }
  if (!session.projectable) {
    const bridged = projectViaSessionMigrate(harness, session.path)
    if (bridged && bridged.length > 0) {
      return {
        session: { ...session, projectable: true },
        events: bridged,
        warnings: [{ code: "session_migrate_bridge", message: "projected via vendored session-migrate" }],
      }
    }
    return {
      session,
      events: [],
      warnings: [{ code: "not_projectable", message: `${harness} listing is available; full projection uses the vendored session-migrate reader` }],
    }
  }
  let projected: { events: PortableEvent[]; warnings: NativeTranscript["warnings"]; lastEventId?: string }
  switch (harness) {
    case "claude":
    case "gizzi":
    case "qwen":
    case "cursor":
      projected = projectClaudeLike(session.path)
      break
    case "codex":
      projected = projectCodex(session.path)
      break
    case "grok":
      projected = projectGrok(session.path)
      break
    case "kimi":
      projected = projectKimi(session.path)
      break
    case "kimi-cli":
    case "copilot":
    case "vibe":
    case "gemini":
    case "droid":
    case "pi":
    case "omp":
    case "muse":
      projected = session.path.endsWith(".jsonl") ? projectRoleJsonl(session.path) : projectClaudeLike(existsSync(join(session.path, "messages.jsonl")) ? join(session.path, "messages.jsonl") : session.path)
      break
    case "openhands":
      projected = projectOpenHands(session.path)
      break
    case "cline":
      projected = projectCline(session.path)
      break
    case "amp":
      projected = projectAmp(session.path)
      break
    case "opencode":
      projected = projectOpenCode(session.path)
      break
    default:
      projected = { events: [], warnings: [{ code: "no_reader", message: harness }] }
  }
  if (projected.events.length === 0) {
    const bridged = projectViaSessionMigrate(harness, session.path)
    if (bridged && bridged.length > 0) {
      projected = {
        events: bridged,
        warnings: [...projected.warnings, { code: "session_migrate_bridge", message: "projected via vendored session-migrate" }],
        lastEventId: bridged.map((e) => e.sourceId).filter(Boolean).at(-1),
      }
    }
  }
  if (projected.lastEventId) session.lastEventId = projected.lastEventId
  return { session, events: projected.events, warnings: projected.warnings }
}

export function eventsAfter(transcript: NativeTranscript, afterEventId?: string): PortableEvent[] {
  if (!afterEventId) return transcript.events
  const idx = transcript.events.findIndex((e) => e.sourceId === afterEventId)
  if (idx < 0) return transcript.events
  return transcript.events.slice(idx + 1)
}
