/**
 * Human rendering for engine replies. Pure string functions (no I/O) so the
 * CLI commands and the tests share them. `--json` never goes through here:
 * that path prints the engine's document unchanged.
 */
import type { Board, Delivery, FactoryAgent, NodeCard } from "@/cli/factory/types"

const ESC = "\x1b["
export const C = {
  reset: `${ESC}0m`,
  bold: `${ESC}1m`,
  dim: `${ESC}90m`,
  // Gizzi coral (#D97757), the TUI's theme.gizzi.
  gizzi: `${ESC}38;2;217;119;87m`,
  green: `${ESC}92m`,
  yellow: `${ESC}93m`,
  red: `${ESC}91m`,
  blue: `${ESC}94m`,
}

export interface RenderOptions {
  color?: boolean
}

function paint(on: boolean, code: string, text: string) {
  return on ? `${code}${text}${C.reset}` : text
}

const HARNESS_NAMES: Record<string, string> = {
  gizzi: "Gizzi",
  claude: "Claude Code",
  codex: "Codex",
  kimi: "Kimi",
  grok: "Grok",
  agy: "agy",
}

const VENDOR_NAMES: Record<string, string> = {
  chatgpt: "ChatGPT",
  openai: "ChatGPT",
  claude: "Claude",
  anthropic: "Claude",
  gemini: "Gemini",
  grok: "Grok",
  copilot: "Copilot",
}

const LANE_NAMES: Record<string, string> = {
  official: "official",
  channel: "channel",
  ui_bridge: "UI bridge",
  local: "local",
}

function titleCase(s: string) {
  return s ? s[0]!.toUpperCase() + s.slice(1) : s
}

/** "Hosted · Gizzi", "Terminal · Claude Code", "Vendor · ChatGPT · official". */
export function bindingBadge(agent: Pick<FactoryAgent, "binding">): string {
  const b = agent.binding
  if (!b?.type) return "—"
  if (b.type === "hosted") return "Hosted · Gizzi"
  if (b.type === "terminal") {
    const h = b.harness ? (HARNESS_NAMES[b.harness] ?? titleCase(b.harness)) : "terminal"
    return `Terminal · ${h}`
  }
  const v = b.vendor ? (VENDOR_NAMES[b.vendor.toLowerCase()] ?? titleCase(b.vendor)) : "vendor"
  const lane = b.lane ? ` · ${LANE_NAMES[b.lane] ?? b.lane}` : ""
  return `Vendor · ${v}${lane}`
}

export function proofText(proof: { proven: number; total: number } | null | undefined): string {
  if (!proof) return "—"
  return `${proof.proven}/${proof.total}`
}

export function stateColor(state: string | undefined): keyof typeof C {
  switch (state) {
    case "working":
    case "checking":
      return "blue"
    case "done":
      return "green"
    case "needs_you":
    case "blocked":
      return "yellow"
    case "failed":
      return "red"
    default:
      return "dim"
  }
}

export function agentAddress(a: FactoryAgent): string {
  return a.address || (a.team ? `${a.slug ?? a.id}@${a.team}` : (a.slug ?? a.name ?? a.id))
}

/** Pad to a visible width, ignoring ANSI escapes. */
function pad(text: string, width: number) {
  const visible = text.replace(/\x1b\[[0-9;]*m/g, "")
  return visible.length >= width ? text : text + " ".repeat(width - visible.length)
}

function table(header: string[], rows: string[][], color: boolean): string {
  const widths = header.map((h, i) => Math.max(h.length, ...rows.map((r) => (r[i] ?? "").replace(/\x1b\[[0-9;]*m/g, "").length)))
  const head = header.map((h, i) => paint(color, C.dim, pad(h, widths[i]!))).join("  ")
  const body = rows.map((r) => r.map((cell, i) => (i === r.length - 1 ? cell : pad(cell, widths[i]!))).join("  "))
  return [head, ...body].join("\n")
}

export function renderAgents(agents: FactoryAgent[], opts: RenderOptions = {}): string {
  const color = opts.color ?? false
  if (agents.length === 0) return "No bots are running. Start a team with `gizzi agents up`."
  const rows = agents.map((a) => [
    paint(color, C.bold, agentAddress(a)),
    bindingBadge(a),
    paint(color, C[stateColor(a.state)], a.state ?? "—"),
    a.currentNode ? a.currentNode.title : "—",
    proofText(a.proof),
  ])
  return table(["BOT", "BINDING", "STATE", "NODE", "PROOF"], rows, color)
}

function nodeLine(n: NodeCard, color: boolean) {
  const status = paint(color, C[stateColor(n.status)], n.status)
  const who = n.assignee ? `  ${n.assignee}` : ""
  return `  ${n.nodeId}  ${n.title}  ${status}  ${proofText(n.proof)}${paint(color, C.dim, who)}`
}

export function renderBoard(board: Board, opts: RenderOptions = {}): string {
  const color = opts.color ?? false
  const out: string[] = []
  out.push(paint(color, C.gizzi + C.bold, board.campaign.title))
  if (board.campaign.intent) out.push(paint(color, C.dim, board.campaign.intent))
  out.push(`Proven ${board.summary.proven.k}/${board.summary.proven.n}`)
  const section = (title: string, nodes: NodeCard[]) => {
    out.push("")
    out.push(paint(color, C.bold, `${title} (${nodes.length})`))
    if (nodes.length === 0) out.push(paint(color, C.dim, "  —"))
    for (const n of nodes) out.push(nodeLine(n, color))
  }
  section("Needs you", board.summary.needsYou)
  section("Now", board.summary.now)
  section("Next", board.summary.next)
  return out.join("\n")
}

export function renderDelivery(d: Delivery, opts: RenderOptions = {}): string {
  const color = opts.color ?? false
  const tone = d.state === "verified" ? C.green : d.state === "failed" ? C.red : C.yellow
  const ticket = d.ticket ? ` ${d.ticket}` : ""
  const detail = d.detail ? `\n  ${d.detail}` : ""
  return `${paint(color, tone, d.state)} → ${d.to} via ${d.via}${ticket}${detail}`
}

function isDelivery(v: any): v is Delivery {
  return v && typeof v === "object" && typeof v.via === "string" && typeof v.state === "string" && "to" in v
}

function isBoard(v: any): v is Board {
  return v && typeof v === "object" && v.campaign && v.summary && Array.isArray(v.waves)
}

function scalar(v: unknown): string {
  if (v === null || v === undefined) return "—"
  if (typeof v === "object") {
    const s = JSON.stringify(v)
    return s.length > 60 ? s.slice(0, 57) + "…" : s
  }
  return String(v)
}

function renderObjectList(items: Record<string, unknown>[], color: boolean): string {
  if (items.length === 0) return "(none)"
  const keys = Object.keys(items[0]!).filter((k) => {
    const v = items[0]![k]
    return v === null || typeof v !== "object"
  })
  const cols = keys.slice(0, 6)
  if (cols.length === 0) return items.map((i) => JSON.stringify(i)).join("\n")
  return table(
    cols.map((c) => c.toUpperCase()),
    items.map((i) => cols.map((c) => scalar(i[c]))),
    color,
  )
}

/**
 * Render any engine document. Known shapes (agents, board, delivery) get
 * their own layout; everything else falls back to a table for arrays of
 * records or `key: value` lines, so a verb Gizzi has no custom view for
 * still prints what the engine said — never invented content.
 */
export function renderDocument(doc: unknown, opts: RenderOptions = {}): string {
  const color = opts.color ?? false
  if (doc === null || doc === undefined) return "Done."
  if (typeof doc !== "object") return String(doc)
  if (Array.isArray(doc)) {
    if (doc.every((x) => x && typeof x === "object" && !Array.isArray(x))) return renderObjectList(doc as any, color)
    return doc.map(scalar).join("\n")
  }
  const obj = doc as Record<string, unknown>
  if (Array.isArray(obj.agents) && Object.keys(obj).length <= 2) return renderAgents(obj.agents as FactoryAgent[], opts)
  if (isBoard(obj)) return renderBoard(obj, opts)
  if (isDelivery(obj)) return renderDelivery(obj, opts)
  if (Array.isArray(obj.deliveries)) return (obj.deliveries as Delivery[]).map((d) => renderDelivery(d, opts)).join("\n") || "(none)"
  const keys = Object.keys(obj)
  if (keys.length === 1 && Array.isArray(obj[keys[0]!])) return renderDocument(obj[keys[0]!], opts)
  if (typeof obj.message === "string" && keys.length === 1) return obj.message
  return keys
    .map((k) => {
      const v = obj[k]
      if (Array.isArray(v) && v.every((x) => x && typeof x === "object")) {
        return `${paint(color, C.bold, k)}\n${renderObjectList(v as any, color)
          .split("\n")
          .map((l) => "  " + l)
          .join("\n")}`
      }
      return `${paint(color, C.dim, k + ":")} ${scalar(v)}`
    })
    .join("\n")
}

export function renderError(err: { fact: string; action: string | null }, opts: RenderOptions = {}): string {
  const color = opts.color ?? false
  const lines = [paint(color, C.red + C.bold, "✗ ") + err.fact]
  if (err.action) lines.push(paint(color, C.dim, "  " + err.action))
  return lines.join("\n")
}
