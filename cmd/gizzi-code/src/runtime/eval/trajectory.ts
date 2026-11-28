// SessionTrace entries -> eval Trajectory (CONTRACT.md section 4, "Adapter rules").
import { SessionTrace } from "@/runtime/session/trace"
import type { Trajectory } from "./schema"

/** Tools whose calls change the outside world. Anything starting `mcp__` counts too. */
export const WRITE_TOOLS: ReadonlySet<string> = new Set(["write", "edit", "patch", "multiedit", "bash"])

export function isWriteTool(tool: string): boolean {
  return WRITE_TOOLS.has(tool) || tool.startsWith("mcp__")
}

type Rec = Record<string, unknown>
const rec = (v: unknown): Rec => (v && typeof v === "object" ? (v as Rec) : {})
const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0)

/**
 * Tokens for one request.completed. Total = input + output + reasoning + cache.read
 * (cache reads included, cache writes not). `tokens.total` is ignored: providers fill it inconsistently.
 */
function tokenTotal(tokens: unknown): number {
  const t = rec(tokens)
  return num(t.input) + num(t.output) + num(t.reasoning) + num(rec(t.cache).read)
}

export function toTrajectory(entries: SessionTrace.Entry[]): Trajectory {
  let agent = "main"
  const order: string[] = []
  const last = new Map<string, { tool: string; status: string; node: string }>()
  const userMessages = new Set<string>()
  let text: string | undefined
  let guard: string | undefined
  let failed = false
  let completed = false
  let cost = 0
  let tokens = 0

  for (const e of entries) {
    const d = rec(e.data)
    switch (e.kind) {
      case "request.started":
        if (typeof d.agent === "string" && d.agent) agent = d.agent
        break
      case "message.updated":
        if (d.role === "user" && e.messageID) userMessages.add(e.messageID)
        break
      case "part.updated":
        if (d.type === "tool" && typeof d.callID === "string") {
          const status = String(rec(d.state).status ?? "")
          const prev = last.get(d.callID)
          if (!prev) order.push(d.callID)
          last.set(d.callID, { tool: String(d.tool ?? ""), status, node: prev?.node ?? agent })
        } else if (d.type === "text" && !d.synthetic && typeof d.text === "string" && !(e.messageID && userMessages.has(e.messageID))) {
          text = d.text
        }
        break
      case "guardrail.tripped":
        if (typeof d.kind === "string") guard = d.kind
        break
      case "session.error":
      case "request.failed":
        failed = true
        break
      case "request.completed":
        completed = true
        cost += num(d.cost)
        tokens += tokenTotal(d.tokens)
        break
    }
  }

  const steps: Trajectory["steps"] = []
  const effects: Trajectory["effects"] = []
  for (const id of order) {
    const c = last.get(id)!
    if (c.status !== "completed" && c.status !== "error") continue
    const ok = c.status === "completed"
    steps.push({ node: c.node, primitive: c.tool, ok })
    if (isWriteTool(c.tool)) effects.push({ node: c.node, tool: c.tool, committed: ok })
  }

  const status = guard !== undefined ? "stopped" : failed ? "failed" : completed ? "completed" : "unknown"
  return {
    status,
    verifier: null,
    steps,
    effects,
    texts: text === undefined ? [] : [text],
    stopped_at: guard !== undefined ? guard : null,
    missing: [],
    cost_usd: cost,
    tokens,
    latency_ms: entries.length ? Math.max(0, entries[entries.length - 1]!.time - entries[0]!.time) : 0,
  }
}

/** Read a whole session's trace (list pages at 5000 max) and adapt it. */
export function recordedTrajectory(sessionID: string): Trajectory {
  const all: SessionTrace.Entry[] = []
  let after = 0
  for (;;) {
    const page = SessionTrace.list({ sessionID, after, limit: 5000 })
    all.push(...page)
    if (page.length < 5000) break
    after = page[page.length - 1]!.sequence
  }
  return toTrajectory(all)
}
