/**
 * `gizzi phone task "<goal>" [--phone]` and `gizzi phone runs [--phone]` —
 * start a task on a linked phone, follow it, and answer its approval holds
 * from the terminal. Routes (allternit-api, Desktop's gateway; contract in
 * surfaces/docs/api/phone-holds.mdx):
 *
 * - `GET  /api/v1/phones`                      linked phones + online state
 * - `POST /api/v1/phones/:serial/task`         the run (answers when it ends)
 * - `GET  /api/v1/phones/history`              runs; a run waiting on a hold
 *                                              carries `status: waiting_approval`
 *                                              and `approvalId`
 * - `GET  /api/v1/phones/holds/:id[/crop]`     the hold and its screen image
 * - `POST /api/v1/inbox/approval:<id>/approve` `{decision: approve|deny}`
 *
 * Approve is only offered after the screen image has been shown. Nothing here
 * approves on its own: with no terminal to ask, the hold is left for the app.
 */
import { spawn } from "node:child_process"
import { mkdtemp, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createInterface } from "node:readline"
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { platformFetch, PlatformSignedOutError } from "@/runtime/bots/platform-api"

// ── transport ─────────────────────────────────────────────────────────────

export class PhoneLineError extends Error {}

type Reply = { status: number; body: any; raw: Response }

async function call(method: "GET" | "POST", path: string, body?: unknown, timeoutMs = 15_000): Promise<Reply> {
  let raw: Response
  try {
    raw = await platformFetch(method, path, body, { timeoutMs })
  } catch (e) {
    if (e instanceof PlatformSignedOutError) throw new PhoneLineError("You're signed out. Run `gizzi login` first.")
    throw new PhoneLineError("Allternit Desktop isn't reachable. Open it (or run `gizzi serve`) and try again.")
  }
  const ct = raw.headers.get("content-type") ?? ""
  const parsed = ct.includes("json") ? await raw.clone().json().catch(() => undefined) : undefined
  return { status: raw.status, body: parsed, raw }
}

function reason(r: Reply): string {
  return r.body?.message || r.body?.error || `HTTP ${r.status}`
}

// ── phones ────────────────────────────────────────────────────────────────

export interface Phone {
  serial: string
  name: string
  model?: string
  online: boolean
}

export async function listPhones(): Promise<Phone[]> {
  const r = await call("GET", "/api/v1/phones")
  if (r.status === 401 || r.status === 403) throw new PhoneLineError("You're signed out. Run `gizzi login` first.")
  if (r.status !== 200) throw new PhoneLineError(`Couldn't list your phones: ${reason(r)}`)
  return (r.body?.phones ?? []) as Phone[]
}

const label = (p: Phone) => p.name || p.model || p.serial

/** Which phone a command means, or the one plain line that says why none. */
export function choosePhone(phones: Phone[], key?: string): Phone {
  if (phones.length === 0)
    throw new PhoneLineError("No phone is linked. Link one in Allternit Desktop (Phone Link) or the Allternit phone app.")
  let phone: Phone | undefined
  if (key) {
    const k = key.toLowerCase()
    phone =
      phones.find((p) => p.serial === key) ??
      phones.find((p) => p.name.toLowerCase() === k) ??
      phones.filter((p) => p.name.toLowerCase().startsWith(k) || p.serial.toLowerCase().startsWith(k))[0]
    if (!phone) throw new PhoneLineError(`No linked phone matches "${key}". You have: ${phones.map(label).join(", ")}.`)
  } else if (phones.length === 1) phone = phones[0]
  else {
    const online = phones.filter((p) => p.online)
    if (online.length !== 1)
      throw new PhoneLineError(`You have ${phones.length} phones (${phones.map(label).join(", ")}). Pick one with --phone.`)
    phone = online[0]
  }
  if (!phone!.online) throw new PhoneLineError(`${label(phone!)} is offline. Wake it or reconnect it, then try again.`)
  return phone!
}

// ── runs and holds ────────────────────────────────────────────────────────

export interface RunRow {
  run_id: string
  serial: string
  started_at: string
  ended_at: string
  steps: number
  failed: number
  source?: string | null
  status?: string
  step?: number
  step_label?: string | null
  approvalId?: string
  effect?: string
}

export async function listRuns(): Promise<RunRow[]> {
  const r = await call("GET", "/api/v1/phones/history")
  if (r.status !== 200) throw new PhoneLineError(`Couldn't read phone runs: ${reason(r)}`)
  return (r.body?.runs ?? []) as RunRow[]
}

export interface Hold {
  approvalId: string
  inboxId: string
  state: string
  effect: string
  expiresAt?: string
  phone?: { app?: string; phoneName?: string; element?: { label?: string }; intent?: string; to?: string }
}

async function getHold(id: string): Promise<Hold> {
  const r = await call("GET", `/api/v1/phones/holds/${encodeURIComponent(id)}`)
  if (r.status !== 200) throw new PhoneLineError(`Couldn't read the approval: ${reason(r)}`)
  return r.body as Hold
}

export function describeHold(h: Hold, now = Date.now()): string[] {
  const p = h.phone ?? {}
  const left = h.expiresAt ? Math.max(0, Math.round((Date.parse(h.expiresAt) - now) / 1000)) : undefined
  const el = p.element?.label || p.intent || ""
  const lines = [`Needs your OK on ${p.phoneName || "your phone"}: ${h.effect}${el ? ` · "${el}"` : ""}`]
  lines.push(`  app: ${p.app || "unknown"}${p.to ? ` · to: ${p.to}` : ""}${left === undefined ? "" : ` · expires in ${left}s`}`)
  return lines
}

// ── screen image ──────────────────────────────────────────────────────────

function openWithViewer(path: string): void {
  const [bin, ...args] =
    process.platform === "darwin" ? ["open", path] : process.platform === "win32" ? ["cmd", "/c", "start", "", path] : ["xdg-open", path]
  try {
    spawn(bin!, args, { stdio: "ignore", detached: true }).on("error", () => {}).unref()
  } catch {
    /* the path is printed anyway */
  }
}

/** Show the hold's screen image. Returns false when none could be loaded. */
async function showCrop(id: string): Promise<boolean> {
  const r = await call("GET", `/api/v1/phones/holds/${encodeURIComponent(id)}/crop`, undefined, 30_000)
  if (r.status !== 200) {
    process.stdout.write(`  The screen image didn't load (${reason(r)}). You can deny this, or approve it in the app.\n`)
    return false
  }
  const bytes = Buffer.from(await r.raw.arrayBuffer())
  const ext = (r.raw.headers.get("content-type") ?? "").includes("png") ? "png" : "jpg"
  const dir = await mkdtemp(join(tmpdir(), "gizzi-phone-"))
  const file = join(dir, `hold-${id}.${ext}`)
  await writeFile(file, bytes)
  if (process.stdout.isTTY && ["iTerm.app", "WezTerm"].includes(process.env.TERM_PROGRAM ?? "")) {
    process.stdout.write(`\x1b]1337;File=inline=1;width=40;preserveAspectRatio=1:${bytes.toString("base64")}\x07\n`)
  } else {
    openWithViewer(file)
  }
  process.stdout.write(`  Screen image: ${file}\n`)
  return true
}

// ── decision ──────────────────────────────────────────────────────────────

export type Decision = "approved" | "denied" | "left"

export interface HoldIO {
  /** Show the hold's screen image; false when it didn't load. */
  showCrop(id: string): Promise<boolean>
  /** `approve` / `deny`; 409 review_required is reported as "review_required". */
  decide(id: string, decision: "approve" | "deny"): Promise<"ok" | "review_required" | string>
  /** The owner's choice, or undefined when no terminal can ask. */
  ask(hold: Hold, state: { cropShown: boolean }): Promise<"approve" | "deny" | "open" | undefined>
  say(line: string): void
}

/**
 * One hold, from first sight to answer: show it, open the crop on request,
 * approve only once the crop has been shown (a 409 review_required opens it
 * and asks again), deny whenever asked.
 */
export async function resolveHold(hold: Hold, io: HoldIO): Promise<Decision> {
  for (const l of describeHold(hold)) io.say(l)
  let cropShown = false
  for (let turns = 0; turns < 20; turns++) {
    const choice = await io.ask(hold, { cropShown })
    if (!choice) {
      io.say("  No terminal to ask on, so I left it. Approve or deny it in the Allternit app.")
      return "left"
    }
    if (choice === "open") {
      cropShown = await io.showCrop(hold.approvalId)
      continue
    }
    if (choice === "deny") {
      const r = await io.decide(hold.inboxId, "deny")
      if (r !== "ok") {
        io.say(`  Couldn't deny it: ${r}`)
        continue
      }
      io.say("  Denied. The phone stays as it is.")
      return "denied"
    }
    if (!cropShown) {
      io.say("  Look at the screen image first.")
      cropShown = await io.showCrop(hold.approvalId)
      continue
    }
    const r = await io.decide(hold.inboxId, "approve")
    if (r === "review_required") {
      io.say("  Review first: opening the screen image.")
      cropShown = await io.showCrop(hold.approvalId)
      continue
    }
    if (r !== "ok") {
      io.say(`  Couldn't approve it: ${r}`)
      continue
    }
    io.say("  Approved. The run continues.")
    return "approved"
  }
  return "left"
}

// ── follow a run ──────────────────────────────────────────────────────────

export interface TaskReport {
  ok?: boolean
  status?: string
  result?: string
  message?: string
  reason?: string
  steps?: Array<unknown>
  run_id?: string
}

export interface FollowDeps {
  /** The task request: settles when the run ends. */
  task: Promise<TaskReport>
  /** Runs of this phone that started after the task did. */
  runs(): Promise<RunRow[]>
  /** Answer one hold (see resolveHold). */
  hold(approvalId: string): Promise<Decision>
  say(line: string): void
  pollMs?: number
}

/**
 * Print step progress and answer each hold once while the task request is
 * open. Holds are handled one at a time because the run is blocked on them;
 * a hold that was already answered or expired in the app is skipped by `hold`.
 */
export function formatStep(row: RunRow): string {
  const n = row.step ?? row.steps
  const label = row.step_label?.trim()
  return `  step ${n}${label ? ` · ${label}` : ""}${row.failed ? ` (${row.failed} failed)` : ""}`
}

export async function followTask(d: FollowDeps): Promise<TaskReport> {
  let done = false
  const finished = d.task.finally(() => {
    done = true
  })
  const seen = new Set<string>()
  let lastStep = 0
  let lastLine = ""
  const pollMs = d.pollMs ?? 1500
  while (!done) {
    await Promise.race([finished.catch(() => {}), new Promise((r) => setTimeout(r, pollMs))])
    if (done) break
    let rows: RunRow[] = []
    try {
      rows = await d.runs()
    } catch {
      continue // a missed poll only delays the next line
    }
    const row = rows[0]
    if (row && (row.step ?? row.steps) > 0) {
      const n = row.step ?? row.steps
      const line = formatStep(row)
      // Once per step, plus once more if the label changes (e.g. it starts waiting for an OK).
      if (n > lastStep || (n === lastStep && line !== lastLine && row.step_label)) {
        lastStep = n
        lastLine = line
        d.say(line)
      }
    }
    const waiting = rows.find((r) => r.status === "waiting_approval" && r.approvalId && !seen.has(r.approvalId))
    if (waiting?.approvalId) {
      seen.add(waiting.approvalId)
      await d.hold(waiting.approvalId)
    }
  }
  return finished
}

const OUTCOME: Record<string, string> = {
  done: "Done.",
  denied: "Stopped: you denied the approval.",
  approval_timeout: "Stopped: nobody answered the approval in time.",
  phone_locked: "Stopped: the phone was locked.",
  failed: "Failed.",
}

export function describeOutcome(r: TaskReport): string {
  const head = OUTCOME[r.status ?? ""] ?? (r.ok ? "Done." : `Ended: ${r.status ?? "unknown"}.`)
  return r.result ? `${head} ${r.result}` : head
}

// ── terminal prompt ───────────────────────────────────────────────────────

function terminalIO(): HoldIO {
  const say = (l: string) => process.stdout.write(l + "\n")
  return {
    say,
    showCrop,
    async decide(id, decision) {
      const r = await call("POST", `/api/v1/inbox/${encodeURIComponent(id)}/approve`, { decision, surface: "gizzi" })
      if (r.status === 200) return "ok"
      if (r.status === 409 && r.body?.error === "review_required") return "review_required"
      if (r.status === 409) return "the screen or the approval changed, so nothing was approved"
      return reason(r)
    },
    async ask(_hold, { cropShown }) {
      if (!process.stdin.isTTY || !process.stdout.isTTY) return undefined
      const rl = createInterface({ input: process.stdin, output: process.stdout })
      const q = cropShown ? "  [a]pprove  [d]eny  [o]pen image again > " : "  [o]pen image  [d]eny > "
      const answer: string = await new Promise((res) => rl.question(q, res))
      rl.close()
      const c = answer.trim().toLowerCase()[0]
      if (c === "a") return "approve"
      if (c === "d") return "deny"
      return "open"
    },
  }
}

// ── commands ──────────────────────────────────────────────────────────────

async function guard(fn: () => Promise<void>): Promise<void> {
  try {
    await fn()
  } catch (e) {
    if (e instanceof PhoneLineError) process.stderr.write(e.message + "\n")
    else UI.error((e as Error).message)
    process.exitCode = 1
  }
}

const phoneOpt = (y: Argv) =>
  y.option("phone", { type: "string", describe: "phone serial or name (default: your only phone)" })

const TaskCommand = cmd({
  command: "task <goal..>",
  describe: "tell your phone what to do, and answer its approvals here",
  builder: (y: Argv) =>
    phoneOpt(y)
      .positional("goal", { type: "string", array: true, demandOption: true })
      .option("thread", { type: "string", describe: "bot thread this task belongs to (default: $GIZZI_THREAD_ID)" })
      .option("session", { type: "string", describe: "session this task belongs to (default: $GIZZI_SESSION_ID)" }),
  handler: (args) =>
    guard(async () => {
      const goal = (args.goal as string[]).join(" ").trim()
      if (!goal) throw new PhoneLineError('Say what to do, e.g. gizzi phone task "open Settings".')
      const phone = choosePhone(await listPhones(), args.phone as string | undefined)
      const thread = (args.thread as string | undefined) || process.env.GIZZI_THREAD_ID || undefined
      const session = (args.session as string | undefined) || process.env.GIZZI_SESSION_ID || undefined
      const body: Record<string, unknown> = { goal, source: thread || session ? "chat" : "api" }
      if (thread) body.thread_id = thread
      if (session) body.session_id = session
      const startedAt = Date.now() - 2000
      process.stdout.write(`Starting on ${label(phone)}: ${goal}\n`)
      const task = call("POST", `/api/v1/phones/${encodeURIComponent(phone.serial)}/task`, body, 6 * 60_000).then(
        (r): TaskReport => {
          if (r.status === 403 && r.body?.status === "refused") return { ...r.body, ok: false }
          if (r.status === 401 || r.status === 403) throw new PhoneLineError("You're signed out. Run `gizzi login` first.")
          if (r.status !== 200) throw new PhoneLineError(reason(r))
          return r.body as TaskReport
        },
      )
      const io = terminalIO()
      const report = await followTask({
        task,
        say: io.say,
        runs: async () =>
          (await listRuns())
            .filter((r) => r.serial === phone.serial && Date.parse(r.started_at) >= startedAt)
            .sort((a, b) => Date.parse(b.started_at) - Date.parse(a.started_at)),
        hold: async (id) => {
          const hold = await getHold(id)
          if (hold.state !== "requested") {
            io.say(`  That approval is already ${hold.state}.`)
            return "left"
          }
          return resolveHold(hold, io)
        },
      })
      if (report.status === "refused") {
        process.stdout.write((report.message || "The task was refused.") + "\n")
        process.exitCode = 1
        return
      }
      process.stdout.write(describeOutcome(report) + "\n")
      if (report.status !== "done" && report.ok !== true) process.exitCode = 1
    }),
})

export function formatRuns(rows: RunRow[], phones: Phone[] = []): string {
  if (!rows.length) return "No phone runs yet."
  const name = (s: string) => phones.find((p) => p.serial === s)?.name || s
  return rows
    .map((r) => {
      const status = r.status === "waiting_approval" ? `waiting for you (${r.effect ?? "approval"})` : r.failed ? `ended, ${r.failed} failed` : "ended"
      return `${r.started_at.slice(0, 16).replace("T", " ")}  ${name(r.serial).padEnd(18).slice(0, 18)} ${String(r.steps).padStart(3)} steps  ${status}  ${r.source ?? ""}`.trimEnd()
    })
    .join("\n")
}

const RunsCommand = cmd({
  command: "runs",
  describe: "recent phone runs with their status",
  builder: (y: Argv) => phoneOpt(y).option("limit", { type: "number", default: 20 }),
  handler: (args) =>
    guard(async () => {
      const phones = await listPhones().catch(() => [] as Phone[])
      const key = args.phone as string | undefined
      let serial: string | undefined
      if (key) {
        const m = phones.find((p) => p.serial === key || p.name.toLowerCase() === key.toLowerCase())
        serial = m?.serial ?? key
      }
      const rows = (await listRuns()).filter((r) => !serial || r.serial === serial).slice(0, Number(args.limit) || 20)
      process.stdout.write(formatRuns(rows, phones) + "\n")
    }),
})

export const PhoneCommand = cmd({
  command: "phone",
  describe: "run tasks on your linked phone and approve what it asks",
  builder: (y: Argv) =>
    y.command(TaskCommand).command(RunsCommand).demandCommand(1, "Specify a phone command (task, runs)"),
  handler: () => {},
})
