/**
 * Pure rendering for `gizzi do`: the review card, the status line and the receipt.
 * Wording follows the Island bar contract (CONTRACT.md sections 4 and 5). The rule text
 * is printed exactly as the server sent it.
 */

export type Review = {
  job_id: string
  state: string
  level?: string | null
  needs_confirmation?: boolean
  rule?: { id: string; text: string } | null
  approver?: { user_id: string; name?: string | null; is_you?: boolean } | null
  lines?: { key: string; label: string; value: unknown }[]
}

export type BarJob = {
  id: string
  tool?: string
  title?: string | null
  args?: Record<string, unknown> | null
  state: string
  result?: any
  error?: string | null
  approver_user_id?: string | null
}

export const TERMINAL = ["completed", "failed", "cancelled"]

export function formatReview(r: Review): string[] {
  const out: string[] = []
  for (const l of r.lines ?? []) out.push(`  ${l.label}: ${String(l.value)}`)
  out.push(`  Level: ${r.level ?? "unknown"}`)
  if (r.rule) {
    out.push(`  Rule: ${r.rule.text}`)
    if (r.approver) out.push(`  Approver: ${r.approver.is_you ? "you" : (r.approver.name ?? "someone else")}`)
  } else {
    out.push("  Rule: none applies")
  }
  return out
}

/** True when someone other than the caller must approve. `--yes` never skips this. */
export function needsOwnerApproval(r: Review): boolean {
  return !!r.rule && !!r.approver && !r.approver.is_you
}

const BARS = 4

/** "● Waiting on Noah ▰▱▱▱". `tick` moves the bar so a waiting job looks alive. */
export function statusLine(job: BarJob, approverName?: string | null, tick = 0): string {
  const bar = (n: number) => "▰".repeat(n) + "▱".repeat(BARS - n)
  const pos = (tick % BARS) + 1
  switch (job.state) {
    case "waiting_approval":
      return `● ${approverName ? `Waiting on ${approverName}` : "Waiting on approval"} ${bar(pos)}`
    case "working":
      return `● Working ${bar(pos)}`
    case "input_required":
      return "● Needs input"
    case "review":
      return "● Review"
    case "completed":
      return "● Done"
    case "failed":
      return `● Failed${job.error ? `: ${job.error}` : ""}`
    case "cancelled":
      return "● Cancelled"
    default:
      return `● ${job.state}`
  }
}

const PAST: Record<string, string> = {
  refund: "Refunded", create: "Created", send: "Sent", delete: "Deleted", update: "Updated",
  list: "Listed", cancel: "Cancelled", pay: "Paid", add: "Added", close: "Closed",
}
const SYMBOL: Record<string, string> = { eur: "€", usd: "$", gbp: "£" }

/** "Refunded €320" from the job result or args; falls back to the tool title, never raw JSON. */
export function receipt(job: BarJob): string {
  const sc = job.result?.structuredContent ?? job.result ?? {}
  const args = job.args ?? {}
  const verbKey = (job.tool ?? "").split(/[_\-.]/)[0]?.toLowerCase() ?? ""
  const verb = PAST[verbKey]
  const amount = typeof sc.amount === "number" || typeof sc.amount === "string" ? sc.amount : (args as any).amount
  const cur = typeof sc.currency === "string" ? sc.currency.toLowerCase() : undefined
  if (verb && amount !== undefined && amount !== null && typeof amount !== "object") {
    const sym = cur ? SYMBOL[cur] : undefined
    const unit = sym ? "" : cur ? ` ${cur.toUpperCase()}` : ""
    return `${verb} ${sym ?? ""}${amount}${unit}`
  }
  if (verb) return `${verb}: ${job.title ?? job.tool}`
  return job.title ?? job.tool ?? "Done"
}

/** Clear one-line wording for server error codes (never print a raw code). */
export function errorMessage(status: number, code: string | undefined, message?: string): string {
  if (status === 409 && code === "no_runtime" || status === 428 || status === 0 || code === "no_runtime")
    return "No computer is connected. Run `gizzi connect` to set one up, then try again."
  switch (code) {
    case "app_not_runnable":
      return "This app is not runnable yet."
    case "no_grant":
      return "Ask the owner to grant access to this app."
    case "level_read_only":
      return "This app is read-only at your access level."
    case "approver_required":
      return "A rule needs an approver and none was named."
    case "team_rules_unavailable":
    case "tier_check_unavailable":
      return "Can't check the rules right now. Try again in a moment."
    case "missing_fields":
      return message ?? "Fill in the missing fields first."
  }
  if (status === 401) return "Not signed in. Run `gizzi auth login` first."
  return message ?? `Request failed (${status})`
}
