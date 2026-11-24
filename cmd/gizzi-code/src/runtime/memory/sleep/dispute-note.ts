/**
 * Memory conflicts in recall (Allternit Sleep `disputes` stage, step 6.2).
 *
 * A recall row from the server may carry `dispute`: both sides, the pick and
 * the reason. `describeDispute` turns it into the text the model sees.
 * `askDisputes` asks the person ONCE per dispute id per session (through the
 * existing question integration) when the dispute needs them, and records the
 * answer with POST /memory/disputes/:id/resolve. Older servers send no
 * `dispute`, so nothing here runs and recall output is unchanged.
 */
import { sleepCall, type SleepOutcome } from "./client"

export interface DisputeSide {
  id: string
  text?: string | null
  current?: boolean
}

export interface DisputeNote {
  id: string
  state: "open" | "needs_user" | "resolved" | string
  pick?: "a" | "b" | "both_scoped" | "neither" | null
  reason?: string | null
  scope?: string | null
  stakes?: string | null
  margin?: number | null
  side_a: DisputeSide
  side_b: DisputeSide
}

export type DisputeChoice = "a" | "b" | "both_scoped" | "neither"

const clip = (s: string | null | undefined, n = 240) => {
  const t = (s ?? "").replace(/\s+/g, " ").trim()
  return t.length > n ? `${t.slice(0, n - 1)}…` : t || "(text not available)"
}

/** Accepts only a well-formed dispute; anything else is treated as "no dispute". */
export function readDispute(value: unknown): DisputeNote | undefined {
  const d = value as DisputeNote | null | undefined
  if (!d || typeof d !== "object" || typeof d.id !== "string" || !d.side_a || !d.side_b) return undefined
  return d
}

export function needsUser(d: DisputeNote): boolean {
  return d.state === "needs_user" && !d.pick
}

/** The sentence(s) the model sees after a disputed memory. */
export function describeDispute(d: DisputeNote): string {
  const a = clip(d.side_a.text)
  const b = clip(d.side_b.text)
  if (needsUser(d)) {
    return `Memory conflict (dispute ${d.id}) — ask the user which is right before relying on either: A: "${a}" / B: "${b}"`
  }
  const why = d.reason ? ` because ${clip(d.reason, 200)}` : ""
  const scope = d.scope ? ` (scope: ${clip(d.scope, 80)})` : ""
  const picked =
    d.pick === "a" || d.pick === "b"
      ? `picked ${d.pick.toUpperCase()}${why}`
      : d.pick === "both_scoped"
        ? `kept both, each in its own context${scope}${why}`
        : d.pick === "neither"
          ? `kept neither${why}`
          : `not settled yet${why}`
  return `Memory conflict: A: "${a}" / B: "${b}" — ${picked} (you may mention the conflict if it matters)`
}

/** The "Also related" lines for server recall hits; a disputed hit gets its conflict note under it. */
export function relatedLines(hits: { text: string; dispute?: unknown }[]): { lines: string[]; disputes: DisputeNote[] } {
  const disputes = new Map<string, DisputeNote>()
  const lines = hits.map((k) => {
    const first = `- ${k.text.split("\n")[0]}`
    const d = readDispute(k.dispute)
    if (!d) return first
    disputes.set(d.id, d)
    return `${first}\n  ${describeDispute(d)}`
  })
  return { lines, disputes: [...disputes.values()] }
}

// ── asking, once per dispute per session ───────────────────────────────────

const asked = new Map<string, Set<string>>()

export function resetAskedDisputesForTests(): void {
  asked.clear()
}

const LABELS: Record<DisputeChoice, string> = {
  a: "A",
  b: "B",
  both_scoped: "Both, in different contexts",
  neither: "Neither",
}

/** One four-option question for a dispute. Labels carry the side text (clipped), descriptions the full text. */
export function disputeQuestion(d: DisputeNote) {
  const label = (prefix: string, t?: string | null) => `${prefix}: ${clip(t, 40)}`
  const labels: Record<DisputeChoice, string> = {
    a: label("A", d.side_a.text),
    b: label("B", d.side_b.text),
    both_scoped: LABELS.both_scoped,
    neither: LABELS.neither,
  }
  return {
    labels,
    question: {
      question: "Two of your memories disagree. Which is right?",
      header: "Memory conflict",
      custom: false,
      options: [
        { label: labels.a, description: clip(d.side_a.text, 400) },
        { label: labels.b, description: clip(d.side_b.text, 400) },
        { label: labels.both_scoped, description: "Both are true, each in its own context" },
        { label: labels.neither, description: "Neither is right; drop both" },
      ],
    },
  }
}

export interface AskDeps {
  /** Raises a question and waits for the reply (the question integration). */
  ask: (input: { sessionID: string; questions: any[] }) => Promise<string[][]>
  /** The network edge: POST /memory/disputes/:id/resolve. */
  resolve: (id: string, choice: DisputeChoice, reason?: string) => Promise<DisputeResolveResult>
}

export type DisputeResolveResult = { kind: "ok" } | { kind: "quiet"; message: string } | { kind: "error"; message: string }

/** Ask once per dispute id per session; returns one note line per dispute asked ("" when nothing was asked). */
export async function askDisputes(sessionID: string, disputes: DisputeNote[], deps: AskDeps): Promise<string[]> {
  const done = asked.get(sessionID) ?? new Set<string>()
  asked.set(sessionID, done)
  const notes: string[] = []
  for (const d of disputes) {
    if (!needsUser(d) || done.has(d.id)) continue
    done.add(d.id) // before asking: never twice, even if the reply fails
    const { labels, question } = disputeQuestion(d)
    let answers: string[][]
    try {
      answers = await deps.ask({ sessionID, questions: [question] })
    } catch {
      notes.push(`The user did not answer the question about memory conflict ${d.id}; do not rely on either side.`)
      continue
    }
    const picked = (Object.keys(labels) as DisputeChoice[]).find((k) => labels[k] === answers[0]?.[0])
    if (!picked) {
      notes.push(`The user did not choose for memory conflict ${d.id}; do not rely on either side.`)
      continue
    }
    const r = await deps.resolve(d.id, picked, "chosen by the user in a gizzi session")
    notes.push(
      r.kind === "ok"
        ? `The user settled memory conflict ${d.id}: ${labels[picked]}. It is recorded.`
        : `The user chose for memory conflict ${d.id}: ${labels[picked]}. (${r.message})`,
    )
  }
  return notes
}

/** The real resolve call. A 409 (sleep off, already settled) or an older runtime is quiet, not an error. */
export async function resolveDispute(id: string, choice: DisputeChoice, reason?: string): Promise<DisputeResolveResult> {
  let out: SleepOutcome
  try {
    out = await sleepCall("POST", `disputes/${encodeURIComponent(id)}/resolve`, reason ? { choice, reason } : { choice })
  } catch (e: any) {
    return { kind: "error", message: `could not record it: ${e?.message ?? "offline"}` }
  }
  if (out.kind === "ok") return { kind: "ok" }
  if (out.kind === "refused") return { kind: "quiet", message: out.message }
  if (out.kind === "no_sleep") return { kind: "quiet", message: out.message }
  if (out.status === 409) return { kind: "quiet", message: "already settled or Sleep is off; nothing to record" }
  return { kind: "error", message: out.message }
}
