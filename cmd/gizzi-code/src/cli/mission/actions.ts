/**
 * Everything the mission runner touches outside its own record: Factory
 * swarms, the orchestrator's engine session, shell commands and git. The
 * runner takes these as one `MissionDeps` object so tests can hand it fakes;
 * `realDeps()` is the production set.
 *
 * Also the mission steps the CLI (`gizzi mission`) and the TUI (`/mission`)
 * share, so neither needs the other's prompts: `newMission` → `planMission`
 * → (the person approves) → `approveAndSpawn`; `prepareResume`, `stopMission`
 * and `landMission` for the rest.
 */
import crypto from "node:crypto"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { spawn, spawnSync } from "node:child_process"
import { runEngine } from "@/cli/factory/engine"
import {
  drainReady,
  isSettled,
  landSwarm,
  observeSwarm,
  readSwarm,
  startSwarm,
  stopWorker,
  type LandStep,
  type StartOptions,
  type StartPlan,
  type SwarmRecord,
} from "@/cli/factory/swarm"
import type { FactoryAgent } from "@/cli/factory/types"
import { killChildren, labelSession, orchestratorTurn, planPrompt, REMOTE_FILES_MAX, resolveOn, selfArgv, trackChild, type TurnResult } from "@/cli/mission/orchestrator"
import {
  advance,
  currentMilestone,
  DEFAULT_MAX_ATTEMPTS,
  DEFAULT_MAX_WORKERS,
  log as logLine,
  overHours,
  parsePlanReply,
  planProblems,
  type Mission,
  type PlanInput,
} from "@/cli/mission/state"
import { logFile, missionsDir, newMissionId, projectRoot, runnerAlive, writeMission } from "@/cli/mission/store"

export { killChildren }

export interface CommandResult {
  /** Exit code; non-zero (or -1 when it could not start / timed out) is a failure. */
  code: number
  /** stdout and stderr together, in order (kept to the last ~64 KB). */
  output: string
}

export interface MissionDeps {
  startSwarm(opts: StartOptions): Promise<{ record: SwarmRecord; plan: StartPlan }>
  /** One look at the engine folded into the record (and queued mailboxes delivered); the record as it is now. */
  observeSwarm(rec: SwarmRecord): Promise<SwarmRecord>
  stopWorker(rec: SwarmRecord, bot: string): Promise<SwarmRecord>
  landSwarm(rec: SwarmRecord, opts: { bots?: string[]; dryRun?: boolean; includeUnfinished?: boolean }): Promise<LandStep[]>
  readSwarm(id: string): SwarmRecord | null
  orchestratorTurn(m: Mission, prompt: string, opts: { engine?: string; cwd?: string }): Promise<TurnResult>
  labelSession(m: Mission): Promise<void>
  /** Run a shell command (`sh -c`) in `cwd`. */
  exec(cmd: string, cwd: string): Promise<CommandResult>
  git(args: string[], cwd: string): CommandResult
  now(): number
}

const OUTPUT_KEEP = 64_000
const VALIDATE_TIMEOUT_MS = 60 * 60_000

export function execShell(cmd: string, cwd: string, timeoutMs = VALIDATE_TIMEOUT_MS): Promise<CommandResult> {
  return new Promise((resolve) => {
    let output = ""
    const add = (d: Buffer | string) => {
      output += d
      if (output.length > OUTPUT_KEEP * 2) output = output.slice(-OUTPUT_KEEP)
    }
    let child: ReturnType<typeof spawn>
    try {
      child = spawn("/bin/sh", ["-c", cmd], { cwd, stdio: ["ignore", "pipe", "pipe"], env: process.env })
    } catch (err) {
      resolve({ code: -1, output: `couldn't run ${cmd}: ${(err as Error).message}` })
      return
    }
    trackChild(child)
    const timer = setTimeout(() => {
      add(`\n[timed out after ${Math.round(timeoutMs / 60_000)} min]\n`)
      child.kill("SIGTERM")
    }, timeoutMs)
    child.stdout?.on("data", add)
    child.stderr?.on("data", add)
    child.on("error", (err) => add(`\n${err.message}\n`))
    child.on("close", (code) => {
      clearTimeout(timer)
      resolve({ code: code ?? -1, output: output.slice(-OUTPUT_KEEP) })
    })
  })
}

export function realDeps(): MissionDeps {
  return {
    startSwarm,
    async observeSwarm(rec) {
      let agents: FactoryAgent[]
      try {
        const { data } = await runEngine<{ agents?: FactoryAgent[] }>(["agents", "ps"], { timeoutMs: 10000 })
        agents = data?.agents ?? []
      } catch {
        // The engine is restarting or busy: look again on the next poll.
        return readSwarm(rec.id) ?? rec
      }
      const next = observeSwarm(readSwarm(rec.id) ?? rec, agents)
      drainReady(next, agents)
      return next
    },
    stopWorker,
    landSwarm,
    readSwarm,
    orchestratorTurn: (m, prompt, opts) => orchestratorTurn(m, prompt, opts),
    labelSession,
    exec: (cmd, cwd) => execShell(cmd, cwd),
    git(args, cwd) {
      const r = spawnSync("git", args, { cwd, encoding: "utf8" })
      return { code: r.status ?? -1, output: `${r.stdout ?? ""}${r.stderr ?? ""}`.trim() }
    },
    now: () => Date.now(),
  }
}

type Git = MissionDeps["git"]

// ---------------------------------------------------------------- record

/**
 * Where mission checkouts live: outside every project, so a project's test
 * runners and file watchers never pick them up (`ALLTERNIT_MISSION_CHECKOUTS`
 * overrides it). Swarm workers of a mission get sibling trees here too: a
 * swarm puts each worker at `<the tree it started from>-<swarm>-<worker>`.
 */
export function missionCheckoutsDir(env: NodeJS.ProcessEnv = process.env): string {
  return env.ALLTERNIT_MISSION_CHECKOUTS?.trim() || path.join(os.homedir(), ".allternit", "mission-checkouts")
}

/**
 * A mission's own checkout: `<checkouts>/<repo-name>-<hash of the repo path>/<id>`
 * (the hash keeps two repos with the same folder name apart). The record and
 * log stay at `<root>/.allternit/missions/`.
 */
export function missionWorktree(root: string, id: string, env: NodeJS.ProcessEnv = process.env): string {
  let real = root
  try {
    real = fs.realpathSync(root)
  } catch {
    // not there (yet): hash the path as given
  }
  const hash = crypto.createHash("sha256").update(real).digest("hex").slice(0, 8)
  return path.join(missionCheckoutsDir(env), `${path.basename(real) || "repo"}-${hash}`, id)
}

export interface NewMissionOptions {
  goal: string
  /** Any directory of the project; the record goes to its repo root. */
  cwd: string
  brain: string
  maxWorkers?: number
  maxAttempts?: number
  maxHours?: number
  /** `--on <runtime>`: plan and fix turns run on that paired computer (see orchestrator.ts). */
  on?: string
  now?: number
  git?: Git
}

/** A new mission record (status `planning`, not written yet), or why it can't start here. */
export function newMission(o: NewMissionOptions): { mission: Mission } | { error: string } {
  const git = o.git ?? realDeps().git
  const goal = o.goal.trim()
  if (!goal) return { error: 'Give the mission a goal: gizzi mission start "<goal>"' }
  if (git(["rev-parse", "--show-toplevel"], o.cwd).code !== 0) return { error: `${o.cwd} isn't in a git repository; a mission works on a branch of one` }
  const root = projectRoot(o.cwd)
  const head = git(["rev-parse", "--abbrev-ref", "HEAD"], root)
  const baseBranch = head.output.trim()
  if (head.code !== 0 || !baseBranch || baseBranch === "HEAD") return { error: `${root} isn't on a branch (detached HEAD or no commits yet); check out the branch the mission should land on` }
  const now = o.now ?? Date.now()
  const id = newMissionId(now)
  return {
    mission: {
      id,
      goal,
      root,
      baseBranch,
      branch: `mission/${id}`,
      worktree: missionWorktree(root, id),
      brain: o.brain,
      maxWorkers: Math.max(1, Math.floor(o.maxWorkers ?? DEFAULT_MAX_WORKERS)),
      maxAttempts: Math.max(0, Math.floor(o.maxAttempts ?? DEFAULT_MAX_ATTEMPTS)),
      ...(o.maxHours ? { maxHours: o.maxHours } : {}),
      ...(o.on ? { on: { runtime: o.on } } : {}),
      status: "planning",
      milestones: [],
      current: 0,
      createdAt: now,
      log: [{ at: now, text: "created" }],
    },
  }
}

// ---------------------------------------------------------------- plan / approve

export interface PlanOutcome {
  /** The record as written (`awaiting_approval` when the plan is fine). */
  mission: Mission
  /** The parsed plan, also when it has problems (so the person can edit it). */
  plan?: PlanInput
  problems: string[]
  /** The turn couldn't run or replied with no plan at all. */
  error?: string
}

/**
 * The plan turn: asks the orchestrator for milestones and records them.
 * `retry` is the previous plan's problems, handed back on a second try.
 * Makes one model call; nothing is spawned.
 */
export async function planMission(m: Mission, opts: { engine?: string; retry?: string[]; deps?: MissionDeps } = {}): Promise<PlanOutcome> {
  // --on: the runtime must be paired, reachable and the relay usable, or nothing starts (never a local fallback).
  const on = await resolveOn(m)
  if ("error" in on) return { mission: m, problems: [], error: on.error }
  const deps = opts.deps ?? realDeps()
  let next: Mission = { ...m, ...(on.on ? { on: on.on } : { engineURL: opts.engine ?? m.engineURL }) }
  writeMission(next)
  // The orchestrator on another computer can't open the repo: it gets the tracked paths instead.
  const files = next.on ? trackedFiles(m.root, deps.git) : undefined
  const prompt = [planPrompt(m.goal, m.root, m.maxWorkers, files), ...(opts.retry?.length ? ["", "Your last plan couldn't run:", ...opts.retry.map((p) => `- ${p}`), "Fix those and reply with the whole plan again."] : [])].join("\n")
  const turn = await deps.orchestratorTurn(next, prompt, { engine: next.on ? undefined : opts.engine, cwd: m.root })
  next = { ...next, orchestratorSession: turn.sessionID ?? next.orchestratorSession, ...(turn.on ? { on: turn.on } : {}) }
  if (turn.error) {
    writeMission(next)
    return { mission: next, problems: [], error: `the plan turn failed: ${turn.error}` }
  }
  const plan = parsePlanReply(turn.text)
  if (!plan) {
    writeMission(next)
    return { mission: next, problems: [`the orchestrator's reply had no plan JSON: ${turn.text.slice(0, 200)}`] }
  }
  const problems = planProblems(plan)
  if (!problems.length) next = advance(next, { type: "planned", plan, session: turn.sessionID }, deps.now())
  writeMission(next)
  return { mission: next, plan, problems }
}

function trackedFiles(root: string, git: Git): string[] {
  const r = git(["ls-files"], root)
  return r.code === 0 ? r.output.split("\n").filter(Boolean).slice(0, REMOTE_FILES_MAX) : []
}

export interface SpawnOutcome {
  mission: Mission
  /** pid of the detached runner (the runner records its own `runnerPid`). */
  pid?: number
  log: string
  error?: string
}

/**
 * The person approved: record it (with the edited plan, when there is one),
 * make the mission's checkout and start the detached runner. Never prompts.
 */
export function approveAndSpawn(m: Mission, opts: { plan?: PlanInput; deps?: MissionDeps; spawn?: (m: Mission) => number | undefined } = {}): SpawnOutcome {
  const deps = opts.deps ?? realDeps()
  const problems = opts.plan ? planProblems(opts.plan) : m.milestones.length ? [] : ["the mission has no plan yet"]
  if (problems.length) return { mission: m, log: logFile(m), error: `the plan can't run: ${problems.join("; ")}` }
  const next = advance(m, { type: "approved", plan: opts.plan }, deps.now())
  if (next.status !== "running") return { mission: m, log: logFile(m), error: `mission ${m.id} is ${m.status}, not waiting for approval` }
  return startRunner(next, deps, opts.spawn)
}

/** Make the checkout, write the record and spawn a runner unless one is alive. */
export function startRunner(m: Mission, deps: MissionDeps = realDeps(), spawnFn: (m: Mission) => number | undefined = spawnRunner): SpawnOutcome {
  const wt = ensureWorktree(m, deps.git)
  if ("error" in wt) {
    const failed = advance(m, { type: "error", text: wt.error }, deps.now())
    writeMission(failed)
    return { mission: failed, log: logFile(m), error: wt.error }
  }
  writeMission(m)
  if (runnerAlive(m)) return { mission: m, pid: m.runnerPid, log: logFile(m) }
  return { mission: m, pid: spawnFn(m), log: logFile(m) }
}

/** `gizzi mission run <id>` in the background, its output appended to the mission's log. */
export function spawnRunner(m: Mission): number | undefined {
  const [cmd, ...pre] = selfArgv()
  fs.mkdirSync(missionsDir(m.root), { recursive: true })
  const fd = fs.openSync(logFile(m), "a")
  try {
    const child = spawn(cmd!, [...pre, "mission", "run", m.id], { cwd: m.root, detached: true, stdio: ["ignore", fd, fd], env: process.env })
    child.unref()
    return child.pid
  } finally {
    fs.closeSync(fd)
  }
}

// ---------------------------------------------------------------- resume / stop

export type ResumeOutcome =
  /** Spawn a runner for it (status `running`). */
  | { kind: "run"; mission: Mission }
  /** Never approved: show the plan and ask again. */
  | { kind: "approve"; mission: Mission }
  | { kind: "error"; text: string }

/**
 * What `gizzi mission resume` does for `m`. `maxHours` (from --max-hours)
 * replaces the limit; a mission over its hours only goes on with a larger one.
 */
export function prepareResume(m: Mission, opts: { maxHours?: number; now?: number } = {}): ResumeOutcome {
  const now = opts.now ?? Date.now()
  if (opts.maxHours !== undefined && !(opts.maxHours > 0)) return { kind: "error", text: "--max-hours must be a positive number of hours" }
  let next: Mission = opts.maxHours !== undefined ? { ...m, maxHours: opts.maxHours } : m
  switch (m.status) {
    case "done":
      return { kind: "error", text: m.landed === false ? `Mission ${m.id} is done but not landed (${m.landNote ?? "see status"}): gizzi mission land ${m.id}` : `Mission ${m.id} is already done` }
    case "failed":
      return { kind: "error", text: `Mission ${m.id} failed${m.note ? `: ${m.note}` : ""}; start a new one` }
    case "planning":
    case "awaiting_approval":
      return { kind: "approve", mission: next }
    case "running":
      if (runnerAlive(m)) return { kind: "error", text: `Mission ${m.id} is already running (runner pid ${m.runnerPid})` }
      break // the runner died: start another
    case "needs_you":
    case "stopped":
      next = advance(next, { type: "resume" }, now)
      if (next.status !== "running") return { kind: "approve", mission: next }
      break
  }
  if (overHours(next, now)) {
    const used = ((now - (next.startedAt ?? now)) / 3600_000).toFixed(1)
    return { kind: "error", text: `Mission ${m.id} has run ${used} h, over its --max-hours ${next.maxHours}. To go on, give it more: gizzi mission resume ${m.id} --max-hours <hours>` }
  }
  return { kind: "run", mission: { ...next, runnerPid: undefined } }
}

/**
 * Stop a mission: SIGTERM its runner when one is alive (the runner stops the
 * workers and records `stopped`); otherwise stop the open swarm's workers
 * here and record it.
 */
export async function stopMission(m: Mission, deps: MissionDeps = realDeps()): Promise<{ mission: Mission; signalled: boolean; errors: string[] }> {
  if (runnerAlive(m)) {
    process.kill(m.runnerPid!, "SIGTERM")
    return { mission: m, signalled: true, errors: [] }
  }
  const errors: string[] = []
  const ms = currentMilestone(m)
  const open = ms?.status === "working" ? ms.swarms[ms.swarms.length - 1] : undefined
  let rec = open ? deps.readSwarm(open) : null
  for (const w of rec?.workers ?? []) {
    if (isSettled(w)) continue
    try {
      rec = await deps.stopWorker(rec!, w.bot)
    } catch (err) {
      errors.push(`${w.address}: ${(err as Error)?.message ?? String(err)}`)
    }
  }
  const next = advance({ ...m, runnerPid: undefined }, { type: "stop", reason: "stopped by you" }, deps.now())
  writeMission(next)
  return { mission: next, signalled: false, errors }
}

// ---------------------------------------------------------------- git

const EXCLUDE = ".allternit/missions/"

/** Keep the mission records (and older in-project checkouts) out of the person's `git status` (local exclude, not .gitignore). */
export function excludeMissionDirs(root: string, git: Git): void {
  if (git(["check-ignore", "-q", ".allternit/missions/x.json"], root).code === 0) return
  const p = git(["rev-parse", "--path-format=absolute", "--git-path", "info/exclude"], root)
  const file = p.code === 0 ? p.output.trim().split("\n").pop() : undefined
  if (!file || !path.isAbsolute(file)) return
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true })
    const cur = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : ""
    if (cur.split("\n").some((l) => l.trim() === EXCLUDE)) return
    fs.appendFileSync(file, `${cur && !cur.endsWith("\n") ? "\n" : ""}${EXCLUDE}\n`)
  } catch {
    // best effort: the checkout still works, it just shows in `git status`
  }
}

function samePath(a: string, b: string): boolean {
  try {
    return fs.realpathSync(a) === fs.realpathSync(b)
  } catch {
    return path.resolve(a) === path.resolve(b)
  }
}

/**
 * Make sure the mission's own checkout exists: a git worktree of `branch`
 * (made from `baseBranch` the first time) at `m.worktree`. The person's
 * checkout at `root` is never switched. A record whose worktree is `root`
 * (old records, tests) works in root as before.
 */
export function ensureWorktree(m: Mission, git: Git, exists: (p: string) => boolean = fs.existsSync): { ok: true; created: boolean } | { ok: false; error: string } {
  const dir = m.worktree
  if (!dir || dir === m.root) return { ok: true, created: false }
  if (exists(dir)) {
    // A folder that isn't its own checkout would make git act on root's: refuse.
    const top = git(["rev-parse", "--show-toplevel"], dir)
    if (top.code !== 0 || !samePath(top.output.trim(), dir)) return { ok: false, error: `${dir} exists but isn't the mission's git checkout; move it away, then resume` }
    return { ok: true, created: false }
  }
  excludeMissionDirs(m.root, git)
  const branch = m.branch || `mission/${m.id}`
  const has = git(["rev-parse", "--verify", "--quiet", `refs/heads/${branch}`], m.root).code === 0
  git(["worktree", "prune"], m.root)
  try {
    fs.mkdirSync(path.dirname(dir), { recursive: true })
  } catch {
    // git says why below
  }
  const r = git(has ? ["worktree", "add", dir, branch] : ["worktree", "add", "-b", branch, dir, m.baseBranch], m.root)
  if (r.code !== 0) return { ok: false, error: `couldn't make the mission checkout at ${dir} (${branch}${has ? "" : ` from ${m.baseBranch}`}): ${r.output}` }
  return { ok: true, created: true }
}

export interface LandResult {
  landed: boolean
  note: string
}

/**
 * Land a done mission: merge `branch` into `baseBranch` in the person's
 * checkout. Only when that checkout is on `baseBranch` with no uncommitted
 * changes; a fast-forward by default, a merge commit with `merge`. Never
 * forces; a failed merge is aborted.
 */
export function landMission(m: Mission, git: Git, opts: { merge?: boolean } = {}): LandResult {
  const hint = `gizzi mission land ${m.id}`
  const head = git(["rev-parse", "--abbrev-ref", "HEAD"], m.root)
  const on = head.output.trim()
  if (head.code !== 0) return { landed: false, note: `couldn't read the branch of ${m.root}: ${head.output}` }
  if (on !== m.baseBranch) return { landed: false, note: `${m.root} is on ${on}, not ${m.baseBranch}; switch it to ${m.baseBranch}, then ${hint}` }
  const dirty = git(["status", "--porcelain", "--untracked-files=no"], m.root)
  if (dirty.code !== 0 || dirty.output.trim()) return { landed: false, note: `${m.root} has uncommitted changes; commit or stash them, then ${hint}` }
  const ahead = git(["rev-list", "--count", `${m.baseBranch}..${m.branch}`], m.root)
  if (ahead.code === 0 && ahead.output.trim() === "0") return { landed: true, note: `${m.baseBranch} already has everything on ${m.branch}` }
  if (!opts.merge) {
    const ff = git(["merge", "--ff-only", m.branch], m.root)
    if (ff.code === 0) return { landed: true, note: `fast-forwarded ${m.baseBranch} to ${m.branch}` }
    const tail = ff.output.split("\n").slice(-2).join(" ")
    const moved = /fast-forward/i.test(ff.output)
    return { landed: false, note: moved ? `${m.baseBranch} moved since the mission started, so it isn't a fast-forward; ${hint} --merge makes a merge commit` : `the fast-forward failed (${tail}); ${hint} again once that's fixed` }
  }
  const mg = git(["merge", "--no-ff", "--no-edit", "-m", `Land mission ${m.id}: ${m.goal.split("\n")[0]!.slice(0, 60)}`, m.branch], m.root)
  if (mg.code === 0) return { landed: true, note: `merged ${m.branch} into ${m.baseBranch}` }
  git(["merge", "--abort"], m.root)
  return { landed: false, note: `merging ${m.branch} into ${m.baseBranch} failed and was undone (${mg.output.split("\n").slice(-2).join(" ")}); merge it by hand` }
}

/** After landing: drop the mission's checkout (the branch stays). Best effort. */
export function removeWorktree(m: Mission, git: Git): void {
  if (!m.worktree || m.worktree === m.root) return
  if (git(["worktree", "remove", m.worktree], m.root).code !== 0) {
    // git refuses a checkout with untracked files, and the Factory engine
    // always leaves its own state (`.allternit/`: ledger, mail, teams) in the
    // one the workers ran from. Force only when that is all that's left;
    // anything else there is somebody's work and the checkout stays.
    const left = git(["status", "--porcelain", "--untracked-files=all"], m.worktree)
    if (left.code !== 0) return
    const lines = left.output.split("\n").filter((l) => l.trim())
    if (!lines.length || lines.some((l) => !/^\?\? "?\.allternit\//.test(l))) return
    if (git(["worktree", "remove", "--force", m.worktree], m.root).code !== 0) return
  }
  git(["worktree", "prune"], m.root)
  // The repo's folder under the checkouts dir goes once its last mission checkout does.
  try {
    fs.rmdirSync(path.dirname(m.worktree))
  } catch {
    // not empty (another mission of this repo), or already gone
  }
}

/** Try landing and record the outcome on the mission (not written). */
export function landAndRecord(m: Mission, git: Git, opts: { merge?: boolean; now?: number } = {}): Mission {
  const r = landMission(m, git, opts)
  if (r.landed) removeWorktree(m, git)
  const text = r.landed ? `landed: ${r.note}` : `ready to land: gizzi mission land ${m.id} (${r.note})`
  return logLine({ ...m, landed: r.landed, landNote: r.note }, text, opts.now ?? Date.now())
}
