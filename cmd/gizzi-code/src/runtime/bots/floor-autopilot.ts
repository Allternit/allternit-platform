/**
 * Autopilot + "While you were away" client for the Workspace floor
 * (allternit-api `/api/v1/floor/autopilot`, `/floor/digest`, `/floor/seen`).
 * Pure formatting lives here so bun tests can drive it without the TUI.
 */
import { PlatformApiError, PlatformSignedOutError, platformRequest } from "@/runtime/bots/platform-api"

export type AutopilotNeed = "suggestion" | "permission" | "question"
export type AutopilotLevel = "draft" | "ask" | "tell" | "limits"

export interface AutopilotRule {
  id: string
  scope: { type: "all" | "bot" | "pod"; id?: string }
  need: AutopilotNeed
  level: AutopilotLevel
  limits?: { maxPerDay?: number; allowedActions?: string[] }
  note?: string
}

export interface AutopilotState {
  enabled: boolean
  mainBotId?: string | null
  mainBotName?: string | null
  rules: AutopilotRule[]
  updatedAt?: number
}

export interface DigestItem {
  botId?: string
  botName?: string
  threadId?: string
  text: string
  at: number
  ruleId?: string
}

export interface Digest {
  since: number
  until: number
  answered: DigestItem[]
  waiting: DigestItem[]
  finished: DigestItem[]
  suggestions: DigestItem[]
}

const LEVEL_WORDS: Record<AutopilotLevel, string> = {
  draft: "Draft only",
  ask: "Ask first",
  tell: "Send and tell me",
  limits: "Act within limits",
}

export function levelWords(level: AutopilotLevel, need?: AutopilotNeed): string {
  // Questions are draft only for now, whatever the rule says.
  if (need === "question") return LEVEL_WORDS.draft
  return LEVEL_WORDS[level] ?? String(level)
}

export function scopeWords(scope: AutopilotRule["scope"]): string {
  if (scope.type === "all") return "Everything"
  return `${scope.type === "pod" ? "Project" : "Bot"} ${scope.id ?? ""}`.trim()
}

export function formatRule(rule: AutopilotRule): string {
  let level = levelWords(rule.level, rule.need)
  if (rule.level === "limits" && rule.need !== "question" && rule.limits?.maxPerDay) {
    level += ` (up to ${rule.limits.maxPerDay} a day)`
  }
  return `${rule.need.padEnd(10)}  ${scopeWords(rule.scope).padEnd(18)}  ${level}`
}

export function formatAutopilot(state: AutopilotState): string {
  const lines = [`Autopilot is ${state.enabled ? "on" : "off"}.`]
  lines.push(`Main bot: ${state.mainBotName ?? state.mainBotId ?? "not set (pick one in the Allternit app)"}`)
  if (state.rules.length === 0) {
    lines.push("No rules yet. Add them in the Workspace's Autopilot sheet.")
  } else {
    lines.push("", `${"Need".padEnd(10)}  ${"Scope".padEnd(18)}  Level`)
    for (const r of state.rules) lines.push(formatRule(r))
  }
  return lines.join("\n")
}

function section(title: string, items: DigestItem[], suffix = ""): string[] {
  if (items.length === 0) return []
  return [
    "",
    `${title} (${items.length})`,
    ...items.map((i) => `  - ${i.botName ? `${i.botName}: ` : ""}${i.text}${suffix && i.ruleId ? suffix : ""}`),
  ]
}

export function digestIsEmpty(d: Digest): boolean {
  return d.answered.length + d.waiting.length + d.finished.length + d.suggestions.length === 0
}

export function formatDigest(d: Digest): string {
  if (digestIsEmpty(d)) return "Nothing happened while you were away."
  return [
    "While you were away",
    ...section("Answered", d.answered, " (on autopilot)"),
    ...section("Waiting for you", d.waiting),
    ...section("Finished", d.finished),
    ...section("Suggestions", d.suggestions),
  ].join("\n")
}

/** Turn an API failure into one plain sentence for the terminal. */
export function describeFloorError(e: unknown): string {
  if (e instanceof PlatformSignedOutError) return e.message
  if (e instanceof PlatformApiError) {
    if (e.status === 404) return "This runtime does not support Autopilot yet."
    if (e.status === 401 || e.status === 403) return "Not signed in to Allternit. Run `gizzi login`."
    return e.message
  }
  return "Can't reach the Allternit runtime. Check that it is running and you are online."
}

export async function getAutopilot(): Promise<AutopilotState> {
  return platformRequest<AutopilotState>("GET", "/api/v1/floor/autopilot", undefined, { timeoutMs: 10_000 })
}

export async function setAutopilotEnabled(enabled: boolean): Promise<AutopilotState> {
  const current = await getAutopilot()
  return platformRequest<AutopilotState>(
    "PUT",
    "/api/v1/floor/autopilot",
    { enabled, mainBotId: current.mainBotId ?? null, rules: current.rules },
    { timeoutMs: 10_000 },
  )
}

export async function getDigest(): Promise<Digest> {
  return platformRequest<Digest>("GET", "/api/v1/floor/digest", undefined, { timeoutMs: 10_000 })
}

export async function markSeen(): Promise<void> {
  await platformRequest("POST", "/api/v1/floor/seen", {}, { timeoutMs: 10_000 })
}

/** Run `/autopilot [status|on|off]`. Returns the text to print. */
export async function runAutopilot(args: string): Promise<string> {
  const arg = args.trim().toLowerCase()
  if (arg !== "" && arg !== "status" && arg !== "on" && arg !== "off") return "Usage: /autopilot [status|on|off]"
  try {
    const state = arg === "on" || arg === "off" ? await setAutopilotEnabled(arg === "on") : await getAutopilot()
    return formatAutopilot(state)
  } catch (e) {
    return describeFloorError(e)
  }
}

/** Run `/away`: print the digest, then mark it seen. */
export async function runAway(): Promise<string> {
  try {
    const digest = await getDigest()
    const text = formatDigest(digest)
    await markSeen().catch(() => undefined)
    return text
  } catch (e) {
    return describeFloorError(e)
  }
}
