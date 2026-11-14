/**
 * Pure parts of `gizzi connect`: the client catalog, the merge of cloud and
 * runtime rows, the hub grouping and the status words. Contract:
 * CONTRACT-connections-2026-11 (four words only: Not connected, Connecting…,
 * Connected, Needs you). Mirrors the ids and copy of the web catalog
 * (src/components/connect-kit/registry.ts in allternit-ai).
 */

export type Group = "brains" | "computers" | "phones" | "tools" | "voice" | "apps" | "channels"
export type State = "off" | "connecting" | "on" | "needs"
export type RunsOn = "server" | "runner" | "device" | "computer"

export type Row = {
  id: string
  group?: Group
  state?: State
  found?: string | null
  reason?: string | null
  reason_code?: string | null
  runs_on?: RunsOn
  available?: boolean
  unavailable_reason?: string | null
  fallbacks?: string[]
  terms?: "cleared" | "pending" | "blocked"
  saved?: string
  models?: string[]
  source?: string
}

export type Entry = { id: string; group: Group; name: string; blurb?: string; runsOn?: RunsOn; hidden?: boolean }

const e = (id: string, group: Group, name: string, blurb?: string, runsOn?: RunsOn, hidden?: boolean): Entry => ({ id, group, name, blurb, runsOn, hidden })

export const CATALOG: Entry[] = [
  e("allternit", "brains", "Allternit Cloud", undefined, "server"),
  e("chatgpt", "brains", "ChatGPT", "Plus or Pro plan", "computer"),
  e("claude", "brains", "Claude", "Pro or Max plan", "runner"),
  e("gemini", "brains", "Gemini", "Free Google AI Studio key", "server"),
  e("copilot", "brains", "GitHub Copilot", "Copilot Pro or Business", "server"),
  e("grok", "brains", "Grok", "SuperGrok plan", "runner"),
  e("kimi", "brains", "Kimi", "Kimi plan", "runner"),
  e("openai-key", "brains", "OpenAI key", "An OpenAI API key", "server"),
  e("anthropic-key", "brains", "Anthropic key", "An Anthropic API key", "server"),
  e("google-key", "brains", "Google AI Studio key", "A Google AI Studio key", "server", true),
  e("xai-key", "brains", "xAI key", "An xAI API key", "server"),
  e("openrouter-key", "brains", "OpenRouter key", "An OpenRouter API key", "server"),
  e("together-key", "brains", "Together key", "A Together API key", "server"),
  e("fireworks-key", "brains", "Fireworks key", "A Fireworks API key", "server"),
  e("deepinfra-key", "brains", "DeepInfra key", "A DeepInfra API key", "server"),
  e("groq-key", "brains", "Groq key", "A Groq API key", "server"),
  e("deepseek-key", "brains", "DeepSeek key", "A DeepSeek API key", "server"),
  e("kimi-key", "brains", "Kimi key", "A Moonshot API key", "server"),
  e("local", "brains", "Local brain", "Runs offline on your own computer", "computer"),

  e("cloud-computer", "computers", "Allternit Cloud computer"),
  e("this-mac", "computers", "Your Mac or PC", "Lets Allternit use the apps on it"),
  e("server", "computers", "Your own server", "A Linux machine you own"),

  e("phone", "phones", "Your phone", "iPhone or Android"),
  e("phone-control", "phones", "Let Allternit use your phone", "Taps and types in your phone apps for you"),

  e("gizzi", "tools", "Gizzi Code"),
  e("claude-code", "tools", "Claude Code"),
  e("codex", "tools", "Codex"),
  e("gemini-cli", "tools", "Gemini CLI"),
  e("cursor", "tools", "Cursor Agent"),
  e("grok-cli", "tools", "Grok CLI"),
  e("kimi-cli", "tools", "Kimi CLI"),

  e("openai-voice", "voice", "OpenAI", "Realtime voice"),
  e("elevenlabs", "voice", "ElevenLabs", "Studio voices"),

  e("gmail", "apps", "Gmail"),
  e("drive", "apps", "Google Drive"),
  e("slack", "apps", "Slack"),
  e("github", "apps", "GitHub"),
  e("notion", "apps", "Notion"),

  e("telegram", "channels", "Telegram"),
  e("whatsapp", "channels", "WhatsApp"),
  e("discord", "channels", "Discord"),
]

export const REASON_TEXT: Record<string, string> = {
  login_expired: "Sign-in expired",
  key_rejected: "The key was rejected",
  vendor_limited: "The vendor limited this plan",
  no_computer: "Needs your own computer for now",
  not_available_yet: "Not available yet",
  computer_offline: "Your computer is offline",
  could_not_start: "Could not start. Try again",
}

export const GROUP_TITLE: Record<Group, string> = {
  brains: "Brains",
  computers: "Computers",
  phones: "Phones",
  tools: "Coding tools",
  voice: "Voice",
  apps: "Apps",
  channels: "Channels",
}
export const GROUP_ORDER: Group[] = ["brains", "computers", "phones", "tools", "voice", "apps", "channels"]
export const BRAIN_SUB = ["Works on any device", "Needs your computer", "Coming soon"] as const
export type BrainSub = (typeof BRAIN_SUB)[number]

/** A runtime row wins over a cloud row with the same id only when its state is `on`. */
export function mergeRows(cloud: Row[], runtime: Row[]): Row[] {
  const out = new Map<string, Row>()
  for (const r of cloud) out.set(r.id, r)
  for (const r of runtime) {
    const prev = out.get(r.id)
    if (!prev || r.state === "on") out.set(r.id, r)
  }
  return [...out.values()]
}

/** Contract defaults for rows the server did not send: not yet usable, with a plain reason and fallback. */
const NOT_YET = "Not available yet. Use a key for now."
export const DEFAULT_ROWS: Record<string, Row> = {
  claude: { id: "claude", state: "off", available: false, terms: "pending", reason_code: "not_available_yet", unavailable_reason: NOT_YET, fallbacks: ["anthropic-key"] },
  grok: { id: "grok", state: "off", available: false, terms: "pending", reason_code: "not_available_yet", unavailable_reason: NOT_YET, fallbacks: ["xai-key"] },
  kimi: { id: "kimi", state: "off", available: false, terms: "pending", reason_code: "not_available_yet", unavailable_reason: NOT_YET, fallbacks: ["kimi-key"] },
  copilot: { id: "copilot", state: "off", available: false, reason_code: "not_available_yet", unavailable_reason: "Coming soon.", fallbacks: ["github"] },
}

/** Catalog order; rows with unknown ids are ignored, missing ids use the contract default (else off). */
export function resolve(rows: Row[]): { entry: Entry; row: Row }[] {
  const byId = new Map(rows.map((r) => [r.id, r]))
  return CATALOG.map((entry) => ({ entry, row: byId.get(entry.id) ?? DEFAULT_ROWS[entry.id] ?? { id: entry.id, state: "off" as State } }))
}

export function brainSub(entry: Entry, row: Row): BrainSub {
  if (row.available === false && (row.reason_code === "not_available_yet" || row.terms === "pending")) return "Coming soon"
  if (row.available === false && row.reason_code !== "no_computer" && row.reason_code !== "computer_offline" && row.runs_on !== "computer") return "Coming soon"
  const runsOn = row.runs_on ?? entry.runsOn
  if (runsOn === "computer" || runsOn === "device") return "Needs your computer"
  return "Works on any device"
}

/** One of the four status words plus its text. */
export function statusText(entry: Entry, row: Row): string {
  switch (row.state ?? "off") {
    case "on":
      return row.found ? `Connected · ${row.found}` : "Connected"
    case "connecting":
      return "Connecting…"
    case "needs": {
      const why = row.reason || (row.reason_code ? REASON_TEXT[row.reason_code] : undefined)
      return why ? `Needs you · ${why}` : "Needs you"
    }
    default:
      return entry.blurb ? `Not connected · ${entry.blurb}` : "Not connected"
  }
}

export function nameOf(id: string): string {
  return CATALOG.find((c) => c.id === id)?.name ?? id
}

/** The line shown for a row the account cannot use: the reason and up to two fallbacks, never a Connect hint. */
export function unavailableText(entry: Entry, row: Row): string {
  const why = row.unavailable_reason || (row.reason_code ? REASON_TEXT[row.reason_code] : "Not available yet")
  const fb = (row.fallbacks ?? []).slice(0, 2)
  return fb.length ? `${why}  Try: ${fb.map((f) => `gizzi connect ${f}`).join("  or  ")}` : why!
}

function line(entry: Entry, row: Row): string {
  const name = entry.name.padEnd(30)
  return `  ${name}${row.available === false ? unavailableText(entry, row) : statusText(entry, row)}`
}

/** The hub as terminal lines, grouped like the web page (Brains split in three). */
export function renderHub(rows: Row[]): string[] {
  const all = resolve(rows).filter((x) => !x.entry.hidden)
  const out: string[] = []
  for (const g of GROUP_ORDER) {
    const items = all.filter((x) => x.entry.group === g)
    if (!items.length) continue
    out.push(GROUP_TITLE[g])
    if (g === "brains") {
      for (const sub of BRAIN_SUB) {
        const part = items.filter((x) => brainSub(x.entry, x.row) === sub)
        if (!part.length) continue
        out.push(` ${sub}`)
        for (const x of part) out.push(line(x.entry, x.row))
      }
    } else {
      for (const x of items) out.push(line(x.entry, x.row))
    }
    out.push("")
  }
  return out
}

/** Find a row by id, or by a unique case-insensitive name/id prefix. */
export function findEntry(key: string): Entry | undefined {
  const k = key.toLowerCase()
  return CATALOG.find((c) => c.id === k) ?? CATALOG.find((c) => c.name.toLowerCase() === k)
}
