/** Allternit Sleep (memory consolidation) from the terminal: pure helpers. Same words as Desktop's Sleep settings. */

export type SleepCycle = {
  id: string
  kind?: string
  state: string
  stage?: string | null
  started_at?: string
  finished_at?: string | null
  undone_at?: string | null
  summary?: string | null
}
export type SleepStatus = {
  available?: boolean
  enabled: boolean
  debt?: number
  last_cycle?: SleepCycle | null
  spend_today_usd?: number
  daily_spend_cap_usd?: number
  open_disputes?: number
}

export const NO_SLEEP = "this runtime has no Sleep yet"
export const SIGNED_OUT = "Sign in first: run `gizzi login`, then try again."

/** Turn an API failure into one plain line. 404 means an older runtime; 401/403 means signed out. */
export function sleepError(e: unknown): string {
  const m = e instanceof Error ? e.message : String(e)
  if (/\b404\b/.test(m)) return NO_SLEEP
  if (/\b(401|403)\b/.test(m)) return SIGNED_OUT
  return m
}

export function isRunning(c?: SleepCycle | null): boolean {
  return !!c && ["running", "queued", "active", "started"].includes(c.state) && !c.finished_at
}

export function stateWord(s: SleepStatus): "paused" | "running" | "idle" | "never" {
  if (!s.enabled) return "paused"
  if (isRunning(s.last_cycle)) return "running"
  return s.last_cycle ? "idle" : "never"
}

function ago(iso?: string | null, now = Date.now()): string {
  if (!iso) return "—"
  const ms = now - Date.parse(iso)
  if (!Number.isFinite(ms)) return "—"
  const a = Math.abs(ms)
  const u = a < 60_000 ? `${Math.round(a / 1000)}s` : a < 3_600_000 ? `${Math.round(a / 60_000)}m` : a < 86_400_000 ? `${Math.round(a / 3_600_000)}h` : `${Math.round(a / 86_400_000)}d`
  return `${u} ago`
}

export function formatSleepStatus(s: SleepStatus, now = Date.now()): string {
  const c = s.last_cycle
  const head = {
    paused: "Sleep is paused",
    running: `Sleeping now${c?.kind ? ` (${c.kind}${c.stage ? `, ${c.stage}` : ""})` : ""}`,
    idle: "Sleep is on",
    never: "Sleep is on, but has not run yet",
  }[stateWord(s)]
  const lines = [head]
  lines.push(`Last run:  ${c ? `${ago(c.finished_at ?? c.started_at, now)}${c.undone_at ? " (undone)" : ""}` : "never"}`)
  lines.push(
    `Next run:  ${s.enabled ? `when memory debt builds${typeof s.debt === "number" ? ` (debt ${s.debt.toFixed(1)})` : ""}` : "paused (resume to continue)"}`,
  )
  if (c?.summary) lines.push(`Report:    ${c.summary}`)
  if (s.open_disputes) lines.push(`Open disputes: ${s.open_disputes}`)
  if (typeof s.spend_today_usd === "number" && s.daily_spend_cap_usd)
    lines.push(`Spend today: $${s.spend_today_usd.toFixed(2)} of $${s.daily_spend_cap_usd.toFixed(2)}`)
  return lines.join("\n")
}
