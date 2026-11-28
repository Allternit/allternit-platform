/**
 * `gizzi sleep status | now | pause | resume | report` — Allternit Sleep
 * (memory consolidation) from the terminal (allternit-api `/api/v1/memory/sleep/*`).
 * Every verb takes `--json`. Pause and resume flip the Sleep switch.
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { getSleepReport, getSleepStatus, runSleepNow, setSleepEnabled } from "@/cli/work/api"
import { formatSleepStatus, sleepError } from "@/cli/work/sleep-model"

const withJson = (y: Argv) => y.option("json", { type: "boolean", describe: "print JSON" })

async function run(args: { json?: boolean }, fn: () => Promise<{ json: unknown; text: string }>) {
  try {
    const r = await fn()
    process.stdout.write((args.json ? JSON.stringify(r.json, null, 2) : r.text) + "\n")
  } catch (e) {
    const msg = sleepError(e)
    if (args.json) process.stdout.write(JSON.stringify({ error: msg }) + "\n")
    else UI.error(msg)
    process.exitCode = 1
  }
}

const StatusCommand = cmd({
  command: "status",
  describe: "whether Sleep is on, the last run and its report",
  builder: withJson,
  handler: (a) => run(a, async () => { const s = await getSleepStatus(); return { json: s, text: formatSleepStatus(s) } }),
})
const NowCommand = cmd({
  command: "now",
  describe: "run a Sleep cycle now",
  builder: withJson,
  handler: (a) =>
    run(a, async () => {
      const r = await runSleepNow()
      return { json: r, text: r.accepted === false ? `Sleep did not start: ${r.error ?? "refused"}` : "Sleep started. Check progress with `gizzi sleep status`." }
    }),
})
const PauseCommand = cmd({
  command: "pause",
  describe: "turn Sleep off until you resume it",
  builder: withJson,
  handler: (a) => run(a, async () => ({ json: await setSleepEnabled(false), text: "Sleep paused." })),
})
const ResumeCommand = cmd({
  command: "resume",
  describe: "turn Sleep back on",
  builder: withJson,
  handler: (a) => run(a, async () => ({ json: await setSleepEnabled(true), text: "Sleep resumed." })),
})
const ReportCommand = cmd({
  command: "report [id]",
  describe: "the report of the last cycle (or the cycle id you give)",
  builder: (y: Argv) => withJson(y).positional("id", { type: "string" }),
  handler: (a) =>
    run(a, async () => {
      const id = (a as { id?: string }).id ?? (await getSleepStatus()).last_cycle?.id
      if (!id) return { json: null, text: "Sleep has not run yet, so there is no report." }
      const r = await getSleepReport(id)
      return { json: r, text: typeof r.report === "string" ? r.report : JSON.stringify(r.report, null, 2) }
    }),
})

export const SleepCommand = cmd({
  command: "sleep",
  describe: "Allternit Sleep: memory consolidation status, run now, pause, resume, report",
  builder: (yargs: Argv) =>
    yargs.command(StatusCommand).command(NowCommand).command(PauseCommand).command(ResumeCommand).command(ReportCommand).demandCommand(1, "Specify a sleep command (status, now, pause, resume, report)"),
  handler: () => {},
})
