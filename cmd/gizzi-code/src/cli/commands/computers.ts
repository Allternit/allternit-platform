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
  Asleep,
  consoleEnabled,
  formatLogLine,
  formatPorts,
  formatServices,
  formatStatus,
  formatUpdates,
  hostGet,
  NOT_AVAILABLE,
  resolveRow,
  streamLogs,
  Unavailable,
  type HostPort,
  type HostService,
  type HostSnapshot,
  type HostUpdate,
} from "@/cli/work/console"
import {
  cloudBase,
  createRestorePoint,
  listRestorePoints,
  restoreRestorePoint,
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
const HONEST_LINE = "Hosted computers run on Allternit servers. They sleep when idle."

type ComputeCredits = { price_per_hour_usd?: number; stopped_price_per_hour_usd?: number; min_start_usd?: number } | null
type ComputePaths = {
  own?: { download_url?: string }
  plan?: { plans?: { name: string; price_usd_month: number }[] }
  credits?: ComputeCredits
}
/** `computer.hours` for plan computers with included hours (Plus). */
export type ComputeHours = {
  included_hours: number
  used_hours: number
  overage_price_per_hour_usd?: number
  stopped_for_hours?: boolean
  resumes_at?: string | null
}
export type ComputeStatus = {
  state: "none" | "paired" | "provisioning" | "hosted" | "suspended"
  online?: boolean
  gate?: boolean
  computer?: { id: string; kind: string; provider: string | null; status: string; sleeps_when_idle?: boolean; hours?: ComputeHours | null } | null
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
    "What works without a computer",
    "  Works on any device: Allternit Cloud models (they need credits: a pack or a plan), any API key you add,",
    "  and plans you already pay for where the vendor allows it (GitHub Copilot now; Claude and others later).",
    "  Needs a computer: clicking apps, driving a browser, running coding tools (Codex CLI, Claude Code), and keeping agents",
    "  running while your own machine sleeps.",
    "",
    "Ways to get a computer",
    "",
    "  1. Link your own computer. Free. Link the Allternit Desktop app.",
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

/** One plain line about this month's plan hours, or undefined when the computer has none. */
export function formatComputeHours(h: ComputeHours | null | undefined): string | undefined {
  if (!h || !(h.included_hours > 0) || !Number.isFinite(h.used_hours)) return undefined
  if (h.stopped_for_hours) {
    const at = h.resumes_at ? new Date(h.resumes_at) : undefined
    const when =
      at && !Number.isNaN(at.getTime())
        ? ` or it starts again on ${at.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" })}`
        : " or wait for next month"
    return `Stopped: no credits left. Add credits${when}.`
  }
  if (h.used_hours >= h.included_hours) {
    const price = h.overage_price_per_hour_usd === undefined ? "" : ` at ${money(h.overage_price_per_hour_usd)}/hour`
    return `${h.included_hours} hours used. Extra hours bill from credits${price}.`
  }
  return `${Math.floor(h.used_hours)} of ${h.included_hours} hours used this month`
}

export function formatComputeStatus(s: ComputeStatus): string {
  const c = s.computer
  const lines = [`State: ${s.state}${s.online ? " (online)" : ""}`]
  if (c) lines.push(`Computer: ${c.kind}${c.provider ? ` on ${c.provider}` : ""}, ${c.status} (${c.id})`)
  else lines.push("Computer: none")
  const hoursLine = formatComputeHours(c?.hours)
  if (hoursLine) lines.push(hoursLine)
  if (s.state === "suspended" && !c?.hours?.stopped_for_hours) lines.push("Your credits ran out and the computer stopped. Add credits to start it again.")
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
    if (e instanceof Unavailable) {
      process.stdout.write(NOT_AVAILABLE + "\n")
      return
    }
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
  command: "status [computer]",
  describe: "show whether this account has a computer for agents; with a computer, its health",
  builder: (y: Argv) =>
    y.positional("computer", { type: "string", describe: "id or name: show CPU, memory, disk, uptime (Computers console)" }).option("json", { type: "boolean", describe: "print JSON" }),
  handler: (args) => guard(async () => {
    if (args.computer) {
      if (!consoleEnabled()) {
        process.stdout.write(NOT_AVAILABLE + "\n")
        return
      }
      const row = await resolveRow(String(args.computer))
      let snap: HostSnapshot | undefined
      let asleep = false
      try {
        snap = await hostGet<HostSnapshot>(row.id, "snapshot")
      } catch (e) {
        if (!(e instanceof Asleep)) throw e
        asleep = true
        snap = e.lastSample ?? row.lastSample
      }
      process.stdout.write((args.json ? JSON.stringify({ computer: row, asleep, snapshot: snap ?? null }, null, 2) : formatStatus(row, snap, asleep)) + "\n")
      return
    }
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


// ── Computers console (hidden unless GIZZI_COMPUTERS_CONSOLE=1) ─────────────
// Contract: surfaces/docs/dev/computers-console-contract.mdx.

/** Console subcommands do nothing unless the `computers_console` flag (GIZZI_COMPUTERS_CONSOLE=1) is on. */
function gated(fn: () => Promise<void>): Promise<void> {
  if (!consoleEnabled()) {
    process.stdout.write(NOT_AVAILABLE + "\n")
    return Promise.resolve()
  }
  return guard(fn)
}

function hostList<T>(name: string, key: string, describe: string, format: (rows: T[]) => string) {
  return cmd({
    command: `${name} <computer>`,
    describe: consoleEnabled() ? describe : false,
    builder: (y: Argv) => y.positional("computer", { type: "string", demandOption: true }).option("json", { type: "boolean", describe: "print JSON" }),
    handler: (args) => gated(async () => {
      const row = await resolveRow(String(args.computer))
      const body = await hostGet<Record<string, T[]>>(row.id, name)
      const rows = body[key] ?? []
      process.stdout.write((args.json ? JSON.stringify(rows, null, 2) : format(rows)) + "\n")
    }),
  })
}

const ComputersLogsCommand = cmd({
  command: "logs <computer> <unit>",
  describe: consoleEnabled() ? "service logs (masked on the computer; --errors, --lines N, --follow)" : false,
  builder: (y: Argv) =>
    y
      .positional("computer", { type: "string", demandOption: true })
      .positional("unit", { type: "string", demandOption: true })
      .option("errors", { type: "boolean", describe: "only warnings and errors" })
      .option("lines", { type: "number", default: 100, describe: "how many recent lines to print" })
      .option("follow", { type: "boolean", alias: "f", describe: "keep streaming new lines" })
      .option("json", { type: "boolean", describe: "print one JSON object per line" }),
  handler: (args) => gated(async () => {
    const row = await resolveRow(String(args.computer))
    const ctl = new AbortController()
    process.once("SIGINT", () => ctl.abort())
    // Without --follow, take what the stream replays in 8 seconds, then stop.
    const timer = args.follow ? undefined : setTimeout(() => ctl.abort(), 8_000)
    const keep = (l: { level: string }) => !args.errors || l.level === "error" || l.level === "warn"
    const out = (l: Parameters<typeof formatLogLine>[0]) => process.stdout.write((args.json ? JSON.stringify(l) : formatLogLine(l)) + "\n")
    const limit = Math.max(1, Math.floor(args.lines as number))
    const tail: Parameters<typeof formatLogLine>[0][] = []
    try {
      // The stream replays recent history first, then goes live. Without --follow, stop after `lines`.
      for await (const l of streamLogs(row.id, String(args.unit), ctl.signal)) {
        if (!keep(l)) continue
        if (args.follow) out(l)
        else {
          tail.push(l)
          if (tail.length >= limit) break
        }
      }
    } catch (e) {
      if (!ctl.signal.aborted) throw e
    }
    clearTimeout(timer)
    ctl.abort()
    if (!args.follow) tail.slice(-limit).forEach(out)
  }),
})

function ask(question: string): Promise<boolean> {
  if (!process.stdin.isTTY) return Promise.resolve(false)
  return new Promise((resolve) => {
    process.stdout.write(`${question} [y/N] `)
    process.stdin.setEncoding("utf8")
    process.stdin.once("data", (d) => {
      process.stdin.pause()
      resolve(/^y(es)?$/i.test(String(d).trim()))
    })
  })
}

async function localComputer(key: string) {
  const target = pick(await listComputers(), key)
  if (!target) throw new Error(`No computer named or with id "${key}". Run \`gizzi computers ls\`.`)
  return target
}

const ComputersSleepCommand = cmd({
  command: "sleep <computer>",
  describe: consoleEnabled() ? "put a computer to sleep" : false,
  builder: (y: Argv) => y.positional("computer", { type: "string", demandOption: true }).option("yes", { type: "boolean", alias: "y", describe: "skip the question" }),
  handler: (args) => gated(async () => {
    const t = await localComputer(String(args.computer))
    if (!args.yes && !(await ask(`Put ${t.name} to sleep? Running work on it stops.`))) {
      process.stdout.write("Not put to sleep. Pass --yes to skip the question.\n")
      return
    }
    process.stdout.write(`${t.name}: ${(await stopComputer(t.id)).status}\n`)
  }),
})

const ComputersWakeCommand = cmd({
  command: "wake <computer>",
  describe: consoleEnabled() ? "wake a sleeping computer (asks first; a wake can use credits)" : false,
  builder: (y: Argv) => y.positional("computer", { type: "string", demandOption: true }).option("yes", { type: "boolean", alias: "y", describe: "skip the question" }),
  handler: (args) => gated(async () => {
    const t = await localComputer(String(args.computer))
    if (!args.yes && !(await ask(`Wake ${t.name}?`))) {
      process.stdout.write("Not woken. Pass --yes to wake without the question.\n")
      return
    }
    process.stdout.write(`${t.name}: ${(await startComputer(t.id)).status}\n`)
  }),
})

const ComputersSnapshotCommand = cmd({
  command: "snapshot <computer>",
  describe: consoleEnabled() ? "make a restore point (with no point id, list them)" : false,
  builder: (y: Argv) => y.positional("computer", { type: "string", demandOption: true }).option("list", { type: "boolean", describe: "list restore points instead" }).option("json", { type: "boolean", describe: "print JSON" }),
  handler: (args) => gated(async () => {
    const t = await localComputer(String(args.computer))
    if (args.list) {
      const pts = await listRestorePoints(t.id)
      process.stdout.write((args.json ? JSON.stringify(pts, null, 2) : pts.length ? pts.map((p) => `${pad(p.id, 40)} ${p.created_at ?? ""}`).join("\n") : "No restore points.") + "\n")
      return
    }
    const r = await createRestorePoint(t.id)
    process.stdout.write(args.json ? JSON.stringify(r) + "\n" : `Restore point ${r.snapshot_id} made on ${t.name}.\n`)
  }),
})

const ComputersRestoreCommand = cmd({
  command: "restore <computer> <point>",
  describe: consoleEnabled() ? "restore a computer to a restore point (asks first)" : false,
  builder: (y: Argv) =>
    y.positional("computer", { type: "string", demandOption: true }).positional("point", { type: "string", demandOption: true }).option("yes", { type: "boolean", alias: "y", describe: "skip the question" }),
  handler: (args) => gated(async () => {
    const t = await localComputer(String(args.computer))
    if (!args.yes && !(await ask(`Restore ${t.name} to ${args.point}? Changes made since then are lost.`))) {
      process.stdout.write("Not restored.\n")
      return
    }
    await restoreRestorePoint(t.id, String(args.point))
    process.stdout.write(`${t.name}: restored to ${args.point}.\n`)
  }),
})

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
      .command(hostList<HostService>("services", "services", "services on a computer", formatServices))
      .command(hostList<HostPort>("ports", "ports", "listening ports on a computer", formatPorts))
      .command(hostList<HostUpdate>("updates", "updates", "available updates on a computer", formatUpdates))
      .command(ComputersLogsCommand)
      .command(ComputersSleepCommand)
      .command(ComputersWakeCommand)
      .command(ComputersSnapshotCommand)
      .command(ComputersRestoreCommand)
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
