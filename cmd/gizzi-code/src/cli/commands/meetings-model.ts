/** Pure pieces of `gizzi meetings`: state labels (same words as the web), call-link check, SSE parsing, error text. */

export const GIZZI_LABELS: Record<string, string> = {
  scheduled: "Gizzi joins",
  off: "Off",
  launching: "Joining",
  joining: "Joining",
  waiting_admit: "Waiting to be let in",
  live: "Live",
  paused: "Not listening",
  ended: "Recap ready",
  not_admitted: "Not let in",
}

export function gizziLabel(state: string | null | undefined): string {
  return GIZZI_LABELS[state ?? ""] ?? (state || "Off")
}

export const END_REASON_LABELS: Record<string, string> = {
  left_by_user: "You told Gizzi to leave",
  removed: "Gizzi was removed or the call closed",
  not_admitted: "Not let in",
  empty_room: "Everyone else left",
  max_duration: "Reached the 8 hour limit",
  other_notetaker: "Another notetaker joined",
  join_failed: "Gizzi could not join",
  guest_blocked: "The platform refused Gizzi's identity",
  no_capacity: "No capacity right now",
  error: "Error",
}

export type CallPlatform = "meet" | "zoom" | "teams"

/** Same patterns as the cloud contract (section 7). The server stays the authority; this saves a round trip. */
export function parseCallUrl(raw: string): { platform: CallPlatform; label: string } | undefined {
  const url = raw.trim()
  if (/^https:\/\/meet\.google\.com\/([a-z]{3}-[a-z]{4}-[a-z]{3}|lookup\/[^/?#\s]+)(?:[/?#].*)?$/i.test(url))
    return { platform: "meet", label: "Google Meet" }
  if (/^https:\/\/([a-z0-9-]+\.)?zoom\.(us|com)\/(?:(?:j|wc\/join)\/\d{9,11}|wc\/\d{9,11}\/join|my\/[\w.-]+)(?:[/?#].*)?$/i.test(url))
    return { platform: "zoom", label: "Zoom" }
  if (/^https:\/\/teams\.microsoft\.com\/(?:l\/meetup-join\/[^\s]+|meet\/\d{9,15}(?:\?p=[\w-]+)?)$/i.test(url) ||
    /^https:\/\/teams\.live\.com\/meet\/\d{9,15}/i.test(url))
    return { platform: "teams", label: "Microsoft Teams" }
  return undefined
}

export const UPGRADE_URL = "https://ai.allternit.com/settings/billing"

/** One plain line for a failed API call. */
export function describeApiError(status: number, body?: { error?: unknown; message?: string }): string {
  const code = typeof body?.error === "string" ? body.error : (body?.error as { code?: string } | undefined)?.code
  const message = body?.message || (typeof body?.error === "object" ? (body.error as { message?: string })?.message : undefined)
  if (status === 401) return "Allternit Cloud did not accept your credentials. Run `gizzi login` first."
  if (status === 402) return `Meetings needs a paid plan. Upgrade: ${UPGRADE_URL}`
  if (status === 403 && code === "meetings_disabled_by_org") return "Your organization has turned Meetings off. Ask an org admin to enable it."
  if (status === 429) return "Gizzi has no free capacity to join right now. Try again in a few minutes."
  if (status === 400 && code === "unsupported_link") return "That link is not a Google Meet, Zoom or Teams call link."
  if (status === 404 && (code === "meeting_not_found" || code === "session_not_found")) return "No such meeting."
  if (status === 404 || status === 501 || status === 503 || code === "feature_disabled")
    return "Meetings isn't turned on for this account or server yet."
  return `Meetings request failed (HTTP ${status})${message ? `: ${message}` : code ? `: ${code}` : ""}`
}

/** Parse as many complete SSE frames as `buf` holds; returns them plus the unread rest. */
export function parseSse(buf: string): { events: { id?: string; event: string; data: string }[]; rest: string } {
  const events: { id?: string; event: string; data: string }[] = []
  const frames = buf.replace(/\r\n/g, "\n").split("\n\n")
  const rest = frames.pop() ?? ""
  for (const frame of frames) {
    let id: string | undefined
    let event = "message"
    const data: string[] = []
    for (const line of frame.split("\n")) {
      if (line.startsWith(":")) continue
      if (line.startsWith("id:")) id = line.slice(3).trim()
      else if (line.startsWith("event:")) event = line.slice(6).trim()
      else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""))
    }
    if (data.length) events.push({ id, event, data: data.join("\n") })
  }
  return { events, rest }
}

export function mmss(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`
}
