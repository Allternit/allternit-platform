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
