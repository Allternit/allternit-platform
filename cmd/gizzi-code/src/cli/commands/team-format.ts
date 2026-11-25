/** Pure roster formatting for `gizzi team list` (no CLI imports, so it unit-tests cleanly). */
type Session = { address: string; harness: string; status?: string; doing?: string; repo?: string; branch?: string; tier?: string; unread?: number; botId?: string }
type Machine = { name: string; online: boolean; sessions: Session[]; bots: Session[] }
type Person = { display: string; me: boolean; machines: Machine[]; unread: number; held: number }
export type Roster = { org?: string | null; people: Person[] }

export function formatRoster(r: Roster): string[] {
  if (!r.people.length) return ["No teammates reporting. Invite teammates in your organization to see their sessions."]
  const out: string[] = []
  for (const p of r.people) {
    const badges = [p.unread ? `${p.unread} unread` : "", p.held ? `${p.held} held` : ""].filter(Boolean).join(", ")
    out.push(`${p.display}${p.me ? " (you)" : ""}${badges ? `  [${badges}]` : ""}`)
    for (const m of p.machines) {
      out.push(`  ${m.online ? "●" : "○"} ${m.name}`)
      if (!m.sessions.length && !m.bots.length) out.push("      no live sessions")
      for (const s of [...m.sessions, ...m.bots]) {
        const where = [s.repo, s.branch].filter(Boolean).join("@")
        const dot = s.status === "busy" ? "busy" : s.status === "idle" ? "idle" : "offline"
        out.push(`      ${s.address}  ${s.harness}${s.botId ? " (bot)" : ""}  ${dot}${s.tier ? `  ${s.tier}` : ""}${where ? `  ${where}` : ""}`)
        if (s.doing) out.push(`        ${s.doing}`)
      }
    }
  }
  return out
}

// ── Team map (GET /api/v1/team/map) ────────────────────────────────────────────
export type MapAppRef = { kind: string; ref: string; name: string }
export type MapNode = { id: string; kind: "person" | "bot"; name: string; role?: string | null; is_main?: boolean | null; brain?: string | null; state: string }
export type MapEdge = { from: string; to: string; kind: "reports_to" | "owns" | "parent" }
export type MapGrant = { bot: string; app: MapAppRef; level: string }
export type MapRule = { id: string; text: string }
export type MapAsk = { id: string; title: string; actor: string | null; approver_user_id: string | null; rule_id: string | null; resolve?: unknown }
export type MapJob = { job_id: string; actor: string | null; waiting_on: string | null; title: string; state: string }
export type TeamMap = {
  org: { id: string; name: string } | null
  me: string
  nodes: MapNode[]
  edges: MapEdge[]
  grants: MapGrant[]
  rules: MapRule[]
  asks: MapAsk[]
  jobs: MapJob[]
  partial?: string[]
}

const STATE_LABEL: Record<string, string> = {
  idle: "Idle",
  working: "Working",
  needs_input: "Needs input",
  waiting_approval: "Waiting on approval",
  offline: "Offline",
}

export function formatTeamMap(m: TeamMap): string[] {
  const byId = new Map(m.nodes.map((n) => [n.id, n]))
  const first = (id: string | null | undefined) => (id ? (byId.get(id)?.name ?? id.replace(/^[pbv]:(usr_)?/, "")) : null)
  const children = (id: string, kind: MapEdge["kind"]) =>
    m.edges.filter((e) => e.from === id && e.kind === kind).map((e) => byId.get(e.to)).filter((n): n is MapNode => !!n)
  const out: string[] = []
  if (!m.nodes.length) return ["No team yet. Invite teammates in your organization to see the map."]
  if (m.org) out.push(m.org.name)
  const botLines = (b: MapNode, depth: number) => {
    const pad = "  ".repeat(depth)
    const brain = b.brain ? `  brain ${b.brain}` : ""
    out.push(`${pad}${b.is_main ? "★ " : ""}${b.name}${brain}  ${STATE_LABEL[b.state] ?? b.state}`)
    const apps = m.grants.filter((g) => g.bot === b.id).map((g) => `${g.app.name} (${g.level})`)
    if (apps.length) out.push(`${pad}  apps: ${apps.join(", ")}`)
    for (const a of m.asks.filter((x) => x.actor === b.id)) {
      const rule = m.rules.find((r) => r.id === a.rule_id)
      out.push(`${pad}  ⚑ ${b.name} asks: ${rule?.text ?? a.title}`)
    }
    for (const j of m.jobs.filter((x) => x.actor === b.id && x.state === "waiting_approval")) {
      out.push(`${pad}  → waiting on ${first(j.waiting_on) ?? "approval"}${j.title ? `  (${j.title})` : ""}`)
    }
    for (const c of children(b.id, "parent")) botLines(c, depth + 1)
  }
  const personLines = (p: MapNode, depth: number, seen: Set<string>) => {
    if (seen.has(p.id)) return
    seen.add(p.id)
    const pad = "  ".repeat(depth)
    const mine = p.id === m.me ? " (you)" : ""
    out.push(`${pad}${p.name}${mine}${p.role ? `  ${p.role}` : ""}`)
    // Top-level bots only: sub-bots print under their parent bot.
    const owned = children(p.id, "owns")
    const subs = new Set(m.edges.filter((e) => e.kind === "parent").map((e) => e.to))
    for (const b of owned.filter((b) => !subs.has(b.id))) botLines(b, depth + 1)
    for (const r of m.edges.filter((e) => e.kind === "reports_to" && e.to === p.id)) {
      const rp = byId.get(r.from)
      if (rp) personLines(rp, depth + 1, seen)
    }
  }
  const people = m.nodes.filter((n) => n.kind === "person")
  const reporting = new Set(m.edges.filter((e) => e.kind === "reports_to").map((e) => e.from))
  const seen = new Set<string>()
  for (const p of people.filter((p) => !reporting.has(p.id))) personLines(p, 0, seen)
  for (const p of people) personLines(p, 0, seen)
  if (m.partial?.length) out.push("", "Some approvals or apps may be missing.")
  if (!m.jobs.length) out.push("", "Jobs not available yet.")
  return out
}

export type AskRow = { id: string; store?: string; title: string; summary?: string; actor: string | null; risk?: string; rule_id?: string | null; resolve: { method: string; path: string; body: unknown } | null }

export function formatAsks(asks: AskRow[], partial: string[] = []): string[] {
  if (!asks.length) return ["No asks waiting on you."]
  const out: string[] = []
  for (const a of asks) {
    out.push(`${a.id}  ${a.title}${a.risk === "high" ? "  [high risk]" : ""}${a.resolve ? "" : "  (read-only here)"}`)
    if (a.summary) out.push(`  ${a.summary}`)
  }
  if (partial.length) out.push("", "Some approvals may be missing.")
  return out
}

export type RuleRow = { id: string; text: string; enabled?: boolean }
export function formatRules(rules: RuleRow[]): string[] {
  if (!rules.length) return ["No approval rules. Add one with `gizzi team rules add`."]
  return rules.map((r) => `${r.id}${r.enabled === false ? " (off)" : ""}  ${r.text}`)
}

/** Plain-words wording for team error codes (never show the raw code). */
export function teamErrorMessage(status: number, code: string | undefined, fallback: string): string {
  if (status === 409 || status === 428 || status === 0 || code === "no_runtime")
    return "No computer connected. Run `gizzi connect` (or open Allternit on a computer) and try again."
  switch (code) {
    case "above_ceiling": return "That level is above what the bot's owner may give for this app. Pick a lower level."
    case "tier_check_unavailable": return "Could not check the owner's plan right now, so nothing was changed. Try again shortly."
    case "app_not_installed": return "That app is not installed for the bot's owner."
    case "bot_not_found": return "No bot with that name or id."
    case "owner_or_admin_required": return "Only the bot's owner, an org owner or an admin can do that."
    case "invalid_rule": return "That rule is not valid. Check --app, --tool, --field, --op, --value and --approver."
    case "rule_not_found": return "No rule with that id."
  }
  return fallback
}
