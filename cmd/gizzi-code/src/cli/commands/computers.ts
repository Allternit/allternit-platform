/**
 * `gizzi computers ls|start|stop` — the computers Desktop's Computers view
 * shows (allternit-api `/api/v1/computers`) plus the runtimes paired to this
 * account (cloud-api `/api/v1/runtime-devices`), which `gizzi run --on` can
 * send a session to.
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import {
  listComputers,
  listRuntimes,
  pick,
  runOnRuntime,
  startComputer,
  stopComputer,
  type Computer,
  type Runtime,
} from "@/cli/work/api"

/** Print a failed request as one plain line (exit 1), not a stack. */
async function guard(fn: () => Promise<void>): Promise<void> {
  try {
    await fn()
  } catch (e) {
    UI.error((e as Error).message)
    process.exitCode = 1
  }
}

function pad(s: string, n: number): string {
  return s.length >= n ? s.slice(0, n - 1) + "…" : s.padEnd(n)
}

export function formatComputers(computers: Computer[], runtimes: Runtime[] | Error): string {
  const lines: string[] = ["Computers"]
  if (!computers.length) lines.push("  none")
  for (const c of computers) {
    lines.push(`  ${pad(c.status, 9)} ${pad(c.name, 36)} ${pad(c.kind, 14)} ${c.id}`)
  }
  lines.push("", "Paired runtimes (gizzi run --on <name or id>)")
  if (runtimes instanceof Error) lines.push(`  couldn't load: ${runtimes.message}`)
  else if (!runtimes.length) lines.push("  none")
  else
    for (const r of [...runtimes].sort((a, b) => Number(b.status === "online") - Number(a.status === "online")))
      lines.push(`  ${pad(r.status, 9)} ${pad(r.name, 36)} ${pad(r.runtimeType, 14)} ${r.id}`)
  return lines.join("\n")
}

const ComputersLsCommand = cmd({
  command: "ls",
  aliases: ["list"],
  describe: "list your computers and paired runtimes",
  builder: (y: Argv) => y.option("json", { type: "boolean", describe: "print JSON" }),
  handler: (args) => guard(async () => {
    const [computers, runtimes] = await Promise.all([
      listComputers(),
      listRuntimes().catch((e: Error) => e),
    ])
    if (args.json) {
      process.stdout.write(
        JSON.stringify({ computers, runtimes: runtimes instanceof Error ? [] : runtimes }, null, 2) + "\n",
      )
      return
    }
    process.stdout.write(formatComputers(computers, runtimes) + "\n")
  }),
})

function lifecycle(verb: "start" | "stop") {
  return cmd({
    command: `${verb} <computer>`,
    describe: `${verb} a computer (id or name from \`gizzi computers ls\`)`,
    builder: (y: Argv) => y.positional("computer", { type: "string", demandOption: true }),
    handler: (args) => guard(async () => {
      const key = String(args.computer)
      const target = pick(await listComputers(), key)
      if (!target) {
        UI.error(`No computer named or with id "${key}". Run \`gizzi computers ls\`.`)
        process.exitCode = 1
        return
      }
      const result = verb === "start" ? await startComputer(target.id) : await stopComputer(target.id)
      process.stdout.write(`${target.name}: ${result.status}\n`)
    }),
  })
}

export const ComputersCommand = cmd({
  command: "computers",
  describe: "your computers: list, start, stop (run a session on one with `gizzi run --on`)",
  builder: (yargs: Argv) =>
    yargs
      .command(ComputersLsCommand)
      .command(lifecycle("start"))
      .command(lifecycle("stop"))
      .demandCommand(1, "Specify a computers command (ls, start, stop)"),
  handler: () => {},
})

/**
 * `gizzi run --on <computer> "…"`: start the session on another paired
 * runtime (a cloud computer, a VPS, another Mac) through the cloud relay and
 * print where to follow it. The turn runs there, in that computer's engine.
 */
export async function runOnComputer(key: string, message: string, model?: string, title?: string): Promise<number> {
  if (!message.trim()) {
    UI.error("Give the message to run, e.g. gizzi run --on my-cloud \"fix the failing test\"")
    return 1
  }
  const runtimes = await listRuntimes().catch((e: Error) => e)
  if (runtimes instanceof Error) {
    UI.error(`Couldn't list paired runtimes: ${runtimes.message}`)
    return 1
  }
  const target = pick(runtimes, key)
  if (!target) {
    UI.error(`No paired runtime named or with id "${key}". Run \`gizzi computers ls\`.`)
    return 1
  }
  if (target.status !== "online") {
    process.stderr.write(`${target.name} is ${target.status}; the relay wakes a cloud computer, which can take a minute.\n`)
  }
  const [providerID, ...rest] = (model ?? "").split("/")
  try {
    const run = await runOnRuntime(target.id, message, {
      title,
      model: model && rest.length ? { providerID, modelID: rest.join("/") } : undefined,
    })
    process.stdout.write(`Running on ${target.name} · session ${run.sessionID}\n  web:   ${run.web}\n  phone: ${run.phone}\n`)
    return 0
  } catch (e) {
    UI.error((e as Error).message)
    return 1
  }
}
