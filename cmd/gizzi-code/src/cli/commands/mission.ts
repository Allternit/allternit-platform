/**
 * `gizzi mission` — a long-horizon goal that runs on its own until done
 * (see cli/mission/state.ts for the model).
 *
 *   gizzi mission start "<goal>" [--max-workers 3] [--max-hours 6] [-m <brain>] [--yes]
 *   gizzi mission ls
 *   gizzi mission status [id]
 *   gizzi mission resume [id] [--max-hours N]
 *   gizzi mission stop [id]
 *   gizzi mission land [id] [--merge]
 *
 * `start` plans (one orchestrator turn), shows the plan, and after approval
 * starts a detached runner (`gizzi mission run <id>`, hidden) that works in
 * the mission's own git worktree, so this checkout is never switched. The
 * prompt-free steps live in cli/mission/actions.ts for the TUI's `/mission`.
 */
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import readline from "node:readline"
import { spawnSync } from "node:child_process"
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { EXIT } from "@/cli/factory/engine"
import { C } from "@/cli/factory/render"
import {
  approveAndSpawn,
  landAndRecord,
  newMission,
  planMission,
  prepareResume,
  realDeps,
  startRunner,
  stopMission,
  type SpawnOutcome,
} from "@/cli/mission/actions"
import { onUnsupported, servingEngine } from "@/cli/mission/orchestrator"
import { runMission } from "@/cli/mission/runner"
import { badge, label, markdownToPlan, planProblems, planToMarkdown, type Mission, type PlanInput } from "@/cli/mission/state"
import { activeMission, listMissions, logFile, projectRoot, readMission, runnerAlive, writeMission } from "@/cli/mission/store"
import { workerBrain } from "@/cli/commands/factory/swarm"

const color = () => !!process.stdout.isTTY && !process.env.NO_COLOR
const paint = (code: string, s: string) => (color() ? `${code}${s}${C.reset}` : s)

function root(): string {
  return projectRoot(process.cwd())
}

/** The mission an id names, or the active one, or the newest; prints why not and returns null. */
function pick(id: string | undefined): Mission | null {
  const r = root()
  if (id) {
    const m = readMission(r, id)
    if (!m) console.error(`No mission ${id} in ${r}. See: gizzi mission ls`)
    return m
  }
  const m = activeMission(r) ?? listMissions(r)[0] ?? null
  if (!m) console.error('No missions in this project yet. Start one: gizzi mission start "<goal>"')
  return m
}

function ask(question: string): Promise<string> {
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout })
  return new Promise((resolve) => {
    rl.question(question, (a) => {
      rl.close()
      resolve(a.trim())
    })
    rl.on("close", () => resolve(""))
  })
}

/** Open the plan in $EDITOR (vi when unset) and read it back. */
function editPlan(m: Mission, plan: PlanInput): PlanInput | undefined {
  const file = path.join(os.tmpdir(), `gizzi-${m.id}-plan.md`)
  fs.writeFileSync(file, planToMarkdown(m.goal, plan))
  const editor = process.env.VISUAL || process.env.EDITOR || "vi"
  const r = spawnSync("/bin/sh", ["-c", `${editor} "$1"`, "sh", file], { stdio: "inherit" })
  if (r.status !== 0) {
    console.error(`${editor} exited ${r.status ?? r.signal}; the plan is unchanged`)
    return undefined
  }
  try {
    return markdownToPlan(fs.readFileSync(file, "utf8"))
  } finally {
    fs.rmSync(file, { force: true })
  }
}

function printPlan(m: Mission, plan: PlanInput): void {
  console.log("")
  console.log(planToMarkdown(m.goal, plan))
}

/**
 * Show the plan and ask until it is approved (returns the plan to run) or
 * cancelled (undefined). `--yes` approves a plan that has no problems.
 */
async function approvalLoop(m: Mission, plan: PlanInput, problems: string[], yes: boolean): Promise<{ plan?: PlanInput; mission: Mission }> {
  let cur = plan
  let issues = problems
  let mission = m
  for (;;) {
    printPlan(mission, cur)
    if (issues.length) {
      console.error(paint(C.red, "This plan can't run yet:"))
      for (const p of issues) console.error(`  - ${p}`)
    }
    if (yes) return issues.length ? { mission } : { plan: cur, mission }
    if (!process.stdin.isTTY) {
      console.error("stdin isn't a terminal, so nothing can approve the plan here; pass --yes to approve it")
      return { mission }
    }
    const a = (await ask(issues.length ? "r retry · e edit · q cancel > " : "Enter approve · e edit · q cancel > ")).toLowerCase()
    if (a === "q") return { mission }
    if (a === "e") {
      const edited = editPlan(mission, cur)
      if (edited) {
        cur = edited
        issues = planProblems(cur)
      }
      continue
    }
    if (a === "r" && issues.length) {
      console.log("Asking the orchestrator again…")
      const out = await planMission(mission, { engine: mission.engineURL, retry: issues })
      mission = out.mission
      if (out.error) {
        console.error(out.error)
        continue
      }
      if (out.plan) cur = out.plan
      issues = out.problems
      continue
    }
    if (a === "" && !issues.length) return { plan: cur, mission }
  }
}

function printSpawned(out: SpawnOutcome): number {
  if (out.error) {
    console.error(out.error)
    return EXIT.refused
  }
  const m = out.mission
  console.log(`${paint(C.bold, m.id)} running in the background${out.pid ? ` (pid ${out.pid})` : ""} · ${label(m)}`)
  console.log(`  work:   ${m.worktree} (branch ${m.branch}, lands on ${m.baseBranch})`)
  if (m.on) console.log(`  orchestrator on ${m.on.name ?? m.on.runtime}${m.on.web ? `: ${m.on.web}` : ""}`)
  console.log(`  log:    ${out.log}`)
  console.log(`  watch:  gizzi mission status ${m.id}`)
  return EXIT.ok
}

/** Cancel before approval: kept as stopped so `resume` can bring the plan back. */
function cancel(m: Mission): number {
  const latest = readMission(m.root, m.id) ?? m
  writeMission({ ...latest, status: "stopped", note: "cancelled before approval", endedAt: Date.now() })
  console.log(`Cancelled. The plan is kept: gizzi mission resume ${m.id}`)
  return EXIT.ok
}

/** Plan (when there is none yet), ask for approval, then spawn the runner. */
async function planApproveSpawn(m: Mission, yes: boolean): Promise<number> {
  let mission = m
  let plan: PlanInput | undefined
  let problems: string[] = []
  if (mission.milestones.length) {
    plan = { milestones: mission.milestones.map((x) => ({ title: x.title, tracks: x.tracks, validate: x.validate })) }
  } else {
    const unsupported = await onUnsupported(mission)
    if (unsupported) {
      console.error(unsupported)
      return EXIT.usage
    }
    console.log(`Planning ${paint(C.bold, mission.id)} with ${mission.brain}${mission.on ? ` on ${mission.on.runtime}` : ""}…`)
    const engine = mission.on ? undefined : await servingEngine()
    const out = await planMission(mission, { engine })
    mission = out.mission
    if (out.error) {
      console.error(out.error)
      return EXIT.refused
    }
    plan = out.plan ?? { milestones: [] }
    problems = out.problems
  }
  const decided = await approvalLoop(mission, plan, problems, yes)
  if (!decided.plan) {
    if (yes) {
      console.error("The plan has problems, so --yes didn't approve it. Fix it: gizzi mission resume " + mission.id)
      writeMission({ ...decided.mission, status: "stopped", note: "plan had problems" })
      return EXIT.refused
    }
    return cancel(decided.mission)
  }
  return printSpawned(approveAndSpawn(readMission(mission.root, mission.id) ?? decided.mission, { plan: decided.plan }))
}

// ---------------------------------------------------------------- verbs

async function start(argv: { goal: string; on?: string; maxWorkers: number; maxHours?: number; maxAttempts: number; brain?: string; yes: boolean }): Promise<number> {
  const picked = await workerBrain(argv.brain)
  if ("error" in picked) {
    console.error(picked.error)
    return EXIT.usage
  }
  const made = newMission({ goal: argv.goal, cwd: process.cwd(), brain: picked.brain, maxWorkers: argv.maxWorkers, maxAttempts: argv.maxAttempts, maxHours: argv.maxHours, on: argv.on })
  if ("error" in made) {
    console.error(made.error)
    return EXIT.usage
  }
  return planApproveSpawn(made.mission, argv.yes)
}

function ls(json: boolean): number {
  const all = listMissions(root())
  if (json) console.log(JSON.stringify({ missions: all.map((m) => ({ id: m.id, goal: m.goal, status: m.status, badge: badge(m), label: label(m), runnerAlive: runnerAlive(m), landed: m.landed })) }, null, 2))
  else if (!all.length) console.log('No missions in this project yet. Start one: gizzi mission start "<goal>"')
  else
    for (const m of all) {
      const alive = runnerAlive(m) ? paint(C.green, " ●") : ""
      console.log(`${paint(C.bold, m.id)}  ${badge(m).padEnd(8)} ${m.status.padEnd(17)} ${label(m)}${alive}  ${paint(C.dim, m.goal.split("\n")[0]!.slice(0, 60))}`)
    }
  return EXIT.ok
}

const MS_GLYPH: Record<string, string> = { pending: "○", working: "◐", validating: "◑", fixing: "↻", passed: "✓", failed: "✗" }

function tail(m: Mission, n: number): string[] {
  try {
    return fs.readFileSync(logFile(m), "utf8").trimEnd().split("\n").slice(-n)
  } catch {
    return m.log.slice(-n).map((l) => `${new Date(l.at).toISOString()} ${l.text}`)
  }
}

function status(id: string | undefined, json: boolean): number {
  const m = pick(id)
  if (!m) return EXIT.usage
  const alive = runnerAlive(m)
  if (json) {
    console.log(JSON.stringify({ ...m, runnerAlive: alive, logTail: tail(m, 10) }, null, 2))
    return EXIT.ok
  }
  console.log(`${paint(C.bold, m.id)} · ${label(m)} · ${m.status}${m.note ? ` — ${m.note}` : ""}`)
  console.log(`  goal:    ${m.goal.split("\n")[0]}`)
  console.log(`  brain:   ${m.brain} · ${m.maxWorkers} workers max · ${m.maxAttempts} fix rounds${m.maxHours ? ` · ${m.maxHours} h max` : ""}`)
  console.log(`  branch:  ${m.branch} → ${m.baseBranch}${m.landed === true ? " (landed)" : m.landed === false ? ` (not landed: ${m.landNote})` : ""}`)
  console.log(`  work:    ${m.worktree}`)
  console.log(`  runner:  ${alive ? `alive (pid ${m.runnerPid})` : m.status === "running" ? `not running (it stopped without recording why; pick it up: gizzi mission resume ${m.id})` : "not running"}`)
  if (m.on) console.log(`  on:      ${m.on.name ?? m.on.runtime}${m.on.sessionID ? ` · session ${m.on.sessionID}` : ""}${m.on.web ? ` · ${m.on.web}` : ""}`)
  if (m.milestones.length) {
    console.log("")
    m.milestones.forEach((ms, i) => {
      const here = i === m.current && m.status !== "done" ? paint(C.bold, " ←") : ""
      console.log(`  ${MS_GLYPH[ms.status] ?? "·"} ${ms.id} ${ms.title}  ${paint(C.dim, ms.status)}${ms.attempts ? paint(C.dim, ` · ${ms.attempts} failed check${ms.attempts === 1 ? "" : "s"}`) : ""}${here}`)
      ms.tracks.forEach((t, j) => console.log(`      ${j < ms.started ? "•" : "◦"} ${t.title}`))
      for (const f of ms.fixes) console.log(`      ↻ ${f.title}`)
      console.log(paint(C.dim, `      validate: ${ms.validate}`))
    })
  }
  const lines = tail(m, 10)
  if (lines.length) {
    console.log("")
    for (const l of lines) console.log(paint(C.dim, `  ${l}`))
  }
  if (m.status === "done" && m.landed === false) console.log(`\nready to land: gizzi mission land ${m.id}`)
  return EXIT.ok
}

async function resume(id: string | undefined, maxHours: number | undefined, yes: boolean): Promise<number> {
  const m = pick(id)
  if (!m) return EXIT.usage
  const r = prepareResume(m, { maxHours })
  if (r.kind === "error") {
    console.error(r.text)
    return EXIT.refused
  }
  if (r.kind === "approve") {
    writeMission(r.mission)
    return planApproveSpawn(r.mission, yes)
  }
  return printSpawned(startRunner(r.mission))
}

async function stop(id: string | undefined): Promise<number> {
  const m = pick(id)
  if (!m) return EXIT.usage
  if (!["running", "needs_you", "awaiting_approval", "planning"].includes(m.status)) {
    console.log(`Mission ${m.id} is already ${m.status}`)
    return EXIT.ok
  }
  const out = await stopMission(m)
  if (out.signalled) console.log(`Stopping ${m.id}: sent SIGTERM to runner ${m.runnerPid}; it stops the workers and records the state.`)
  else console.log(`Stopped ${m.id}${out.errors.length ? ` (couldn't stop: ${out.errors.join("; ")})` : ""}. Pick it up again: gizzi mission resume ${m.id}`)
  return out.errors.length ? EXIT.refused : EXIT.ok
}

function land(id: string | undefined, merge: boolean): number {
  const m = pick(id)
  if (!m) return EXIT.usage
  if (m.status !== "done") {
    console.error(`Mission ${m.id} is ${m.status}; only a done mission lands`)
    return EXIT.refused
  }
  if (m.landed) {
    console.log(`Mission ${m.id} already landed: ${m.landNote ?? m.baseBranch}`)
    return EXIT.ok
  }
  const next = landAndRecord(m, realDeps().git, { merge })
  writeMission(next)
  if (next.landed) console.log(`Landed ${m.id}: ${next.landNote}`)
  else console.error(`Not landed: ${next.landNote}`)
  return next.landed ? EXIT.ok : EXIT.refused
}

async function run(id: string): Promise<number> {
  try {
    const m = await runMission(id, root())
    return m.status === "done" || m.status === "stopped" ? EXIT.ok : EXIT.refused
  } catch (err) {
    console.error((err as Error)?.message ?? String(err))
    return EXIT.refused
  }
}

// ---------------------------------------------------------------- command

const idArg = (y: Argv) => y.positional("id", { type: "string", describe: "mission id (default: the running one, else the newest)" })
const jsonOpt = (y: Argv) => y.option("json", { type: "boolean", default: false, describe: "one JSON document on stdout" })

export const MissionCommand = cmd({
  command: "mission",
  describe: "Hand a long goal to an orchestrator: it plans milestones, runs them as swarms in a mission worktree, validates, and lands",
  builder: (yargs: Argv) =>
    yargs
      .command({
        command: "start <goal..>",
        describe: "plan a mission, ask for approval, then run it in the background",
        builder: (y: Argv) =>
          y
            .positional("goal", { type: "string", array: true, describe: "what the mission should get done" })
            .option("on", { type: "string", describe: "run the orchestrator's plan and fix turns on a paired computer (name or id from `gizzi computers ls`); workers, validation and landing stay here" })
            .option("max-workers", { type: "number", default: 3, describe: "parallel workers per milestone" })
            .option("max-hours", { type: "number", describe: "stop to ask after this many hours" })
            .option("max-attempts", { type: "number", default: 2, describe: "fix rounds per milestone before it stops to ask" })
            .option("brain", { alias: "m", type: "string", describe: "orchestrator and worker brain: claude-cli/<model>, codex-cli/<model>, kimi-cli, grok-cli, gizzi (default: your default CLI brain)" })
            .option("yes", { alias: "y", type: "boolean", default: false, describe: "approve the plan without asking" })
            .example('$0 mission start "add a --json flag to every list command, with tests"', "plan it, approve, run")
            .example('$0 mission start "port the parser to Rust" --max-hours 6 -m codex-cli/gpt-6.1-sol --yes', "no prompt"),
        handler: async (argv: any) => {
          process.exitCode = await start({
            goal: ((argv.goal as string[] | undefined) ?? []).map(String).join(" "),
            on: argv.on ? String(argv.on) : undefined,
            maxWorkers: Number(argv["max-workers"]),
            maxHours: argv["max-hours"] !== undefined ? Number(argv["max-hours"]) : undefined,
            maxAttempts: Number(argv["max-attempts"]),
            brain: argv.brain ? String(argv.brain) : undefined,
            yes: !!argv.yes,
          })
        },
      })
      .command({
        command: "ls",
        aliases: ["list"],
        describe: "this project's missions",
        builder: jsonOpt,
        handler: (argv: any) => {
          process.exitCode = ls(!!argv.json)
        },
      })
      .command({
        command: "status [id]",
        describe: "milestones, tracks, the last log lines, the runner and the branch",
        builder: (y: Argv) => jsonOpt(idArg(y)),
        handler: (argv: any) => {
          process.exitCode = status(argv.id as string | undefined, !!argv.json)
        },
      })
      .command({
        command: "resume [id]",
        describe: "pick a stopped or waiting mission up again",
        builder: (y: Argv) =>
          idArg(y)
            .option("max-hours", { type: "number", describe: "a new hours limit (needed when the mission ran over its old one)" })
            .option("yes", { alias: "y", type: "boolean", default: false, describe: "approve the plan without asking (missions never approved)" }),
        handler: async (argv: any) => {
          process.exitCode = await resume(argv.id as string | undefined, argv["max-hours"] !== undefined ? Number(argv["max-hours"]) : undefined, !!argv.yes)
        },
      })
      .command({
        command: "stop [id]",
        describe: "stop the runner and its workers; the state is kept for resume",
        builder: idArg,
        handler: async (argv: any) => {
          process.exitCode = await stop(argv.id as string | undefined)
        },
      })
      .command({
        command: "land [id]",
        describe: "merge a done mission's branch into the branch it started from",
        builder: (y: Argv) => idArg(y).option("merge", { type: "boolean", default: false, describe: "make a merge commit when it isn't a fast-forward" }),
        handler: (argv: any) => {
          process.exitCode = land(argv.id as string | undefined, !!argv.merge)
        },
      })
      .command({
        command: "run <id>",
        describe: false,
        builder: (y: Argv) => y.positional("id", { type: "string" }),
        handler: async (argv: any) => {
          process.exitCode = await run(String(argv.id))
        },
      })
      .demandCommand(1, "Specify a mission command (see gizzi mission --help)"),
  handler: () => {},
})
