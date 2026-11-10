/**
 * The mission runner: the background process that drives one mission to the
 * end. Each turn it asks the state machine what to do (`nextAction`), does
 * it, folds the outcome in (`advance`) and writes the record, so a restart
 * picks up exactly where it was: `gizzi mission resume` applies `resume` and
 * starts a runner again, and an in-flight swarm is watched again, not
 * restarted.
 *
 * The runner only works a mission whose status is `running`; the caller
 * applies `approved` / `resume` first. It stops on any other status
 * (done, needs_you, stopped, failed). To stop a running mission, send its
 * `runnerPid` SIGTERM: the runner stops the open swarm's workers, records
 * `stopped` and exits.
 *
 * Git: the record stays at `root` (the person's checkout), but the work
 * happens in the mission's own worktree (`m.worktree`, on `mission/<id>`,
 * made from the start branch the first time), so the person's checkout is
 * never switched. Swarms start from that worktree, so `landSwarm` merges
 * every worker into the mission branch, and validation runs there. When the
 * last milestone passes, the mission branch is landed into the start branch
 * if that is a clean fast-forward; otherwise it waits for
 * `gizzi mission land <id>`.
 *
 * With --on, the orchestrator's fix turns run on the paired runtime (see
 * orchestrator.ts); everything else here stays on this computer.
 */
import fs from "node:fs"
import { isSettled, type SwarmRecord } from "@/cli/factory/swarm"
import { ensureWorktree, killChildren, landAndRecord, realDeps, type MissionDeps } from "@/cli/mission/actions"
import { fixPrompt, onUnsupported } from "@/cli/mission/orchestrator"
import {
  advance,
  currentMilestone,
  nextAction,
  overHours,
  parseFixReply,
  type Milestone,
  type Mission,
  type MissionAction,
  type MissionEvent,
  type Track,
} from "@/cli/mission/state"
import { logFile, readMission, runnerAlive, writeMission } from "@/cli/mission/store"

export interface RunOptions {
  /** How often an open swarm is looked at (default 5 s). */
  pollMs?: number
  /** Aborting it stops the mission the way SIGTERM does (without exiting the process). */
  signal?: AbortSignal
  /** Stop on SIGTERM / SIGINT, then exit the process (default true). */
  handleSignals?: boolean
}

export const POLL_MS = 5000
/** Validation output kept for the orchestrator's fix prompt. */
export const FAILURE_TAIL = 3000

function errText(err: unknown): string {
  const e = err as { fact?: string; action?: string; message?: string }
  if (e?.fact) return e.action ? `${e.fact}. ${e.action}` : e.fact
  return e?.message ?? String(err)
}

/** The task one worker gets for one track: the track, plus where it sits in the mission. */
export function trackTask(m: Pick<Mission, "id" | "goal">, ms: Pick<Milestone, "id" | "title" | "validate">, t: Track): string {
  return [
    `${t.title}: ${t.task}`,
    "",
    `This is one track of mission ${m.id} (goal: ${m.goal}), milestone ${ms.id} "${ms.title}". Other workers do the other tracks at the same time, so keep to this task. Once every track is merged, this must pass from the repo root: ${ms.validate}`,
  ].join("\n")
}

function describe(a: MissionAction, ms: Milestone | undefined): string {
  const id = ms?.id ?? "?"
  switch (a.kind) {
    case "start_tracks":
      return `${id}: starting ${a.tracks.length} track${a.tracks.length === 1 ? "" : "s"} (${a.tracks.map((t) => t.title).join(", ")})`
    case "start_fix":
      return `${id}: starting fix track "${a.fix.title}"`
    case "wait":
      return `${id}: watching swarm ${a.swarm}`
    case "validate":
      return `${id}: validating: ${a.command}`
    case "plan_fix":
      return `${id}: asking the orchestrator for a fix`
    case "merge":
      return `${id}: passed; recording it`
    case "none":
      return `${id}: nothing to do`
  }
}

/**
 * Drive mission `id` (recorded under `root`) until it is no longer running.
 * Resolves with the mission as last written.
 */
export async function runMission(id: string, root: string, deps: MissionDeps = realDeps(), opts: RunOptions = {}): Promise<Mission> {
  const pollMs = opts.pollMs ?? POLL_MS
  const first = readMission(root, id)
  if (!first) throw new Error(`No mission ${id} in ${root}`)
  if (first.runnerPid && first.runnerPid !== process.pid && runnerAlive(first)) {
    throw new Error(`Mission ${id} already has a runner (pid ${first.runnerPid})`)
  }
  let m: Mission = first

  const line = (text: string) => {
    try {
      fs.appendFileSync(logFile(m), `${new Date(deps.now()).toISOString()} ${text}\n`)
    } catch {
      // the log is a convenience; the record is the truth
    }
  }

  if (m.status !== "running") {
    line(`runner: mission is ${m.status}, nothing to run`)
    return m
  }
  // --on: the orchestrator's runtime must still be paired and reachable; never fall back to running it here.
  const unsupported = await onUnsupported(m)
  if (unsupported) {
    m = advance(m, { type: "error", text: unsupported }, deps.now())
    writeMission(m)
    line(`runner: ${unsupported}`)
    return m
  }
  /** Where git, swarms and validation run: the mission's worktree. */
  const work = () => m.worktree || root

  // ------------------------------------------------------------ stop

  let stopping = false
  let stopDone: Promise<void> = Promise.resolve()
  let wake: (() => void) | undefined
  const sleep = (ms: number) =>
    new Promise<void>((resolve) => {
      const t = setTimeout(done, ms)
      function done() {
        clearTimeout(t)
        wake = undefined
        resolve()
      }
      wake = done
    })

  const save = (next: Mission) => {
    if (stopping) return
    m = next
    writeMission(m)
  }
  const apply = (e: MissionEvent) => save(advance(m, e, deps.now()))

  /** Stop the workers of a swarm that are still going; the finished ones keep their work for landing. */
  async function stopOpen(swarmId: string): Promise<void> {
    let rec = deps.readSwarm(swarmId)
    if (!rec) return
    for (const w of rec.workers) {
      if (isSettled(w)) continue
      try {
        rec = await deps.stopWorker(rec, w.bot)
        line(`stopped worker ${w.address}`)
      } catch (err) {
        line(`couldn't stop worker ${w.address}: ${errText(err)}`)
      }
    }
  }

  function stop(reason: string): Promise<void> {
    if (stopping) return stopDone
    stopping = true
    wake?.()
    // An orchestrator turn or validation in flight ends now, not when it finishes.
    killChildren()
    stopDone = (async () => {
      const ms = currentMilestone(m)
      const open = ms?.status === "working" ? ms.swarms[ms.swarms.length - 1] : undefined
      if (open) await stopOpen(open)
      m = advance({ ...m, runnerPid: undefined }, { type: "stop", reason }, deps.now())
      writeMission(m)
      line(`stopped: ${reason}`)
      await deps.labelSession(m).catch(() => {})
    })()
    return stopDone
  }

  const onSignal = (sig: NodeJS.Signals) => {
    void stop(`stopped (${sig})`).finally(() => {
      killChildren()
      process.exit(0)
    })
  }
  const onAbort = () => void stop("stopped by you")
  if (opts.handleSignals !== false) {
    process.once("SIGTERM", onSignal)
    process.once("SIGINT", onSignal)
  }
  opts.signal?.addEventListener("abort", onAbort)
  if (opts.signal?.aborted) onAbort()

  // ------------------------------------------------------------ git

  /** Put the mission's worktree on the mission branch, making both the first time. */
  function ensureBranch(): boolean {
    const branch = m.branch || `mission/${m.id}`
    const wt = ensureWorktree({ ...m, branch }, deps.git)
    if ("error" in wt) {
      apply({ type: "error", text: wt.error })
      return false
    }
    if (wt.created) line(`made ${work()} on ${branch} (from ${m.baseBranch})`)
    const dir = work()
    const head = deps.git(["rev-parse", "--abbrev-ref", "HEAD"], dir)
    if (head.code === 0 && head.output.trim() === branch) {
      if (m.branch !== branch) save({ ...m, branch })
      return true
    }
    const has = deps.git(["rev-parse", "--verify", "--quiet", `refs/heads/${branch}`], dir).code === 0
    if (!has) {
      const made = deps.git(["branch", branch, m.baseBranch], dir)
      if (made.code !== 0) {
        apply({ type: "error", text: `couldn't create ${branch} from ${m.baseBranch}: ${made.output}` })
        return false
      }
      line(`created ${branch} from ${m.baseBranch}`)
    }
    const sw = deps.git(["switch", branch], dir)
    if (sw.code !== 0) {
      apply({ type: "error", text: `couldn't switch ${dir} to ${branch} (commit or stash what's there, then resume): ${sw.output}` })
      return false
    }
    line(`switched ${dir} to ${branch}`)
    if (m.branch !== branch) save({ ...m, branch })
    return true
  }

  // ------------------------------------------------------------ actions

  async function start(ms: Milestone, tracks: Track[], fix: boolean): Promise<void> {
    if (!ensureBranch()) return
    const { record } = await deps.startSwarm({
      task: `${m.id} ${ms.id}: ${tracks.map((t) => t.title).join(", ")}`,
      tasks: tracks.map((t) => trackTask(m, ms, t)),
      workers: Math.min(tracks.length, Math.max(1, m.maxWorkers)),
      brain: m.brain,
      cwd: work(),
      log: line,
    })
    const workers = record.workers.map((w) => w.address)
    const event: MissionEvent = fix ? { type: "fix_started", swarm: record.id, workers } : { type: "tracks_started", swarm: record.id, workers, count: tracks.length }
    if (stopping) {
      // Stopped while the panes came up: stop them too, and record the swarm so resume finds it.
      await stopDone
      await stopOpen(record.id)
      m = advance(m, event, deps.now())
      writeMission(m)
      return
    }
    apply(event)
  }

  async function wait(swarmId: string): Promise<void> {
    let rec: SwarmRecord | null = deps.readSwarm(swarmId)
    if (!rec) {
      apply({ type: "error", text: `swarm ${swarmId} has no record on this computer` })
      return
    }
    for (;;) {
      rec = await deps.observeSwarm(rec)
      if (stopping) return
      if (rec.workers.every(isSettled)) break
      if (overHours(m, deps.now())) return
      await sleep(pollMs)
      if (stopping) return
    }
    if (rec.workers.length && rec.workers.every((w) => w.phase === "failed")) {
      const why = [...new Set(rec.workers.map((w) => w.note).filter(Boolean))].join("; ")
      apply({ type: "error", text: `every worker of swarm ${swarmId} failed${why ? `: ${why}` : ""}` })
      return
    }
    // landSwarm merges into the branch the swarm started from: the mission branch (after a resume, too).
    if (!ensureBranch()) return
    const steps = await deps.landSwarm(rec, { dryRun: false })
    if (stopping) return
    const merged = steps.filter((s) => s.outcome === "merged").length
    const conflicts = steps.filter((s) => s.outcome === "conflict" || s.outcome === "failed")
    apply({ type: "landed", note: `${merged} merged${conflicts.length ? `, ${conflicts.length} conflicted` : ""}` })
    for (const s of steps) line(`  ${s.bot} ${s.branch}: ${s.outcome ?? s.action}${s.detail ? ` (${s.detail})` : ""}`)
    if (conflicts.length) {
      // A conflict is a failed attempt: the orchestrator gets it as the failure and can add a fix track.
      const output = [
        `Landing swarm ${swarmId} on ${m.branch} hit merge conflicts, so validation did not run.`,
        ...conflicts.map((s) => `- ${s.branch}: ${s.detail ?? s.outcome}`),
        "Each conflicting branch is kept; a fix track can `git merge` it and resolve the conflict.",
      ].join("\n")
      apply({ type: "validated", ok: false, output })
    }
  }

  async function validate(command: string): Promise<void> {
    if (!ensureBranch()) return
    const r = await deps.exec(command, work())
    if (stopping) return
    line(`  exit ${r.code}`)
    apply({ type: "validated", ok: r.code === 0, output: r.output.slice(-FAILURE_TAIL) })
  }

  async function planFix(ms: Milestone): Promise<void> {
    const stat = m.branch ? deps.git(["diff", "--stat", `${m.baseBranch}...${m.branch}`], work()) : undefined
    const changes = stat && stat.code === 0 ? stat.output : undefined
    const turn = await deps.orchestratorTurn(m, fixPrompt(m, ms.attempts, changes), { engine: m.engineURL, cwd: root })
    if (stopping) return
    if (turn.sessionID && !m.orchestratorSession) save({ ...m, orchestratorSession: turn.sessionID })
    if (turn.on && (turn.on.sessionID !== m.on?.sessionID || turn.on.web !== m.on?.web)) save({ ...m, on: turn.on })
    if (turn.error) {
      apply({ type: "error", text: `the orchestrator couldn't plan a fix: ${turn.error}` })
      return
    }
    const reply = parseFixReply(turn.text)
    if (!reply) {
      apply({ type: "error", text: `the orchestrator's reply was neither a fix nor a stop: ${turn.text.slice(0, 200)}` })
      return
    }
    if ("fix" in reply) apply({ type: "fix_planned", fix: reply.fix })
    else apply({ type: "fix_declined", reason: reply.stop })
  }

  async function perform(a: MissionAction): Promise<void> {
    const ms = currentMilestone(m)
    switch (a.kind) {
      case "start_tracks":
        return start(ms!, a.tracks, false)
      case "start_fix":
        return start(ms!, [a.fix], true)
      case "wait":
        return wait(a.swarm)
      case "validate":
        return validate(a.command)
      case "plan_fix":
        return planFix(ms!)
      case "merge":
        apply({ type: "merged" })
        if (m.status === "done") {
          // The last milestone passed: land the mission branch into the start branch when that's a clean fast-forward.
          save(landAndRecord(m, (args, cwd) => deps.git(args, cwd), { now: deps.now() }))
          line(m.landed ? `landed: ${m.landNote}` : `ready to land: gizzi mission land ${m.id} (${m.landNote})`)
        }
        await deps.labelSession(m).catch(() => {})
        return
      case "none":
        apply({ type: "error", text: `${ms?.id ?? "the mission"} has nothing to do next (milestone ${ms?.status ?? "missing"})` })
        return
    }
  }

  // ------------------------------------------------------------ loop

  save({ ...m, runnerPid: process.pid })
  line(`runner ${process.pid} started (${m.status}, ${m.milestones[m.current]?.id ?? "-"})`)
  try {
    while (!stopping && m.status === "running") {
      // Another process (`gizzi mission stop` without a live runner pid, an edit) may have ended it: the record on disk wins.
      const disk = readMission(root, id)
      if (disk && disk.status !== "running") {
        m = disk
        break
      }
      if (overHours(m, deps.now())) {
        const ms = currentMilestone(m)
        if (ms?.status === "working" && ms.swarms.length) await stopOpen(ms.swarms[ms.swarms.length - 1]!)
        apply({ type: "budget", reason: `over --max-hours ${m.maxHours}: raise it, then \`gizzi mission resume ${m.id}\`` })
        line(`over --max-hours ${m.maxHours}`)
        break
      }
      const a = nextAction(m)
      line(describe(a, currentMilestone(m)))
      try {
        await perform(a)
      } catch (err) {
        if (stopping) break
        line(`  failed: ${errText(err)}`)
        apply({ type: "error", text: errText(err) })
      }
    }
  } finally {
    opts.signal?.removeEventListener("abort", onAbort)
    process.removeListener("SIGTERM", onSignal)
    process.removeListener("SIGINT", onSignal)
  }

  if (stopping) {
    await stopDone
    return m
  }
  if (m.runnerPid === process.pid) {
    m = { ...m, runnerPid: undefined }
    writeMission(m)
  }
  line(`runner exits: ${m.status}${m.note ? ` (${m.note})` : ""}`)
  await deps.labelSession(m).catch(() => {})
  return m
}
