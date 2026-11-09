/**
 * `gizzi swarm` and `gizzi board` — delegations you can start, watch and
 * land from one terminal, over the Factory engine (see cli/factory/swarm.ts).
 *
 *   gizzi swarm "<task>" --workers 2 --brain claude-cli/claude-sonnet-5
 *   gizzi board                       live board of every running bot
 *   gizzi swarm wait [id]             block until every worker finishes, then a summary
 *   gizzi swarm land [id] [--worker w1] [--all] [--dry-run]
 *   gizzi swarm list
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { EXIT, FactoryEngineError } from "@/cli/factory/engine"
import { C, renderError } from "@/cli/factory/render"
import { FactoryLive } from "@/cli/factory/live"
import { formatElapsed, STATE_GLYPH } from "@/cli/factory/board"
import { runBoardTty, printBoardOnce } from "@/cli/factory/board-tty"
import {
  drainReady,
  isSettled,
  landSwarm,
  listSwarms,
  observeSwarm,
  planSwarm,
  resolveSwarm,
  startSwarm,
  type SwarmRecord,
  type SwarmWorker,
} from "@/cli/factory/swarm"

const color = () => !!process.stdout.isTTY && !process.env.NO_COLOR
const paint = (code: string, s: string) => (color() ? `${code}${s}${C.reset}` : s)

function fail(err: unknown, json: boolean): number {
  const e =
    err instanceof FactoryEngineError ? err : new FactoryEngineError({ code: "transport", fact: (err as Error)?.message || String(err) })
  if (json) console.log(JSON.stringify(e.toJSON()))
  else console.error(renderError(e, { color: color() }))
  return e.exitCode
}

const PHASE_GLYPH: Record<SwarmWorker["phase"], string> = {
  starting: STATE_GLYPH.starting,
  working: STATE_GLYPH.working,
  waiting: STATE_GLYPH.waiting,
  done: STATE_GLYPH.done,
  failed: STATE_GLYPH.failed,
  stopped: STATE_GLYPH.stopped,
  landed: "⇣",
}

function summaryLines(rec: SwarmRecord): string[] {
  const out = [`${paint(C.bold, rec.id)} · ${rec.brain} · ${rec.task.split("\n")[0]}`]
  for (const w of rec.workers) {
    const took = w.finishedAt ? formatElapsed(w.finishedAt - w.startedAt) : formatElapsed(Date.now() - w.startedAt)
    out.push(`  ${PHASE_GLYPH[w.phase]} ${w.address.padEnd(14)} ${w.phase.padEnd(8)} ${took.padEnd(8)} ${w.branch}${w.note ? `  ${paint(C.dim, w.note)}` : ""}`)
  }
  return out
}

// ---------------------------------------------------------------- start

async function start(args: { task: string; tasks: string[]; workers: number; brain: string; json: boolean; dryRun: boolean }): Promise<number> {
  try {
    if (args.dryRun) {
      const plan = planSwarm({ task: args.task, tasks: args.tasks, workers: args.workers, brain: args.brain, cwd: process.cwd() })
      if (args.json) console.log(JSON.stringify({ dryRun: true, plan }, null, 2))
      else {
        console.log(`Would start swarm ${plan.id} from ${plan.root} (${plan.baseBranch}):`)
        for (const w of plan.workers) {
          console.log(`  ${w.address}  ${plan.harness}${plan.model ? ` · ${plan.model}` : ""}  worktree ${w.workdir}  branch ${w.branch}`)
          console.log(`    task: ${JSON.stringify((w.task ?? args.task).split("\n")[0])}`)
        }
      }
      return EXIT.ok
    }
    const { record } = await startSwarm({
      task: args.task,
      tasks: args.tasks,
      workers: args.workers,
      brain: args.brain,
      cwd: process.cwd(),
      log: args.json ? undefined : (l) => console.log(l),
    })
    if (args.json) console.log(JSON.stringify(record, null, 2))
    else {
      console.log("")
      console.log(`Swarm ${paint(C.bold, record.id)} is running. Watch it: ${paint(C.gizzi, "gizzi board")} · wait for it: gizzi swarm wait ${record.id} · land it: gizzi swarm land ${record.id}`)
    }
    return record.workers.every((w) => w.phase === "failed") ? EXIT.transport : EXIT.ok
  } catch (err) {
    return fail(err, args.json)
  }
}

// ---------------------------------------------------------------- wait

async function wait(args: { id?: string; json: boolean; timeout?: number }): Promise<number> {
  let rec: SwarmRecord
  try {
    rec = resolveSwarm(args.id)
  } catch (err) {
    return fail(err, args.json)
  }
  const live = new FactoryLive({ lines: false })
  const announced = new Set<string>()
  let clock: ReturnType<typeof setInterval> | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  const result = await new Promise<{ code: number; rec: SwarmRecord }>((resolve) => {
    if (args.timeout) timer = setTimeout(() => resolve({ code: EXIT.timeout, rec }), args.timeout * 1000)
    // Workers keep their time; a clock tick re-folds idle-after-send without asking the engine anything.
    clock = setInterval(() => check(live.current.agents, live.current.connected), 5000)
    function check(agents: typeof live.current.agents, connected: boolean) {
      if (!connected) return
      rec = observeSwarm(rec, agents)
      drainReady(rec, agents)
      for (const w of rec.workers) {
        const key = `${w.bot}:${w.phase}`
        if (announced.has(key) || !(w.phase === "waiting" || isSettled(w))) continue
        announced.add(key)
        if (!args.json) {
          const msg = w.phase === "waiting" ? `▲ ${w.address} is waiting on you — open \`gizzi board\` and press enter to answer it` : `${PHASE_GLYPH[w.phase]} ${w.address} ${w.phase}${w.note ? ` (${w.note})` : ""}`
          console.error(msg)
        }
      }
      if (rec.workers.every(isSettled)) {
        resolve({ code: rec.workers.some((w) => w.phase === "failed") ? EXIT.refused : EXIT.ok, rec })
      }
    }
    live.subscribe((v) => {
      if (!v.connected && v.error && !live.url) {
        resolve({ code: EXIT.transport, rec })
      }
      check(v.agents, v.connected)
    })
    void live.start()
  })
  if (timer) clearTimeout(timer)
  clearInterval(clock)
  live.stop()
  if (args.json) console.log(JSON.stringify({ swarm: result.rec, exitCode: result.code }, null, 2))
  else {
    if (result.code === EXIT.timeout) console.error(`Timed out after ${args.timeout}s; workers are still running.`)
    if (result.code === EXIT.transport) console.error(live.current.error)
    for (const l of summaryLines(result.rec)) console.log(l)
    if (result.code === EXIT.ok || result.code === EXIT.refused) console.log(`Next: gizzi swarm land ${result.rec.id} --dry-run`)
  }
  return result.code
}

// ---------------------------------------------------------------- land / list

async function land(args: { id?: string; workers: string[]; all: boolean; dryRun: boolean; json: boolean }): Promise<number> {
  try {
    const rec = resolveSwarm(args.id)
    const steps = await landSwarm(rec, { bots: args.workers, includeUnfinished: args.all, dryRun: args.dryRun })
    if (args.json) console.log(JSON.stringify({ swarm: rec.id, dryRun: args.dryRun, steps }, null, 2))
    else {
      console.log(`${args.dryRun ? "Would land" : "Landed"} swarm ${rec.id} into ${rec.baseBranch} at ${rec.root}:`)
      for (const s of steps) {
        const what = args.dryRun ? `${s.action}: ${s.reason}` : `${s.outcome ?? s.action}${s.detail ? ` — ${s.detail}` : s.outcome === "skipped" ? ` — ${s.reason}` : ""}`
        console.log(`  ${s.bot.padEnd(4)} ${s.branch.padEnd(24)} ${what}`)
      }
    }
    return steps.some((s) => s.outcome === "conflict" || s.outcome === "failed") ? EXIT.refused : EXIT.ok
  } catch (err) {
    return fail(err, args.json)
  }
}

function list(json: boolean): number {
  const all = listSwarms()
  if (json) console.log(JSON.stringify({ swarms: all }, null, 2))
  else if (!all.length) console.log('No swarms yet. Start one: gizzi swarm "<task>" --workers 2 --brain claude-cli/<model>')
  else for (const r of all) for (const l of summaryLines(r)) console.log(l)
  return EXIT.ok
}

// ---------------------------------------------------------------- commands

const VERBS = new Set(["wait", "land", "list"])

export const SwarmCommand = cmd({
  command: "swarm [args..]",
  describe: "Hand one task to N CLI agents in their own worktrees; then wait, watch (gizzi board) and land",
  builder: (y: Argv) =>
    y
      .strict(false)
      .positional("args", { type: "string", array: true, describe: '"<task>", or wait|land|list [id]' })
      .option("workers", { alias: "n", type: "number", default: 2, describe: "how many workers (1–12)" })
      .option("task", { type: "string", array: true, describe: "one task per worker (repeat it; sets --workers)" })
      .option("brain", { type: "string", default: "claude-cli/claude-sonnet-5", describe: "CLI brain: claude-cli/<model>, codex-cli/<model>, kimi-cli, grok-cli, gizzi" })
      .option("worker", { type: "string", array: true, describe: "land: only these workers (w1, w2…)" })
      .option("all", { type: "boolean", default: false, describe: "land: also workers that haven't finished" })
      .option("timeout", { type: "number", describe: "wait: give up after this many seconds (exit 4)" })
      .option("dry-run", { type: "boolean", default: false, describe: "print exactly what would happen" })
      .option("json", { type: "boolean", default: false, describe: "one JSON document on stdout" })
      .example('$0 swarm "add type hints to calc.py" -n 2 --brain claude-cli/claude-sonnet-5', "start two Claude Code workers")
      .example('$0 swarm --task "write tests for add" --task "add a sub function"', "one worker per task")
      .example("$0 board", "watch them live")
      .example("$0 swarm wait", "block until the newest swarm finishes")
      .example("$0 swarm land --dry-run", "see what landing would merge"),
  handler: async (argv) => {
    const words = ((argv.args as string[] | undefined) ?? []).map(String)
    const json = !!argv.json
    const verb = words[0]
    if (verb && VERBS.has(verb) && words.length <= 2) {
      const id = words[1]
      if (verb === "wait") process.exitCode = await wait({ id, json, timeout: argv.timeout as number | undefined })
      else if (verb === "land")
        process.exitCode = await land({ id, workers: ((argv.worker as string[]) ?? []).map(String), all: !!argv.all, dryRun: !!argv["dry-run"], json })
      else process.exitCode = list(json)
      return
    }
    const task = words.join(" ").trim()
    const tasks = ((argv.task as string[] | undefined) ?? []).map(String).filter((t) => t.trim())
    if (!task && !tasks.length) {
      console.error('Give the swarm a task: gizzi swarm "<task>" --workers 2 --brain claude-cli/claude-sonnet-5')
      process.exitCode = EXIT.usage
      return
    }
    process.exitCode = await start({ task, tasks, workers: Number(argv.workers), brain: String(argv.brain), json, dryRun: !!argv["dry-run"] })
  },
})

export const BoardCommand = cmd({
  command: "board",
  describe: "Live board of every running bot and swarm worker: state, brain, age, last line (enter attach · s steer · x stop · l land)",
  builder: (y: Argv) => y.option("once", { type: "boolean", default: false, describe: "print one frame and exit (no screen takeover)" }),
  handler: async (argv) => {
    process.exitCode = argv.once ? await printBoardOnce(color()) : await runBoardTty()
  },
})
