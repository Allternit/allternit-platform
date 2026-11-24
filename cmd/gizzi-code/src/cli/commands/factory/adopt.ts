/**
 * `gizzi agents adopt <session> [--copy | --new] [--cancel] [--yes] [--json]`
 *
 * Move a coding-agent session from macOS Terminal (or another terminal) into an
 * Allternit terminal. Talks to allternit-api: `GET /api/v1/live-sessions`,
 * `POST /api/v1/live-sessions/adopt`, `GET /api/v1/live-sessions/adopt/:jobId`.
 * Never signals a process itself; the runtime does that, and only after the
 * answer here is yes.
 */
import type { Argv } from "yargs"
import { createInterface } from "readline"
import { cmd } from "@/cli/commands/cmd"
import { platformFetch } from "@/runtime/bots/platform-api"
import {
  errorSentence,
  holderLine,
  matchSession,
  refuseMove,
  resumeCommand,
  stepLine,
  type AdoptJob,
  type LiveSession,
} from "./adopt-core"

type Mode = "move" | "copy" | "new" | "cancel"

class AdoptHttpError extends Error {
  constructor(
    readonly status: number,
    readonly code?: string,
    message?: string,
  ) {
    super(errorSentence(status, code, message))
  }
}

async function call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  const res = await platformFetch(method, path, body, { timeoutMs: 20000 })
  const text = await res.text().catch(() => "")
  let doc: any = undefined
  try {
    doc = text ? JSON.parse(text) : undefined
  } catch {
    // plain text body
  }
  if (!res.ok) throw new AdoptHttpError(res.status, doc?.error, doc?.message ?? (doc ? undefined : text))
  return doc as T
}

function ask(question: string): Promise<boolean> {
  const rl = createInterface({ input: process.stdin, output: process.stdout })
  return new Promise((resolve) => {
    rl.question(question, (a) => {
      rl.close()
      resolve(/^y(es)?$/i.test(a.trim()))
    })
  })
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function fail(json: boolean, message: string, extra?: object): never {
  if (json) console.log(JSON.stringify({ ok: false, error: message, ...extra }))
  else console.error(message)
  process.exitCode = 1
  throw new Done()
}
class Done extends Error {}

export const AdoptCommand = cmd({
  command: "adopt <session>",
  describe: "move a coding-agent session from Terminal into an Allternit terminal (--copy keeps the old one running)",
  builder: (y: Argv) =>
    y
      .positional("session", { type: "string", demandOption: true, describe: "session id, address, or a unique name/prefix from `agents sessions`" })
      .option("copy", { type: "boolean", describe: "open a copy in Allternit and leave the old session running" })
      .option("new", { type: "boolean", describe: "start a fresh session in the same folder; the old one is untouched" })
      .option("cancel", { type: "boolean", describe: "cancel a move that is still waiting for the agent to go idle" })
      .option("yes", { alias: "y", type: "boolean", describe: "don't ask before moving" })
      .option("json", { type: "boolean", describe: "print the raw result" })
      .conflicts("copy", "new"),
  handler: async (args) => {
    const json = !!args.json
    try {
      await run({
        query: String(args.session),
        mode: args.cancel ? "cancel" : args.copy ? "copy" : args.new ? "new" : "move",
        yes: !!args.yes,
        json,
      })
    } catch (e) {
      if (e instanceof Done) return
      process.exitCode = 1
      const msg = e instanceof Error ? e.message : String(e)
      if (json) console.log(JSON.stringify({ ok: false, error: msg }))
      else console.error(msg)
    }
  },
})

async function run(o: { query: string; mode: Mode; yes: boolean; json: boolean }) {
  const { json } = o
  const body = await call<LiveSession[] | { sessions?: LiveSession[] }>("GET", "/api/v1/live-sessions")
  const rows = Array.isArray(body) ? body : (body.sessions ?? [])
  const m = matchSession(rows, o.query)
  if (m.kind === "none") fail(json, `No session matches "${o.query}". Run \`gizzi agents sessions\` to see them.`)
  if (m.kind === "many") {
    const list = m.matches.map((s) => `  ${s.address}  ${s.harness}  ${s.sessionId ?? ""}`).join("\n")
    fail(json, `"${o.query}" matches ${m.matches.length} sessions. Use a longer name or the id:\n${list}`)
  }
  const s = m.session
  const label = s.name || s.address
  const req = (mode: Mode) => ({
    sessionId: s.sessionId,
    address: s.sessionId ? undefined : s.address,
    harness: s.harness,
    mode,
    cols: process.stdout.columns || 120,
    rows: process.stdout.rows || 32,
  })

  if (o.mode === "cancel") {
    const r = await call<unknown>("POST", "/api/v1/live-sessions/adopt", req("cancel"))
    if (json) console.log(JSON.stringify(r ?? { ok: true }))
    else console.log(`Cancelled the move of ${label}.`)
    return
  }

  if (o.mode === "copy" || o.mode === "new") {
    if (o.mode === "copy" && s.canCopy === false) fail(json, errorSentence(422, "no_copy"))
    const r = await call<{ terminalId?: string }>("POST", "/api/v1/live-sessions/adopt", req(o.mode))
    if (json) console.log(JSON.stringify(r))
    else console.log(`${o.mode === "copy" ? "Copied" : "Started a new session"} in Allternit: terminal ${r.terminalId}. The old session was not touched.`)
    return
  }

  const refusal = refuseMove(s)
  if (refusal) fail(json, refusal, { movable: s.movable })
  const holder = s.holders?.[0]
  if (!o.yes) {
    if (!process.stdin.isTTY) fail(json, "This will stop the old process and resume it in Allternit. Re-run with --yes to confirm.")
    console.log(`${label} (${s.harness}) is open in ${holder ? holderLine(holder) : "another terminal"}.`)
    console.log(`It will resume in Allternit with: ${resumeCommand(s)}`)
    if (s.movable === "busy") console.log("It is working now; the move waits until it is idle.")
    console.log("The old Terminal window stays open and shows the exit.")
    if (!(await ask("Move? [y/N] "))) fail(json, "Not moved.")
  }

  const started = await call<AdoptJob>("POST", "/api/v1/live-sessions/adopt", req("move"))
  if (!json) console.log(`Move started (job ${started.jobId}).`)
  let job = started
  let last = ""
  let cancelling = false
  const onSigint = () => {
    if (job.state === "waiting_idle" && !cancelling) {
      cancelling = true
      call("POST", "/api/v1/live-sessions/adopt", req("cancel")).catch(() => {})
      if (!json) console.log("Cancelling…")
    } else if (!cancelling) {
      if (!json) console.log("Too late to cancel; the move continues. Ctrl-C again to stop watching.")
      process.once("SIGINT", () => process.exit(130))
    }
  }
  process.on("SIGINT", onSigint)
  try {
    for (;;) {
      const line = stepLine(job, label, holder?.pid)
      if (line && line !== last && !json) console.log(line)
      if (line) last = line
      if (job.state === "done") break
      if (job.state === "failed") {
        const msg = errorSentence(0, job.code, job.error)
        fail(json, cancelling && job.code !== "too_late" ? "Move cancelled." : msg, { job })
      }
      await sleep(1000)
      job = await call<AdoptJob>("GET", `/api/v1/live-sessions/adopt/${encodeURIComponent(started.jobId)}`)
    }
  } finally {
    process.off("SIGINT", onSigint)
  }
  if (json) console.log(JSON.stringify(job))
}
