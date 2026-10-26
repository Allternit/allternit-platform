/**
 * Open-lane refusal handoff (Allternit Cloud, platform migration 105).
 *
 * Two ways a refused turn reaches an open model:
 *
 * 1. Allternit Cloud does it itself. When the person's turn runs on an
 *    Allternit Cloud frontier model that refuses, api.allternit.com re-runs it
 *    on an open model and says so: a streamed chat-completions chunk with no
 *    choices carrying `allternit.handoff: {from, to}`, or the
 *    `x-allternit-handoff: <from>-><to>` header on a non-streamed reply. The
 *    provider fetch strips the chunk (see retry-hint.ts) and records it here;
 *    the prompt loop puts it on the assistant message.
 * 2. Any other brain (the person's own subscription or key) refuses: the
 *    prompt loop sees a refusal with `isRefusal`, checks the person's
 *    open-lane settings with `handoffTarget`, and runs the turn once more on
 *    the open model through Allternit Cloud. Hard-line moderation, the terms
 *    gate and billing stay server-side in cloud-api.
 */

import { Log } from "@/shared/util/log"
import { cloudAuthToken, cloudOrigin } from "./discovery/allternit-cloud"

const log = Log.create({ service: "provider.handoff" })

export const HANDOFF_HEADER = "x-allternit-handoff"
/** The Allternit Cloud provider id in gizzi. */
export const CLOUD_PROVIDER_ID = "allternit"

export interface Handoff {
  from: string
  to: string
}

// ── 1. Cloud-side handoffs ──────────────────────────────────────────────────

const recorded = new Map<string, Handoff>()

export function recordHandoff(sessionID: string, handoff: Handoff): void {
  recorded.set(sessionID, handoff)
}

/** Take the handoff recorded for a session's last request, at most once. */
export function consumeHandoff(sessionID: string): Handoff | undefined {
  const handoff = recorded.get(sessionID)
  recorded.delete(sessionID)
  return handoff
}

function asHandoff(value: unknown): Handoff | null {
  if (!value || typeof value !== "object") return null
  const { from, to } = value as { from?: unknown; to?: unknown }
  return typeof from === "string" && typeof to === "string" && from && to ? { from, to } : null
}

/** `allternit.handoff` from one SSE frame's data, or null for any other frame. */
export function frameHandoff(frame: string): Handoff | null {
  if (!frame.includes("\"allternit\"")) return null
  const data: string[] = []
  for (const line of frame.split(/\r?\n/)) {
    if (line.startsWith("event:")) return null
    if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""))
  }
  if (data.length === 0) return null
  try {
    const chunk = JSON.parse(data.join("\n")) as { allternit?: { handoff?: unknown } }
    return asHandoff(chunk?.allternit?.handoff)
  } catch {
    return null
  }
}

/** `x-allternit-handoff: <from>-><to>` (the non-streamed reply's marker). */
export function headerHandoff(value: string | null): Handoff | null {
  if (!value) return null
  const at = value.indexOf("->")
  if (at <= 0) return null
  return asHandoff({ from: value.slice(0, at).trim(), to: value.slice(at + 2).trim() })
}

// ── 2. Refusals from any other brain ────────────────────────────────────────
// Same rules as cloud-api `services/refusal.rs`: a provider refusal signal, or
// a refusal phrase opening a SHORT answer. Handing a real answer off would be
// worse than missing a refusal, so long answers and partial help never count.

const REFUSAL_FINISH_REASONS = new Set(["content_filter", "content-filter", "refusal"])

const REFUSAL_MARKERS = [
  "i can't",
  "i cannot",
  "i can not",
  "i won't",
  "i will not",
  "i'm not able",
  "i am not able",
  "i'm unable",
  "i am unable",
  "i'm not going to",
  "i am not going to",
  "i must decline",
  "i have to decline",
  "i'm sorry, but",
  "i am sorry, but",
  "sorry, but i can",
  "i apologize, but",
  "as an ai",
  "i'm not comfortable",
  "i am not comfortable",
]

const PARTIAL_HELP_MARKERS = [", but here", ". here's", ", however, here", "but i can help", "instead, here"]

const LEAD_CHARS = 220
const MAX_REFUSAL_CHARS = 1_200

/** Whether a finished step (its text and finish reason) reads as a refusal. */
export function isRefusal(text: string, finish?: string): boolean {
  if (finish && REFUSAL_FINISH_REASONS.has(finish.toLowerCase())) return true
  const trimmed = text.trim()
  if (!trimmed || [...trimmed].length > MAX_REFUSAL_CHARS) return false
  const lead = [...trimmed.replace(/^["*_>]+/, "")]
    .slice(0, LEAD_CHARS)
    .join("")
    .toLowerCase()
    .replace(/’/g, "'")
  const positions = REFUSAL_MARKERS.map((m) => lead.indexOf(m)).filter((p) => p >= 0)
  if (positions.length === 0) return false
  // The marker has to open the answer (first sentence), not appear later.
  if (Math.min(...positions) > 80) return false
  return !PARTIAL_HELP_MARKERS.some((m) => lead.includes(m))
}

type OpenLaneSettings = {
  available?: boolean
  models?: unknown[]
  terms_accepted?: boolean
  handoff_enabled?: boolean
  default_handoff_model?: string
}

const SETTINGS_TTL_MS = 60_000
let cached: { at: number; token: string; settings: OpenLaneSettings | null } | undefined

async function openLaneSettings(token: string): Promise<OpenLaneSettings | null> {
  if (cached && cached.token === token && Date.now() - cached.at < SETTINGS_TTL_MS) return cached.settings
  let settings: OpenLaneSettings | null = null
  try {
    const res = await fetch(`${cloudOrigin()}/api/v1/open-lane`, {
      headers: { Authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(5000),
    })
    if (res.ok) settings = (await res.json()) as OpenLaneSettings
  } catch (error) {
    log.warn("open-lane settings unavailable", { error: (error as Error).message })
  }
  cached = { at: Date.now(), token, settings }
  return settings
}

export function clearHandoffCache(): void {
  cached = undefined
  recorded.clear()
}

/**
 * The Allternit Cloud open model to hand a refused turn to, or undefined when
 * the person has not turned handoff on (Settings → Open models), has no
 * Allternit Cloud sign-in, or the open lane is not serving.
 */
export async function handoffTarget(): Promise<{ providerID: string; modelID: string } | undefined> {
  if (process.env.GIZZI_DISABLE_OPEN_LANE_HANDOFF) return undefined
  const token = cloudAuthToken()
  if (!token) return undefined
  const settings = await openLaneSettings(token)
  if (!settings?.available || !settings.terms_accepted || !settings.handoff_enabled) return undefined
  const modelID = settings.default_handoff_model || "deepseek-v4.1-flash-open"
  return { providerID: CLOUD_PROVIDER_ID, modelID }
}
