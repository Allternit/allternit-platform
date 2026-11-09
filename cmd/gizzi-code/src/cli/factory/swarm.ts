/**
 * Swarms: one task handed to N Terminal bots at once, each in its own git
 * worktree, watched on one board and landed together.
 *
 * A swarm is a thin layer over the engine, not a second one:
 *   start  `git worktree add` per worker, a team file (`.allternit/teams/<id>`),
 *          `agents up <id> --workdir <tree>` per worker, then
 *          `orchestration send <w@id> "<task>"` once its pane is ready
 *   watch  the engine's event stream (see live.ts)
 *   stop   `agents down <slug>`
 *   land   commit leftovers on each worker branch, merge it into the branch
 *          the swarm started from, remove the worktree
 *
 * The only state Gizzi keeps is the swarm record
 * (`$ALLTERNIT_FACTORY_HOME/swarms/<id>.json`): which bots, trees and
 * branches belong to the swarm, and when each worker finished. The engine
 * reports `idle` again once a harness finishes its turn, so "done" has to be
 * remembered here to stay on the board until the swarm is landed.
 */
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { FactoryEngineError, runEngine } from "@/cli/factory/engine"
import type { FactoryAgent } from "@/cli/factory/types"

export type WorkerPhase = "starting" | "working" | "waiting" | "done" | "failed" | "stopped" | "landed"

export interface SwarmWorker {
  bot: string
  address: string
  /** The engine's agent id (`local:<bot>-<team>`). */
  agentId: string
  /** The pane slug `agents down` takes. */
  slug: string
  branch: string
  workdir: string
  /** This worker's own task (`--task` given once per worker); else the swarm's. */
  task?: string
  startedAt: number
  taskSentAt?: number
  sawWorking?: boolean
  /** The task went to the bot's mailbox, not its pane: finished only once seen working. */
  taskQueued?: boolean
  /** Messages (the task, or a steer) waiting in its mailbox because the pane was busy. */
  pending?: number
  finishedAt?: number
  phase: WorkerPhase
  note?: string
}

export interface SwarmRecord {
  id: string
  task: string
  brain: string
  harness: string
  model: string | null
  root: string
  baseBranch: string
  createdAt: number
  workers: SwarmWorker[]
}

/** CLI brains (the person's own subscriptions) → engine harness names. */
const BRAIN_HARNESS: Record<string, string> = {
  "claude-cli": "claude",
  claude: "claude",
  "codex-cli": "codex",
  codex: "codex",
  "kimi-cli": "kimi",
  kimi: "kimi",
  "grok-cli": "grok",
  grok: "grok",
  agy: "agy",
  gizzi: "gizzi",
}

/** `claude-cli/claude-sonnet-5` → { harness: claude, model: claude-sonnet-5 }. */
export function parseBrain(brain: string): { harness: string; model: string | null } {
  const [provider, ...rest] = brain.trim().split("/")
  const harness = BRAIN_HARNESS[(provider ?? "").toLowerCase()]
  if (!harness) {
    throw new FactoryEngineError({
      code: "usage",
      fact: `${brain} isn't a brain a Factory pane can run`,
      action: `Use a CLI brain: ${Object.keys(BRAIN_HARNESS).filter((k) => k.includes("-cli")).map((k) => `${k}/<model>`).join(", ")}.`,
    })
  }
  const model = rest.join("/").trim()
  return { harness, model: model || null }
}

/** Every Factory pane runs a CLI harness on the person's own plan: no metered cost to show. */
export function costLabel(harness: string | undefined): string {
  return harness ? "plan" : "—"
}

export function factoryHome(env: NodeJS.ProcessEnv = process.env): string {
  return env.ALLTERNIT_FACTORY_HOME || path.join(os.homedir(), ".allternit", "factory")
}

export function swarmsDir(env?: NodeJS.ProcessEnv): string {
  return path.join(factoryHome(env), "swarms")
}

export function newSwarmId(now = Date.now()): string {
  return `sw-${now.toString(36).slice(-5)}${Math.floor(Math.random() * 36).toString(36)}`
}

export function readSwarm(id: string): SwarmRecord | null {
  try {
    return JSON.parse(fs.readFileSync(path.join(swarmsDir(), `${id}.json`), "utf8")) as SwarmRecord
  } catch {
    return null
  }
}

export function writeSwarm(rec: SwarmRecord): void {
  fs.mkdirSync(swarmsDir(), { recursive: true })
  const file = path.join(swarmsDir(), `${rec.id}.json`)
  const tmp = `${file}.${process.pid}.tmp`
  fs.writeFileSync(tmp, JSON.stringify(rec, null, 2))
  fs.renameSync(tmp, file)
}

/** Every swarm record, newest first. */
export function listSwarms(): SwarmRecord[] {
  let names: string[] = []
  try {
    names = fs.readdirSync(swarmsDir()).filter((n) => n.endsWith(".json"))
  } catch {
    return []
  }
  return names
    .map((n) => readSwarm(n.replace(/\.json$/, "")))
    .filter((r): r is SwarmRecord => !!r)
    .sort((a, b) => b.createdAt - a.createdAt)
}

/** Swarms with at least one worker not landed or stopped. */
export function openSwarms(): SwarmRecord[] {
  return listSwarms().filter((s) => s.workers.some((w) => w.phase !== "landed" && w.phase !== "stopped"))
}

/** The swarm named, or the newest open one. */
export function resolveSwarm(id?: string): SwarmRecord {
  const rec = id ? readSwarm(id) : (openSwarms()[0] ?? null)
  if (!rec) {
    throw new FactoryEngineError({
      code: "not_found",
      fact: id ? `No swarm ${id} on this computer` : "No swarm is running on this computer",
      action: 'Start one with `gizzi swarm "<task>" --workers 2 --brain claude-cli/<model>`.',
    })
  }
  return rec
}

/** How long a worker can sit idle after its task landed before idle means "finished". */
export const IDLE_AFTER_SEND_MS = 15000

/**
 * Fold one engine observation into a worker's phase. Pure: returns the
 * updated worker (the same object when nothing changed).
 *
 * done stays done (until landed): the engine flips back to idle once a
 * harness has finished, and the person still needs to see it finished.
 */
export function observeWorker(w: SwarmWorker, agent: FactoryAgent | undefined, now = Date.now()): SwarmWorker {
  if (w.phase === "landed" || w.phase === "stopped" || w.phase === "failed") return w
  const state = agent?.state ?? "offline"
  // A finished worker that starts again (steered, or it picked up a queued
  // message) is working again; it is done again when it goes idle.
  // Once it has picked the task up, the "queued" note is history.
  const note = w.taskQueued ? undefined : w.note
  if (state === "working") return w.phase === "working" && w.sawWorking ? w : { ...w, phase: "working", sawWorking: true, finishedAt: undefined, note }
  if (state === "needs_you" || state === "blocked") return w.phase === "waiting" ? w : { ...w, phase: "waiting", sawWorking: true, finishedAt: undefined, note }
  if (w.phase === "done") return w
  if (state === "failed") return { ...w, phase: "failed", finishedAt: now, note: "the engine reported the pane failed" }
  if (state === "offline") {
    if (!w.taskSentAt && now - w.startedAt < 60000) return w
    return { ...w, phase: "failed", finishedAt: now, note: "its pane is gone" }
  }
  if (!w.taskSentAt) return w
  if (state === "done" || (state === "idle" && (w.sawWorking || (!w.taskQueued && now - w.taskSentAt > IDLE_AFTER_SEND_MS)))) {
    return { ...w, phase: "done", finishedAt: now }
  }
  return w
}

/** Fold a snapshot into a record; writes it back when anything changed. */
export function observeSwarm(rec: SwarmRecord, agents: FactoryAgent[], now = Date.now()): SwarmRecord {
  const byId = new Map(agents.map((a) => [a.id, a]))
  let changed = false
  const workers = rec.workers.map((w) => {
    const next = observeWorker(w, byId.get(w.agentId) ?? agents.find((a) => a.address === w.address), now)
    if (next !== w) changed = true
    return next
  })
  if (!changed) return rec
  const fresh = readSwarm(rec.id) ?? rec
  // Keep phases another process (a land, a stop) moved on since we read it.
  const merged = {
    ...fresh,
    workers: workers.map((w) => {
      const theirs = fresh.workers.find((f) => f.bot === w.bot)
      if (!theirs) return w
      if (theirs.phase === "landed" || theirs.phase === "stopped") return theirs
      return { ...w, pending: theirs.pending }
    }),
  }
  writeSwarm(merged)
  return merged
}

export function isSettled(w: SwarmWorker): boolean {
  if (w.phase === "done") return !w.pending
  return w.phase === "failed" || w.phase === "stopped" || w.phase === "landed"
}

function patchWorker(id: string, bot: string, patch: Partial<SwarmWorker>): SwarmRecord | null {
  const fresh = readSwarm(id)
  if (!fresh) return null
  const next = { ...fresh, workers: fresh.workers.map((w) => (w.bot === bot ? { ...w, ...patch } : w)) }
  writeSwarm(next)
  return next
}

const draining = new Set<string>()

/**
 * Deliver what waits in a worker's mailbox (a queued task or steer) once its
 * pane is free: the engine queues a send to a busy pane rather than typing
 * over the bot, and `orchestration drain` is how it gets delivered. Called
 * by the board and `swarm wait` on every engine update.
 */
export function drainReady(rec: SwarmRecord, agents: FactoryAgent[]): void {
  for (const w of rec.workers) {
    if (!w.pending || draining.has(w.address)) continue
    const a = agents.find((x) => x.id === w.agentId || x.address === w.address)
    if (!a || (a.state !== "idle" && a.state !== "done")) continue
    draining.add(w.address)
    runEngine(["orchestration", "drain", w.address, "--all", "--root", rec.root], { timeoutMs: 30000 })
      .then(({ data }) => {
        if (!JSON.stringify(data ?? {}).includes('"delivered":false')) patchWorker(rec.id, w.bot, { pending: 0 })
      })
      .catch(() => {
        // Still busy (the paste couldn't be verified): try again on the next update.
      })
      .finally(() => draining.delete(w.address))
  }
}

/** Remember a message the engine queued for a swarm worker, so drainReady delivers it. */
export function notePending(rec: SwarmRecord, bot: string): void {
  const w = rec.workers.find((x) => x.bot === bot)
  patchWorker(rec.id, bot, { pending: (w?.pending ?? 0) + 1 })
}

// ---------------------------------------------------------------- git

function git(cwd: string, args: string[]): { ok: boolean; out: string; err: string } {
  const r = spawnSync("git", args, { cwd, encoding: "utf8" })
  return { ok: r.status === 0, out: (r.stdout ?? "").trim(), err: (r.stderr ?? "").trim() }
}

export function repoRoot(cwd: string): string {
  const r = git(cwd, ["rev-parse", "--show-toplevel"])
  if (!r.ok) {
    throw new FactoryEngineError({
      code: "usage",
      fact: `${cwd} isn't inside a git repository`,
      action: "Run `gizzi swarm` from the repo the workers should change (each worker gets its own worktree of it).",
    })
  }
  return r.out
}

// ---------------------------------------------------------------- start

export interface StartOptions {
  task: string
  /** One task per worker (overrides `task` + `workers`). */
  tasks?: string[]
  workers: number
  brain: string
  cwd: string
  /** Progress lines for the person (not printed with --json). */
  log?: (line: string) => void
  /** Max wait per worker for its pane to be ready before sending the task. */
  readyTimeoutMs?: number
}

export interface StartPlan {
  id: string
  root: string
  baseBranch: string
  harness: string
  model: string | null
  workers: { bot: string; address: string; branch: string; workdir: string; task?: string }[]
}

export function planSwarm(opts: StartOptions, id = newSwarmId()): StartPlan {
  const tasks = (opts.tasks ?? []).map((t) => t.trim()).filter(Boolean)
  if (tasks.length) opts = { ...opts, workers: tasks.length, task: opts.task.trim() || tasks.join(" | ") }
  if (!opts.task.trim()) throw new FactoryEngineError({ code: "usage", fact: "A swarm needs a task", action: 'gizzi swarm "<task>" --workers 2' })
  if (!Number.isInteger(opts.workers) || opts.workers < 1 || opts.workers > 12) {
    throw new FactoryEngineError({ code: "usage", fact: `--workers must be 1–12 (got ${opts.workers})` })
  }
  const { harness, model } = parseBrain(opts.brain)
  const root = repoRoot(opts.cwd)
  const head = git(root, ["rev-parse", "--abbrev-ref", "HEAD"])
  const baseBranch = head.ok && head.out !== "HEAD" ? head.out : git(root, ["rev-parse", "HEAD"]).out
  const workers = Array.from({ length: opts.workers }, (_, i) => {
    const bot = `w${i + 1}`
    return {
      bot,
      address: `${bot}@${id}`,
      branch: `swarm/${id}/${bot}`,
      workdir: `${root}-${id}-${bot}`,
      task: tasks[i],
    }
  })
  return { id, root, baseBranch, harness, model, workers }
}

function teamYaml(id: string, harness: string, model: string | null, bots: string[]): string {
  const lines = [`name: ${id}`, "bots:"]
  for (const b of bots) {
    lines.push(`  - { bot: ${b}, role: build, binding: terminal, harness: ${harness}${model ? `, model: ${JSON.stringify(model)}` : ""} }`)
  }
  return lines.join("\n") + "\n"
}

/** The one line each worker receives. Long or multi-line tasks go in a file it reads. */
export function taskMessage(rec: Pick<SwarmRecord, "id" | "task" | "workers">, w: Pick<SwarmWorker, "bot" | "branch" | "task">, taskFile: string | null): string {
  const n = rec.workers.length
  const role = `You are worker ${w.bot} of ${n} in swarm ${rec.id}, in your own git worktree on branch ${w.branch}. Commit your work on this branch when you finish.`
  const task = (w.task ?? rec.task).trim()
  return taskFile ? `Read ${taskFile} and do the task in it. ${role}` : `${task} (${role})`
}

const SINGLE_LINE_MAX = 600

async function waitReady(agentId: string, address: string, timeoutMs: number): Promise<string> {
  const deadline = Date.now() + timeoutMs
  let last = "offline"
  while (Date.now() < deadline) {
    const { data } = await runEngine<{ agents?: FactoryAgent[] }>(["agents", "ps"], { timeoutMs: 10000 })
    const a = data?.agents?.find((x) => x.id === agentId || x.address === address)
    last = a?.state ?? "offline"
    if (last === "idle" || last === "done") return last
    if (last === "needs_you") return last
    await new Promise((r) => setTimeout(r, 1500))
  }
  return last
}

/**
 * Start a swarm: trees, team, panes, task. Each worker is reported as it
 * starts; a worker that fails to start is recorded as failed with the
 * engine's own reason, and the rest still start.
 */
export async function startSwarm(opts: StartOptions): Promise<{ record: SwarmRecord; plan: StartPlan }> {
  const log = opts.log ?? (() => {})
  const plan = planSwarm(opts)
  if (opts.tasks?.length && !opts.task.trim()) opts = { ...opts, task: plan.workers.map((w) => w.task).join(" | ") }

  const record: SwarmRecord = {
    id: plan.id,
    task: opts.task,
    brain: opts.brain,
    harness: plan.harness,
    model: plan.model,
    root: plan.root,
    baseBranch: plan.baseBranch,
    createdAt: Date.now(),
    workers: [],
  }
  const swarmDir = path.join(swarmsDir(), record.id)
  fs.mkdirSync(swarmDir, { recursive: true })
  const taskFileFor = (w: { bot: string; task?: string }): string | null => {
    const text = (w.task ?? opts.task).trim()
    if (!text.includes("\n") && text.length <= SINGLE_LINE_MAX) return null
    const file = path.join(swarmDir, w.task ? `TASK-${w.bot}.md` : "TASK.md")
    fs.writeFileSync(file, text + "\n")
    return file
  }

  const teamDir = path.join(plan.root, ".allternit", "teams", record.id)
  fs.mkdirSync(teamDir, { recursive: true })
  writeSwarm(record)

  const started: string[] = []
  for (const w of plan.workers) {
    const worker: SwarmWorker = {
      bot: w.bot,
      address: w.address,
      agentId: `local:${w.bot}-${record.id}`,
      slug: `${w.bot}-${record.id}`,
      branch: w.branch,
      workdir: w.workdir,
      task: w.task,
      startedAt: Date.now(),
      phase: "starting",
    }
    record.workers.push(worker)
    const wt = git(plan.root, ["worktree", "add", "-b", w.branch, w.workdir, "HEAD"])
    if (!wt.ok) {
      worker.phase = "failed"
      worker.note = `git worktree add failed: ${wt.err || wt.out}`
      log(`✗ ${w.address}  ${worker.note}`)
      writeSwarm(record)
      continue
    }
    // The team file grows one bot per `up`, so each `up` spawns exactly the
    // new bot (the earlier ones are live and planned as skip) in its own tree.
    started.push(w.bot)
    fs.writeFileSync(path.join(teamDir, "team.yaml"), teamYaml(record.id, plan.harness, plan.model, started))
    try {
      const { data } = await runEngine<{ results?: { outcome?: string; fact?: string; step?: { agent?: string; action?: string } }[] }>(
        ["agents", "up", record.id, "--workdir", w.workdir, "--root", plan.root],
        { timeoutMs: 120000 },
      )
      const mine = data?.results?.find((r) => r.step?.agent === w.address && r.step?.action === "spawn")
      if (mine && mine.outcome !== "ok") throw new FactoryEngineError({ code: "transport", fact: mine.fact ?? "the engine didn't start the pane" })
      log(`● ${w.address}  ${plan.harness}${plan.model ? ` · ${plan.model}` : ""}  ${w.workdir}`)
    } catch (err) {
      worker.phase = "failed"
      worker.note = err instanceof FactoryEngineError ? err.fact : (err as Error).message
      log(`✗ ${w.address}  ${worker.note}`)
    }
    writeSwarm(record)
  }

  for (const worker of record.workers) {
    if (worker.phase !== "starting") continue
    try {
      const ready = await waitReady(worker.agentId, worker.address, opts.readyTimeoutMs ?? 90000)
      if (ready === "needs_you") log(`▲ ${worker.address} is asking something before it can start; answer it on the board (enter attaches)`)
      const { data } = await runEngine<{ state?: string; detail?: string | null }>(
        ["orchestration", "send", worker.address, taskMessage(record, worker, taskFileFor(worker)), "--root", plan.root],
        { timeoutMs: 30000 },
      )
      const state = data?.state ?? "sent"
      log(`→ ${worker.address}  task ${state}${data?.detail ? ` (${data.detail})` : ""}`)
      // Only a verified paste starts the idle-means-finished clock; a queued
      // task is done only once the worker has been seen working on it.
      if (state === "queued") worker.note = "task queued until its pane is free; the board delivers it"
      worker.taskSentAt = Date.now()
      worker.taskQueued = state !== "verified"
      if (state === "queued") worker.pending = 1
    } catch (err) {
      worker.phase = "failed"
      worker.note = `couldn't send the task: ${err instanceof FactoryEngineError ? err.fact : (err as Error).message}`
      log(`✗ ${worker.address}  ${worker.note}`)
    }
    writeSwarm(record)
  }
  return { record, plan }
}

// ---------------------------------------------------------------- stop / steer

export async function stopWorker(rec: SwarmRecord, bot: string): Promise<SwarmRecord> {
  const w = rec.workers.find((x) => x.bot === bot)
  if (!w) throw new FactoryEngineError({ code: "not_found", fact: `${bot} isn't a worker of ${rec.id}` })
  await runEngine(["agents", "down", w.slug, "--root", rec.root], { timeoutMs: 30000 })
  const fresh = readSwarm(rec.id) ?? rec
  const next = { ...fresh, workers: fresh.workers.map((x) => (x.bot === bot ? { ...x, phase: "stopped" as const, finishedAt: Date.now() } : x)) }
  writeSwarm(next)
  return next
}

/** `x` on the board for a bot that isn't a swarm worker: stop its pane. */
export async function stopAgent(agent: FactoryAgent): Promise<void> {
  await runEngine(["agents", "down", agent.slug ?? agent.id], { timeoutMs: 30000 })
}

export async function steer(address: string, text: string, root?: string): Promise<{ state: string; detail?: string | null }> {
  const { data } = await runEngine<{ state?: string; detail?: string | null }>(
    ["orchestration", "send", address, text, ...(root ? ["--root", root] : [])],
    { timeoutMs: 30000 },
  )
  return { state: data?.state ?? "sent", detail: data?.detail }
}

// ---------------------------------------------------------------- land

export interface LandStep {
  bot: string
  branch: string
  action: "merge" | "skip"
  reason: string
  outcome?: "merged" | "nothing" | "conflict" | "failed" | "skipped"
  detail?: string
}

/**
 * Land finished workers: commit what each left uncommitted on its branch,
 * merge the branch into the branch the swarm started from (in the main
 * checkout, which must be clean and on that branch), stop its pane, remove
 * its worktree and branch. A conflict aborts that merge and keeps the tree.
 */
export async function landSwarm(
  rec: SwarmRecord,
  opts: { bots?: string[]; dryRun?: boolean; includeUnfinished?: boolean } = {},
): Promise<LandStep[]> {
  const pick = rec.workers.filter((w) => !opts.bots?.length || opts.bots.includes(w.bot))
  const steps: LandStep[] = pick.map((w) => {
    if (w.phase === "landed") return { bot: w.bot, branch: w.branch, action: "skip", reason: "already landed" }
    if (w.phase === "failed" && !fs.existsSync(w.workdir)) return { bot: w.bot, branch: w.branch, action: "skip", reason: "it never started" }
    if (!opts.includeUnfinished && (w.phase === "working" || w.phase === "waiting" || w.phase === "starting")) {
      return { bot: w.bot, branch: w.branch, action: "skip", reason: `still ${w.phase} (wait for it, or pass --all)` }
    }
    return { bot: w.bot, branch: w.branch, action: "merge", reason: `merge ${w.branch} into ${rec.baseBranch}` }
  })
  if (opts.dryRun) return steps

  const head = git(rec.root, ["rev-parse", "--abbrev-ref", "HEAD"])
  if (steps.some((s) => s.action === "merge")) {
    if (head.out !== rec.baseBranch) {
      throw new FactoryEngineError({
        code: "refused",
        fact: `${rec.root} is on ${head.out}, not ${rec.baseBranch} (the branch swarm ${rec.id} started from)`,
        action: `git -C ${rec.root} switch ${rec.baseBranch}, then land again.`,
      })
    }
    const dirty = git(rec.root, ["status", "--porcelain", "--untracked-files=no"])
    if (dirty.out) {
      throw new FactoryEngineError({
        code: "refused",
        fact: `${rec.root} has uncommitted changes, so merging the workers there could mix them in`,
        action: "Commit or stash them, then land again.",
      })
    }
  }

  for (const s of steps) {
    if (s.action !== "merge") {
      s.outcome = "skipped"
      continue
    }
    const w = rec.workers.find((x) => x.bot === s.bot)!
    if (fs.existsSync(w.workdir)) {
      git(w.workdir, ["add", "-A"])
      const staged = git(w.workdir, ["diff", "--cached", "--quiet"])
      if (!staged.ok) git(w.workdir, ["commit", "-q", "-m", `swarm ${rec.id} ${w.bot}: ${rec.task.split("\n")[0]!.slice(0, 60)}`])
    }
    const ahead = git(rec.root, ["rev-list", "--count", `${rec.baseBranch}..${w.branch}`])
    if (ahead.ok && ahead.out === "0") {
      s.outcome = "nothing"
      s.detail = "no commits to merge"
    } else {
      const m = git(rec.root, ["merge", "--no-ff", "--no-edit", "-m", `Land swarm ${rec.id} ${w.bot}`, w.branch])
      if (!m.ok) {
        git(rec.root, ["merge", "--abort"])
        s.outcome = "conflict"
        s.detail = `merge stopped (${(m.out + " " + m.err).trim().split("\n").slice(-1)[0]}); the tree stays at ${w.workdir}`
        continue
      }
      s.outcome = "merged"
      s.detail = git(rec.root, ["rev-parse", "--short", "HEAD"]).out
    }
    try {
      await runEngine(["agents", "down", w.slug, "--root", rec.root], { timeoutMs: 30000 })
    } catch {
      // already stopped
    }
    git(rec.root, ["worktree", "remove", "--force", w.workdir])
    git(rec.root, ["branch", "-D", w.branch])
    const fresh = readSwarm(rec.id) ?? rec
    writeSwarm({ ...fresh, workers: fresh.workers.map((x) => (x.bot === w.bot ? { ...x, phase: "landed" as const } : x)) })
  }
  const after = readSwarm(rec.id)
  if (after && after.workers.every((w) => w.phase === "landed" || w.phase === "stopped" || (w.phase === "failed" && !fs.existsSync(w.workdir)))) {
    fs.rmSync(path.join(rec.root, ".allternit", "teams", rec.id), { recursive: true, force: true })
  }
  return steps
}
