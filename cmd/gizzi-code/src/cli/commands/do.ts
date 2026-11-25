/**
 * `gizzi do [app] [tool]`: run one tool from an app through the Island bar, from the terminal.
 *
 *   gizzi do                                     pick an app, then a tool
 *   gizzi do stripe refund_payment --arg order_id=1042 --arg amount=320
 *   gizzi do stripe refund_payment --bot refunds --yes --json
 *
 * Backed by the runtime api (local allternit-api, else the cloud relay; see runtime-client.ts) /api/v1/bar/* (intent, review, jobs) and the MCP elicitation reply route.
 * `--yes` skips only the "Run it?" confirm. It never skips an owner rule: when the approver is
 * someone else the ask is still sent and this command waits for them.
 */
import type { Argv } from "yargs"
import * as prompts from "@clack/prompts"
import { EOL } from "os"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { RuntimeError, runtimeJson, runtimeSse, setComputer } from "@/cli/commands/runtime-client"
import {
  errorMessage,
  formatReview,
  needsOwnerApproval,
  receipt,
  statusLine,
  TERMINAL,
  type BarJob,
  type Review,
} from "@/cli/commands/do-format"

const BarError = RuntimeError
const friendly = errorMessage
const call = <T>(method: "GET" | "POST" | "DELETE", path: string, body?: unknown, _bot?: string) =>
  runtimeJson<T>(method, `/api/v1${path}`, body, friendly)
const botQuery = (bot?: string) => (bot ? `?bot_id=${encodeURIComponent(bot)}` : "")
const sse = (path: string, signal: AbortSignal) => runtimeSse(`/api/v1${path}`, signal, friendly)

const ok = <T>(v: T | symbol): T => {
  if (prompts.isCancel(v)) throw new UI.CancelledError()
  return v as T
}

type Blank = {
  key: string
  label: string
  type: string
  required?: boolean
  unit?: string
  secret?: boolean
  format?: string
  options?: { value: unknown; label: string }[]
  recent?: { value: unknown; label: string }[]
}

function isSecret(b: Blank) {
  return b.secret === true || b.format === "password" || b.type === "secret" || /(password|secret|token|api[_-]?key)/i.test(b.key)
}

function coerce(b: Blank, v: string): unknown {
  if (b.type === "number") return Number(v)
  if (b.type === "boolean") return /^(true|yes|1)$/i.test(v)
  return v
}

async function askBlank(b: Blank): Promise<unknown> {
  const label = `${b.label}${b.unit ? ` (${b.unit})` : ""}`
  if (b.type === "enum" && b.options?.length)
    return ok(await prompts.select({ message: label, options: b.options.map((o) => ({ value: o.value, label: o.label })) as any }))
  if (b.type === "boolean") return ok(await prompts.confirm({ message: label }))
  if (isSecret(b)) return ok(await prompts.password({ message: label, mask: "*" }))
  if (b.recent?.length) {
    const pick = ok(
      await prompts.select({
        message: label,
        options: [...b.recent.slice(0, 5).map((r) => ({ value: r.value, label: r.label })), { value: "__new__", label: "Something else..." }] as any,
      }),
    )
    if (pick !== "__new__") return pick
  }
  const v = ok(
    await prompts.text({
      message: label,
      validate: (s) => (b.required && !String(s ?? "").trim() ? "Required" : undefined),
    }),
  ) as string
  return coerce(b, v)
}

function parseArgs(list: string[]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const a of list) {
    const i = a.indexOf("=")
    if (i < 1) throw new Error(`--arg expects key=value, got "${a}"`)
    out[a.slice(0, i)] = a.slice(i + 1)
  }
  return out
}

async function pickApp(apps: any[], want: string | undefined): Promise<any> {
  if (want) {
    const w = want.toLowerCase()
    const hit = apps.find((a) => a.app.ref.toLowerCase() === w || a.app.name?.toLowerCase() === w) ??
      apps.filter((a) => a.app.name?.toLowerCase().includes(w) || a.app.ref.toLowerCase().includes(w))[0]
    if (!hit) throw new Error(`No app matching "${want}". Run \`gizzi do\` to pick from your apps.`)
    return hit
  }
  if (!apps.length) throw new Error("You have no apps yet. Connect one first.")
  return ok(
    await prompts.select({
      message: "App",
      options: apps.map((a) => ({ value: a, label: a.app.name ?? a.app.ref, hint: `${a.tool_count} tools · ${a.level}` })) as any,
    }),
  )
}

async function pickTool(tools: any[], want: string | undefined): Promise<any> {
  if (want) {
    const w = want.toLowerCase()
    const hit = tools.find((t) => t.name.toLowerCase() === w || t.title?.toLowerCase() === w) ??
      tools.filter((t) => t.name.toLowerCase().includes(w))[0]
    if (!hit) throw new Error(`No tool matching "${want}" in this app.`)
    return hit
  }
  return ok(
    await prompts.select({
      message: "Tool",
      options: tools.map((t) => ({ value: t, label: t.title ?? t.name, hint: t.read_only ? "read only" : undefined })) as any,
    }),
  )
}

async function answerElicitation(part: any, json: boolean): Promise<void> {
  const id = part?.requestID ?? part?.requestId ?? part?.id
  if (!id) return
  const reply = (body: unknown) => call("POST", `/mcp/elicitation/${encodeURIComponent(id)}/reply`, body).catch(() => {})
  if (json) return void (await reply({ action: "decline" }))
  prompts.log.info(part.message ?? "The app needs more input.")
  const props: Record<string, any> = part.requestedSchema?.properties ?? {}
  const content: Record<string, unknown> = {}
  try {
    for (const [key, s] of Object.entries(props)) {
      const b: Blank = {
        key,
        label: s.title ?? key,
        type: s.enum ? "enum" : s.type === "number" || s.type === "integer" ? "number" : s.type === "boolean" ? "boolean" : "text",
        format: s.format,
        options: s.enum?.map((v: string, i: number) => ({ value: v, label: s.enumNames?.[i] ?? v })),
        required: (part.requestedSchema?.required ?? []).includes(key),
      }
      content[key] = await askBlank(b)
    }
  } catch {
    return void (await reply({ action: "cancel" }))
  }
  await reply({ action: "accept", content })
}

export const DoCommand = cmd({
  command: "do [app] [tool]",
  describe: "run one tool from an app through the Island bar, with review and approval",
  builder: (y: Argv) =>
    y
      .positional("app", { type: "string", describe: "app name or ref (asked if left out)" })
      .positional("tool", { type: "string", describe: "tool name (asked if left out)" })
      .option("arg", { type: "string", array: true, default: [] as string[], describe: "blank value, key=value (repeatable)" })
      .option("bot", { type: "string", describe: "run as this bot (bot_id); without it you are the actor" })
      .option("computer", { type: "string", describe: "which online computer to use when more than one is connected" })
      .option("yes", { alias: "y", type: "boolean", describe: "skip the confirm; never skips an owner approval" })
      .option("json", { type: "boolean", describe: "print machine-readable events (JSON lines), no prompts"  }),
  async handler(args) {
    const json = !!args.json
    const bot = args.bot as string | undefined
    setComputer(args.computer as string | undefined)
    const say = (s: string) => (json ? undefined : UI.println(s))
    try {
      const given = parseArgs((args.arg as string[]) ?? [])
      const { apps } = await call<{ apps: any[] }>("GET", `/bar/apps${botQuery(bot)}`)
      if (json && (!args.app || !args.tool)) throw new Error("--json needs both app and tool.")
      const app = await pickApp(apps, args.app as string | undefined)
      if (app.app.kind === "artifact") throw new BarError(422, "app_not_runnable", errorMessage(422, "app_not_runnable"))
      const { tools } = await call<{ tools: any[] }>("GET", `/bar/apps/${app.app.kind}/${encodeURIComponent(app.app.ref)}/tools${botQuery(bot)}`)
      const tool = await pickTool(tools, args.tool as string | undefined)

      const intent = await call<any>("POST", "/bar/intent", { app: { kind: app.app.kind, ref: app.app.ref }, tool: tool.name, ...(bot ? { bot_id: bot } : {}) })
      const sentence = (intent.sentence ?? []).map((p: any) => (p.type === "text" ? p.text : `[${p.key}]`)).join("")
      say(`${sentence}`)

      const values: Record<string, unknown> = {}
      for (const b of intent.blanks as Blank[]) {
        if (given[b.key] !== undefined) values[b.key] = coerce(b, given[b.key])
        else if (json) {
          if (b.required) throw new Error(`Missing --arg ${b.key}=...`)
        } else values[b.key] = await askBlank(b)
      }

      const review = await call<Review>("POST", "/bar/review", { intent_id: intent.intent_id, args: values, ...(bot ? { bot_id: bot } : {}) })
      if (json) console.log(JSON.stringify({ type: "review", review }))
      else {
        UI.println("")
        UI.println(formatReview(review).join(EOL))
        UI.println("")
      }
      const mustWait = needsOwnerApproval(review)
      if (!args.yes && !json) {
        const go = ok(
          await prompts.confirm({
            message: mustWait ? `Send for approval to ${review.approver?.name ?? "the approver"} and run?` : "Run it?",
          }),
        )
        if (!go) return say("Cancelled. Nothing was run.")
      } else if (mustWait) {
        say(`--yes does not skip the rule: asking ${review.approver?.name ?? "the approver"} and waiting.`)
      }

      let job: BarJob
      try {
        job = (await call<{ job: BarJob }>("POST", `/bar/jobs/${review.job_id}/run`, undefined, bot)).job
      } catch (e) {
        if (e instanceof BarError && e.code === "owner_approval_required") {
          job = { id: review.job_id, state: "waiting_approval", approver_user_id: e.data?.approver?.user_id }
          ;(job as any)._approver = e.data?.approver?.name
        } else throw e
      }
      await follow(job, review.approver?.name ?? (job as any)._approver ?? null, json, bot)
    } catch (e: any) {
      if (e instanceof UI.CancelledError) return
      const msg = e instanceof BarError ? e.message : String(e?.message ?? e)
      if (json) console.log(JSON.stringify({ type: "error", code: e?.code, message: msg }))
      else UI.error(msg)
      process.exitCode = 1
    }
  },
})

async function follow(initial: BarJob, approver: string | null, json: boolean, bot?: string) {
  let job = initial
  let tick = 0
  const ac = new AbortController()
  let confirming = false
  const render = () => {
    if (json) return
    process.stdout.write(`\r\x1b[2K${statusLine(job, approver, tick++)}`)
  }
  const onSigint = async () => {
    if (confirming) return
    confirming = true
    process.stdout.write("\n")
    const yes = json ? true : await prompts.confirm({ message: "Cancel this job?" })
    confirming = false
    if (yes === true) {
      await call("POST", `/bar/jobs/${job.id}/cancel`, undefined, bot).catch(() => {})
      ac.abort()
    }
  }
  process.on("SIGINT", onSigint)
  const timer = setInterval(() => !confirming && !TERMINAL.includes(job.state) && render(), 600)
  try {
    if (json) console.log(JSON.stringify({ type: "bar_job", job }))
    else render()
    for await (const ev of sse(`/bar/jobs/${job.id}/events`, ac.signal)) {
      if (json) console.log(JSON.stringify(ev))
      const type = ev.type ?? ev.kind
      if (type === "bar_job") {
        job = ev.part ?? ev.job ?? job
        if (!json) render()
        if (TERMINAL.includes(job.state)) break
      } else if (type === "mcp_elicitation") {
        confirming = true
        if (!json) process.stdout.write("\n")
        await answerElicitation(ev.part, json)
        confirming = false
      }
    }
  } catch (e: any) {
    if (e?.name !== "AbortError") {
      if (e instanceof RuntimeError && e.code === "no_runtime") throw e
      // The event stream failed: fall back to reading the job every 1.5 s.
      if (!json) process.stdout.write("\n(Live updates are not available; checking every 1.5 s.)\n")
      while (!ac.signal.aborted && !TERMINAL.includes(job.state)) {
        await new Promise((r) => setTimeout(r, 1500))
        if (confirming) continue
        job = (await call<{ job: BarJob }>("GET", `/bar/jobs/${job.id}`, undefined, bot)).job
        if (json) console.log(JSON.stringify({ type: "bar_job", job }))
        else render()
      }
    }
  } finally {
    clearInterval(timer)
    process.off("SIGINT", onSigint)
  }
  if (!TERMINAL.includes(job.state)) {
    // Stream ended early (or we cancelled): read the final state once.
    job = (await call<{ job: BarJob }>("GET", `/bar/jobs/${job.id}`, undefined, bot).catch(() => ({ job }))).job
  }
  if (json) return console.log(JSON.stringify({ type: "result", state: job.state, receipt: job.state === "completed" ? receipt(job) : null, error: job.error ?? null }))
  process.stdout.write(`\r\x1b[2K`)
  UI.println(job.state === "completed" ? `✓ ${receipt(job)}` : statusLine(job, approver))
  if (job.state === "failed") process.exitCode = 1
}
