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
  cloudBase,
  findRuntime,
  listComputers,
  listRuntimes,
  modelRef,
  pick,
  runOnRuntime,
  startComputer,
  stopComputer,
  type Computer,
  type Runtime,
} from "@/cli/work/api"

const DOCS_URL = "https://docs.allternit.com/api/account-compute-status"
const HONEST_LINE = "Hosted computers run on a third-party cloud provider, managed by Allternit. They sleep when idle."

type ComputeCredits = { price_per_hour_usd?: number; stopped_price_per_hour_usd?: number; min_start_usd?: number } | null
type ComputePaths = {
  own?: { download_url?: string }
  plan?: { plans?: { name: string; price_usd_month: number }[] }
  credits?: ComputeCredits
}
export type ComputeStatus = {
  state: "none" | "paired" | "provisioning" | "hosted" | "suspended"
  online?: boolean
  gate?: boolean
  computer?: { id: string; kind: string; provider: string | null; status: string; sleeps_when_idle?: boolean } | null
  paths?: ComputePaths
  docs_url?: string
}

const money = (n: number | undefined) => (n === undefined ? "" : `$${Number(n.toFixed(3))}`)

/** The three ways to get a computer, with costs read from the server's paths (nothing hard-coded). */
export function formatComputePaths(paths: ComputePaths | undefined, docsUrl = DOCS_URL): string {
  const download = paths?.own?.download_url ?? "https://www.allternit.com/download"
  const plans = paths?.plan?.plans ?? []
  const credits = paths?.credits
  const lines = [
    "Agents need a computer. Pick where your agents run. Chat with Allternit Cloud models stays open.",
    "",
    "  1. Use your own computer. Free. Link the Allternit Desktop app.",
    `     gizzi computers link   (download: ${download})`,
    "  2. Subscribe." +
      (plans.length
        ? ` ${plans.map((p) => `${p.name} ${money(p.price_usd_month)}`).join(", ")} a month. Includes one hosted computer.`
        : " Includes one hosted computer."),
    "     https://platform.allternit.com/billing",
  ]
  if (credits) {
    lines.push(
      `  3. Buy credits. A hosted computer billed per minute while it runs. From ${money(credits.price_per_hour_usd)} an hour.`,
      "     https://platform.allternit.com/billing",
    )
  }
  lines.push("", HONEST_LINE, `Docs: ${docsUrl}`)
  return lines.join("\n")
}

export function formatComputeStatus(s: ComputeStatus): string {
  const c = s.computer
  const lines = [`State: ${s.state}${s.online ? " (online)" : ""}`]
  if (c) lines.push(`Computer: ${c.kind}${c.provider ? ` on ${c.provider}` : ""}, ${c.status} (${c.id})`)
  else lines.push("Computer: none")
  if (s.state === "suspended") lines.push("Your credits ran out and the computer stopped. Add credits to start it again.")
  if (s.state === "provisioning") lines.push("Starting your computer. This takes about a minute.")
  if (c?.status === "sleeping") lines.push("Your computer is asleep. It wakes when an agent needs it.")
  if (s.state === "none" || s.state === "suspended" || s.gate) lines.push("", formatComputePaths(s.paths, s.docs_url))
  else lines.push("", `Docs: ${s.docs_url ?? DOCS_URL}`)
  return lines.join("\n")
}

/** If an error carries a 409 compute_required body, the plain message for it; else undefined. */
export function computeRequiredMessage(text: string): string | undefined {
  if (!/compute_required/.test(text)) return undefined
  const start = text.indexOf("{")
  let paths: ComputePaths | undefined
  let docs: string | undefined
  if (start >= 0) {
    try {
      const body = JSON.parse(text.slice(start))
      paths = body?.error?.paths
      docs = body?.error?.docs_url
    } catch {}
  }
  return formatComputePaths(paths && Object.keys(paths).length ? paths : undefined, docs ?? DOCS_URL)
}

/** Print a failed request as one plain line (exit 1), not a stack. */
async function guard(fn: () => Promise<void>): Promise<void> {
  try {
    await fn()
  } catch (e) {
    UI.error(computeRequiredMessage((e as Error).message) ?? (e as Error).message)
    process.exitCode = 1
  }
}

async function authHeaders(): Promise<Record<string, string>> {
  const env = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (env) return { Authorization: `Bearer ${env}` }
  const { Pairing } = await import("@/runtime/services/pairing/pairing")
  const device = await Pairing.load().catch(() => undefined)
  return Pairing.tokenUsable(device) ? { Authorization: `Bearer ${device!.deviceToken!}` } : {}
}

/** GET /api/v1/account/compute-status. Throws a plain Error when it can't be read. */
export async function fetchComputeStatus(): Promise<ComputeStatus> {
  const res = await fetch(`${cloudBase()}/api/v1/account/compute-status`, {
    headers: await authHeaders(),
    signal: AbortSignal.timeout(10_000),
  })
  if (res.status === 401 || res.status === 403) throw new Error("Not signed in. Run `gizzi login`.")
  if (!res.ok) throw new Error(`compute status: ${res.status}`)
  return (await res.json()) as ComputeStatus
}

/** After login: print the status and, when there is no computer, the three paths. Never throws. */
export async function printComputeStatusAfterLogin(): Promise<void> {
  try {
    const s = await fetchComputeStatus()
    if (s.state === "none") UI.println("\n" + formatComputeStatus(s))
    else UI.println(`\nComputer: ${s.computer?.kind ?? s.state}, ${s.computer?.status ?? s.state}`)
  } catch {
    // Older servers have no compute-status route; login still succeeded.
  }
}

const ComputersStatusCommand = cmd({
  command: "status",
  describe: "show whether this account has a computer for agents, and the ways to get one",
  builder: (y: Argv) => y.option("json", { type: "boolean", describe: "print JSON" }),
  handler: (args) => guard(async () => {
    const s = await fetchComputeStatus()
    process.stdout.write((args.json ? JSON.stringify(s, null, 2) : formatComputeStatus(s)) + "\n")
  }),
})

const ComputersLinkCommand = cmd({
  command: "link",
  describe: "steps to link the Allternit Desktop app on your own computer",
  handler: () => guard(async () => {
    process.stdout.write(
      [
        "Link your own computer (free):",
        "  1. Download the Desktop app: https://www.allternit.com/download",
        "  2. Open Allternit on that computer. It opens a page in your browser; sign in and approve the link.",
        "     If the browser does not open, type the code shown in the app at https://platform.allternit.com/pair",
        "  3. Run `gizzi computers status`. It shows paired when the link is done.",
        "",
        "Docs: https://docs.allternit.com/guides/link-your-computer",
        "",
      ].join("\n"),
    )
  }),
})

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
      .command(ComputersStatusCommand)
      .command(ComputersLinkCommand)
      .demandCommand(1, "Specify a computers command (ls, start, stop, status, link)"),
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
  const found = await findRuntime(key)
  if ("error" in found) {
    UI.error(found.error)
    return 1
  }
  const target = found.runtime
  if (target.status !== "online") {
    process.stderr.write(`${target.name} is ${target.status}; the relay wakes a cloud computer, which can take a minute.\n`)
  }
  try {
    const run = await runOnRuntime(target.id, message, {
      title,
      model: modelRef(model),
    })
    process.stdout.write(`Running on ${target.name} · session ${run.sessionID}\n  web:   ${run.web}\n  phone: ${run.phone}\n`)
    return 0
  } catch (e) {
    UI.error(computeRequiredMessage((e as Error).message) ?? (e as Error).message)
    return 1
  }
}
