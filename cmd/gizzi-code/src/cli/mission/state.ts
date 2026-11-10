/**
 * Missions: a long-horizon goal that runs on its own until done.
 *
 *   plan      the orchestrator (the session's brain, in an ordinary engine
 *             session) writes milestones; each has parallel tracks and a
 *             validation command. The person approves it (or edits it).
 *   run       each milestone runs its tracks as Factory swarm workers (fresh,
 *             narrow context, a worktree each), lands them on the mission
 *             branch, then runs the validation command there. A failure goes
 *             back to the orchestrator, which may add one fix track; after
 *             `maxAttempts` failed validations the mission stops and asks.
 *             A passing milestone starts the next one; after the last,
 *             the mission branch is landed into the branch the mission
 *             started from (fast-forward, or `gizzi mission land`).
 *
 * This file is the pure part: the record, the plan format the person edits,
 * and the state machine (`advance` folds an event in, `nextAction` says what
 * the runner does next). runner.ts performs the actions; store.ts keeps the
 * record at `.allternit/missions/<id>.json` so `gizzi mission resume` picks
 * up after a restart.
 */

export type MissionStatus = "planning" | "awaiting_approval" | "running" | "needs_you" | "done" | "stopped" | "failed"

export type MilestoneStatus = "pending" | "working" | "validating" | "fixing" | "passed" | "failed"

export interface Track {
  title: string
  task: string
}

export interface Milestone {
  /** "M1", "M2"… */
  id: string
  title: string
  tracks: Track[]
  /** Shell command run in the mission tree; exit 0 means the milestone is done. */
  validate: string
  status: MilestoneStatus
  /** Tracks of `tracks` already handed to workers (batches of maxWorkers). */
  started: number
  /** Fix tracks the orchestrator added after failed validations. */
  fixes: Track[]
  /** Fix tracks already handed to a worker. */
  fixesStarted: number
  /** Validation runs that failed. */
  attempts: number
  /** Swarm ids, in order; the last one is the batch being worked on. */
  swarms: string[]
  /** Worker addresses across all swarms of this milestone. */
  workers: string[]
  lastFailure?: string
}

export interface MissionLogLine {
  at: number
  text: string
}

export interface Mission {
  id: string
  goal: string
  /** The repo the person started it in. */
  root: string
  /** The branch it started from; passing milestones merge into it. */
  baseBranch: string
  /** `mission/<id>`: where workers land and validation runs. */
  branch: string
  /**
   * The mission's own git worktree of `branch`, outside the project
   * (`~/.allternit/mission-checkouts/<repo>-<hash>/<id>`, see
   * `missionWorktree`), so the person's checkout is never switched and the
   * project's test runners and watchers never see it. Older records keep
   * theirs at `<root>/.allternit/missions/<id>/checkout`. Workers start from it, land into it and validation runs
   * in it. Equal to `root` only in old records and tests.
   */
  worktree: string
  /** Orchestrator and worker brain (a CLI brain, e.g. claude-cli/claude-sonnet-5). */
  brain: string
  /** The orchestrator's engine session (Remote Control shows and steers it). */
  orchestratorSession?: string
  /** The engine it was attached to, when one was serving. */
  engineURL?: string
  maxWorkers: number
  maxHours?: number
  /** Failed validations a milestone may have before the mission stops and asks. */
  maxAttempts: number
  status: MissionStatus
  milestones: Milestone[]
  /** Index of the milestone being worked on. */
  current: number
  createdAt: number
  startedAt?: number
  endedAt?: number
  /** Why it stopped / what it needs. */
  note?: string
  /** pid of the background runner, while one runs. */
  runnerPid?: number
  /**
   * Set when started with --on: the paired computer the orchestrator's plan
   * and fix turns run on (workers, validation and landing stay on this
   * computer, in `worktree`). `runtime` is what the person typed; `id` and
   * `name` are the runtime it resolved to; `sessionID` / `web` are the
   * orchestrator's session there and where to follow it.
   */
  on?: { runtime: string; id?: string; name?: string; sessionID?: string; web?: string }
  /**
   * After the last milestone passes: true once `branch` is merged into
   * `baseBranch` in `root`; false (with `landNote` saying why) when that
   * couldn't happen on its own, so `gizzi mission land <id>` is next.
   */
  landed?: boolean
  landNote?: string
  log: MissionLogLine[]
}

export const DEFAULT_MAX_WORKERS = 3
export const DEFAULT_MAX_ATTEMPTS = 2
const LOG_KEEP = 200

// ---------------------------------------------------------------- plan

export interface PlanInput {
  milestones: { title: string; tracks: Track[]; validate: string }[]
}

export function newMilestones(plan: PlanInput): Milestone[] {
  return plan.milestones.map((m, i) => ({
    id: `M${i + 1}`,
    title: m.title.trim(),
    tracks: m.tracks.map((t) => ({ title: t.title.trim(), task: t.task.trim() })),
    validate: m.validate.trim(),
    status: "pending",
    started: 0,
    fixes: [],
    fixesStarted: 0,
    attempts: 0,
    swarms: [],
    workers: [],
  }))
}

/** Problems that make a plan unrunnable; empty when it is fine. */
export function planProblems(plan: PlanInput): string[] {
  const out: string[] = []
  if (!plan.milestones.length) out.push("the plan has no milestones")
  plan.milestones.forEach((m, i) => {
    const id = `M${i + 1}`
    if (!m.title.trim()) out.push(`${id} has no title`)
    if (!m.tracks.length) out.push(`${id} has no tracks`)
    if (!m.validate.trim()) out.push(`${id} has no validation command`)
    m.tracks.forEach((t, j) => {
      if (!t.task.trim()) out.push(`${id} track ${j + 1} has no task`)
    })
  })
  return out
}

/**
 * The plan as the person reads and edits it:
 *
 *   # Mission: <goal>
 *
 *   ## M1: <title>
 *   - <track title>: <task>
 *   validate: `<command>`
 */
export function planToMarkdown(goal: string, plan: PlanInput): string {
  const lines = [`# Mission: ${goal}`, "", "<!-- Edit freely: one `## M<n>: title` per milestone, one `- title: task` per parallel track, one `validate:` command each. -->", ""]
  plan.milestones.forEach((m, i) => {
    lines.push(`## M${i + 1}: ${m.title}`)
    for (const t of m.tracks) lines.push(`- ${t.title}: ${t.task.replace(/\s*\n\s*/g, " ")}`)
    lines.push(`validate: \`${m.validate}\``, "")
  })
  return lines.join("\n")
}

export function markdownToPlan(md: string): PlanInput {
  const milestones: PlanInput["milestones"] = []
  let cur: PlanInput["milestones"][number] | undefined
  for (const raw of md.replace(/<!--[\s\S]*?-->/g, "").split("\n")) {
    const line = raw.trim()
    const head = /^##\s+(?:M\d+\s*[:.\-—]\s*)?(.+)$/.exec(line)
    if (head) {
      cur = { title: head[1]!.trim(), tracks: [], validate: "" }
      milestones.push(cur)
      continue
    }
    if (!cur) continue
    const val = /^validat(?:e|ion)\s*:\s*(.+)$/i.exec(line)
    if (val) {
      cur.validate = val[1]!.trim().replace(/^`+|`+$/g, "").trim()
      continue
    }
    const track = /^[-*]\s+(.+)$/.exec(line)
    if (track) {
      const body = track[1]!.trim()
      const sep = body.indexOf(": ")
      cur.tracks.push(sep > 0 ? { title: body.slice(0, sep).trim(), task: body.slice(sep + 2).trim() } : { title: body.slice(0, 48), task: body })
    }
  }
  return { milestones }
}

/** The JSON object in an orchestrator reply (bare, or in a ```json fence). */
export function extractJson(text: string): unknown {
  const fenced = /```(?:json)?\s*([\s\S]*?)```/.exec(text)
  const candidates = [fenced?.[1], text]
  for (const c of candidates) {
    if (!c) continue
    const start = c.indexOf("{")
    const end = c.lastIndexOf("}")
    if (start < 0 || end <= start) continue
    try {
      return JSON.parse(c.slice(start, end + 1))
    } catch {
      // try the next candidate
    }
  }
  return undefined
}

/** An orchestrator's plan reply → PlanInput (tolerant of small shape drift). */
export function parsePlanReply(text: string): PlanInput | undefined {
  const data = extractJson(text) as { milestones?: unknown } | undefined
  if (!data || !Array.isArray(data.milestones)) return undefined
  const str = (v: unknown) => (typeof v === "string" ? v : "")
  return {
    milestones: data.milestones.map((m: any) => ({
      title: str(m?.title),
      validate: str(m?.validate ?? m?.validation ?? m?.check),
      tracks: (Array.isArray(m?.tracks) ? m.tracks : []).map((t: any) =>
        typeof t === "string" ? { title: t.slice(0, 48), task: t } : { title: str(t?.title) || str(t?.task).slice(0, 48), task: str(t?.task ?? t?.description) },
      ),
    })),
  }
}

/** A fix reply: `{"fix":{"title","task"}}` or `{"stop":"<reason>"}`. */
export function parseFixReply(text: string): { fix: Track } | { stop: string } | undefined {
  const data = extractJson(text) as { fix?: { title?: string; task?: string }; stop?: string } | undefined
  if (!data) return undefined
  if (typeof data.stop === "string" && data.stop.trim()) return { stop: data.stop.trim() }
  if (data.fix && typeof data.fix.task === "string" && data.fix.task.trim()) {
    return { fix: { title: (data.fix.title ?? "fix").trim() || "fix", task: data.fix.task.trim() } }
  }
  return undefined
}

// ---------------------------------------------------------------- state machine

export type MissionEvent =
  | { type: "planned"; plan: PlanInput; session?: string }
  | { type: "approved"; plan?: PlanInput }
  | { type: "tracks_started"; swarm: string; workers: string[]; count: number }
  | { type: "fix_started"; swarm: string; workers: string[] }
  | { type: "landed"; note?: string }
  | { type: "validated"; ok: boolean; output?: string }
  | { type: "fix_planned"; fix: Track }
  | { type: "fix_declined"; reason: string }
  | { type: "merged"; note?: string }
  | { type: "budget"; reason: string }
  | { type: "stop"; reason?: string }
  | { type: "resume" }
  | { type: "error"; text: string }

export function log(m: Mission, text: string, now = Date.now()): Mission {
  return { ...m, log: [...m.log, { at: now, text }].slice(-LOG_KEEP) }
}

function patchMilestone(m: Mission, patch: Partial<Milestone>): Mission {
  return { ...m, milestones: m.milestones.map((ms, i) => (i === m.current ? { ...ms, ...patch } : ms)) }
}

export function currentMilestone(m: Mission): Milestone | undefined {
  return m.milestones[m.current]
}

const ENDED: MissionStatus[] = ["done", "stopped", "failed"]

/** Fold one event into a mission. Pure. */
export function advance(m: Mission, e: MissionEvent, now = Date.now()): Mission {
  const ms = currentMilestone(m)
  switch (e.type) {
    case "planned":
      return log({ ...m, status: "awaiting_approval", milestones: newMilestones(e.plan), orchestratorSession: e.session ?? m.orchestratorSession }, `planned ${e.plan.milestones.length} milestones`, now)
    case "approved": {
      if (m.status !== "awaiting_approval" && m.status !== "planning") return m
      const milestones = e.plan ? newMilestones(e.plan) : m.milestones
      return log({ ...m, status: "running", milestones, current: 0, startedAt: now, note: undefined }, "approved", now)
    }
    case "tracks_started": {
      if (!ms) return m
      const next = patchMilestone(m, { status: "working", started: ms.started + e.count, swarms: [...ms.swarms, e.swarm], workers: [...ms.workers, ...e.workers] })
      return log(next, `${ms.id}: ${e.count} track${e.count === 1 ? "" : "s"} started (swarm ${e.swarm})`, now)
    }
    case "fix_started": {
      if (!ms) return m
      return log(patchMilestone(m, { status: "working", fixesStarted: ms.fixesStarted + 1, swarms: [...ms.swarms, e.swarm], workers: [...ms.workers, ...e.workers] }), `${ms.id}: fix track started (swarm ${e.swarm})`, now)
    }
    case "landed": {
      if (!ms) return m
      // More tracks than maxWorkers: the next batch starts before validating
      // (also after a fix for a batch whose landing conflicted).
      const status: MilestoneStatus = ms.started < ms.tracks.length ? "pending" : "validating"
      return log(patchMilestone(m, { status }), `${ms.id}: workers landed${e.note ? ` (${e.note})` : ""}`, now)
    }
    case "validated": {
      if (!ms) return m
      if (e.ok) {
        const passed = patchMilestone(m, { status: "passed", lastFailure: undefined })
        return log(passed, `${ms.id}: validation passed`, now)
      }
      const attempts = ms.attempts + 1
      const failure = (e.output ?? "").slice(-4000)
      if (attempts >= m.maxAttempts + 1) {
        const failed = patchMilestone(m, { status: "failed", attempts, lastFailure: failure })
        return log({ ...failed, status: "needs_you", note: `${ms.id} still fails validation after ${attempts} tries; fix it or edit the plan, then \`gizzi mission resume ${m.id}\`` }, `${ms.id}: validation failed (${attempts}/${m.maxAttempts + 1}); stopping to ask`, now)
      }
      return log(patchMilestone(m, { status: "fixing", attempts, lastFailure: failure }), `${ms.id}: validation failed (${attempts}/${m.maxAttempts + 1}); asking the orchestrator for a fix`, now)
    }
    case "fix_planned": {
      if (!ms) return m
      return log(patchMilestone(m, { fixes: [...ms.fixes, e.fix] }), `${ms.id}: fix track "${e.fix.title}"`, now)
    }
    case "fix_declined":
      if (!ms) return m
      return log({ ...patchMilestone(m, { status: "failed" }), status: "needs_you", note: `${ms.id}: the orchestrator stopped: ${e.reason}` }, `${ms.id}: orchestrator stopped (${e.reason})`, now)
    case "merged": {
      if (!ms) return m
      const nextIndex = m.current + 1
      const done = nextIndex >= m.milestones.length
      const moved = { ...m, current: done ? m.current : nextIndex }
      const after = done ? { ...moved, status: "done" as const, endedAt: now, note: e.note } : { ...moved, note: e.note }
      return log(after, done ? `mission done${e.note ? ` (${e.note})` : ""}` : `${ms.id} merged${e.note ? ` (${e.note})` : ""}; starting ${m.milestones[nextIndex]!.id}`, now)
    }
    case "budget":
      // Over --max-hours: a person decides (raise the limit, then resume); nothing is lost.
      if (ENDED.includes(m.status) || m.status === "needs_you") return m
      return log({ ...m, status: "needs_you", note: e.reason }, `needs you: ${e.reason}`, now)
    case "stop":
      if (ENDED.includes(m.status)) return m
      return log({ ...m, status: "stopped", endedAt: now, note: e.reason ?? "stopped by you" }, `stopped${e.reason ? `: ${e.reason}` : ""}`, now)
    case "resume": {
      if (m.status !== "needs_you" && m.status !== "stopped") return m
      // Stopped before anyone approved the plan: back to approval, never straight to running.
      if (!m.startedAt) return log({ ...m, status: m.milestones.length ? "awaiting_approval" : "planning", endedAt: undefined, note: undefined }, "resumed (awaiting approval)", now)
      // A failed milestone is validated again first (the person may have fixed it by hand), with a fresh budget of fixes.
      const reset = ms && ms.status === "failed" ? patchMilestone(m, { status: "validating", attempts: 0 }) : m
      return log({ ...reset, status: "running", endedAt: undefined, note: undefined }, "resumed", now)
    }
    case "error":
      if (ENDED.includes(m.status)) return m
      return log({ ...m, status: "needs_you", note: e.text }, `needs you: ${e.text}`, now)
  }
}

export type MissionAction =
  | { kind: "none" }
  | { kind: "start_tracks"; tracks: Track[] }
  | { kind: "wait"; swarm: string }
  | { kind: "validate"; command: string }
  | { kind: "plan_fix" }
  | { kind: "start_fix"; fix: Track }
  | { kind: "merge" }

/** What the runner does next for a mission. Pure. */
export function nextAction(m: Mission): MissionAction {
  if (m.status !== "running") return { kind: "none" }
  const ms = currentMilestone(m)
  if (!ms) return { kind: "none" }
  switch (ms.status) {
    case "pending":
      return { kind: "start_tracks", tracks: ms.tracks.slice(ms.started, ms.started + Math.max(1, m.maxWorkers)) }
    case "working":
      return { kind: "wait", swarm: ms.swarms[ms.swarms.length - 1]! }
    case "validating":
      return { kind: "validate", command: ms.validate }
    case "fixing":
      // A fix planned but not yet handed to a worker starts; else ask the orchestrator for one.
      return ms.fixes.length > ms.fixesStarted ? { kind: "start_fix", fix: ms.fixes[ms.fixes.length - 1]! } : { kind: "plan_fix" }
    case "passed":
      return { kind: "merge" }
    case "failed":
      return { kind: "none" }
  }
}

/** "◆ M2/5" for the footer; null when nothing runs. */
export function badge(m: Mission): string {
  const n = Math.min(m.current + 1, m.milestones.length)
  return `◆ M${n}/${m.milestones.length}`
}

/** "Mission · M2/5" — the label Remote Control surfaces show on the orchestrator session. */
export function label(m: Mission): string {
  const n = Math.min(m.current + 1, m.milestones.length)
  const tail = m.status === "done" ? " · done" : m.status === "needs_you" ? " · needs you" : m.status === "stopped" ? " · stopped" : m.status === "failed" ? " · failed" : ""
  return `Mission · M${n}/${m.milestones.length || "?"}${tail}`
}

/** Hours used against --max-hours. */
export function overHours(m: Mission, now = Date.now()): boolean {
  return !!m.maxHours && !!m.startedAt && now - m.startedAt > m.maxHours * 3600_000
}
