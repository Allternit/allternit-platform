import type { Argv } from "yargs"
import { readFileSync } from "fs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import {
  createStudioClient,
  exitCodeForError,
  exitCodeForState,
  followJob,
  hardwareFit,
  specFromFlags,
  StudioError,
  type EngineInfo,
  type JobEvent,
  type StudioClient,
} from "@/runtime/services/studioClient"

const out = (s: string): void => void process.stdout.write(s + "\n")
const json = (v: unknown) => out(JSON.stringify(v, null, 2))

function table(rows: string[][]) {
  const w = rows[0].map((_, i) => Math.max(...rows.map((r) => (r[i] ?? "").length)))
  for (const r of rows) out(r.map((c, i) => c.padEnd(w[i])).join("  ").trimEnd())
}

/** Prefer the host's own fit verdict; fall back to a local check for older hosts. */
function hostFit(e: EngineInfo & { fit?: { ok?: boolean; reason?: string } }): { fits: boolean; reason?: string } {
  if (e.fit && typeof e.fit.ok === "boolean") return { fits: e.fit.ok, reason: e.fit.reason }
  return hardwareFit(e)
}

function fmtEvent(ev: JobEvent): string {
  if (ev.kind === "progress" && typeof ev.progress === "number") return `[${ev.seq}] progress ${Math.round(ev.progress * 100)}% ${ev.message}`.trimEnd()
  return `[${ev.seq}] ${ev.kind}: ${ev.message}`
}

/** Follow a job to its end. Ctrl-C detaches without cancelling (exit 130). */
async function follow(client: StudioClient, id: string, asJson: boolean, after = 0): Promise<number> {
  const ac = new AbortController()
  const onSig = () => ac.abort()
  process.once("SIGINT", onSig)
  try {
    const r = await followJob(client, id, {
      after,
      signal: ac.signal,
      onEvent: (ev) => (asJson ? out(JSON.stringify(ev)) : out(fmtEvent(ev))),
    })
    if (r.detached) {
      process.stderr.write(`Detached. The job keeps running: gizzi studio logs ${id} --follow\n`)
      return 130
    }
    if (!asJson) out(`Job ${id} ${r.state}`)
    return exitCodeForState(r.state)
  } finally {
    process.removeListener("SIGINT", onSig)
  }
}

async function run(fn: (c: StudioClient) => Promise<number | void>, asJson: boolean): Promise<void> {
  try {
    const code = await fn(createStudioClient())
    process.exit(typeof code === "number" ? code : 0)
  } catch (err) {
    const e = err instanceof StudioError ? err : undefined
    if (asJson) process.stderr.write(JSON.stringify({ error: e?.code ?? "error", message: (err as Error).message }) + "\n")
    else UI.error(e && e.status === 422 ? `Refused (${e.code}): ${e.message}` : e && e.code !== "unreachable" ? `${e.code}: ${e.message}` : (err as Error).message)
    process.exit(exitCodeForError(err))
  }
}

export const StudioCommand = cmd({
  command: "studio <action> [target]",
  describe: "fine-tune models on this computer: engines, jobs, logs, outputs",
  builder: (yargs: Argv) =>
    yargs
      .positional("action", {
        describe: "engines | install <engine> | run | jobs | logs <id> | cancel <id> | outputs <id>",
        type: "string",
        choices: ["engines", "install", "run", "jobs", "logs", "cancel", "outputs"],
      })
      .positional("target", { describe: "engine id (install) or job id (logs, cancel, outputs)", type: "string" })
      .option("json", { describe: "print JSON instead of text", type: "boolean", default: false })
      .option("spec", { describe: "run: path to a job spec v1 JSON file", type: "string" })
      .option("engine", { describe: "run: engine id (when no --spec)", type: "string" })
      .option("base", { describe: "run: base model repo, e.g. freeweights/qwen2.5-0.5b-instruct", type: "string" })
      .option("dataset", { describe: "run: dataset file path (starts with / ~ or .) or Hugging Face repo", type: "string" })
      .option("method", { describe: "run: lora | qlora | full | dpo (default lora)", type: "string" })
      .option("follow", { describe: "logs: keep following until the job ends. run: follows by default, --no-follow prints the job id and returns at once", type: "boolean" })
      .option("after", { describe: "logs: only events after this sequence number", type: "number", default: 0 })
      .option("register", { describe: "outputs: register each output as a local model", type: "boolean", default: false }),
  handler: async (args) => {
    const asJson = Boolean(args.json)
    const action = args.action as string
    const target = args.target as string | undefined
    const needId = () => {
      if (!target) {
        UI.error(`Usage: gizzi studio ${action} <${action === "install" ? "engine" : "job-id"}>`)
        process.exit(1)
      }
      return target
    }
    await run(async (c) => {
      switch (action) {
        case "engines": {
          const engines = await c.engines()
          const rows = engines.map((e) => ({ ...e, fit: hostFit(e) }))
          if (asJson) return json(rows)
          table([
            ["ID", "LICENSE", "METHODS", "INSTALL", "HARDWARE"],
            ...rows.map((e) => [e.id, e.license, e.methods.join(","), e.install_state, e.fit.fits ? "fits" : `refused: ${e.fit.reason}`]),
          ])
          return
        }
        case "install": {
          const e = await c.install(needId())
          if (asJson) return json(e)
          out(`${e.id}: ${e.install_state}`)
          return
        }
        case "run": {
          let spec: unknown
          if (args.spec) {
            try { spec = JSON.parse(readFileSync(args.spec as string, "utf8")) } catch (e) {
              UI.error(`Cannot read spec ${args.spec}: ${(e as Error).message}`)
              return 1
            }
          } else {
            if (!args.engine || !args.base || !args.dataset) {
              UI.error("Usage: gizzi studio run --spec <file.json>  (or --engine <id> --base <repo> --dataset <path|repo> [--method lora])")
              return 1
            }
            spec = specFromFlags({ engine: args.engine as string, base: args.base as string, dataset: args.dataset as string, method: args.method as string | undefined })
          }
          const job = await c.createJob(spec)
          if (asJson && args.follow === false) { json(job); return 0 }
          out(asJson ? JSON.stringify({ id: job.id, state: job.state }) : `Job ${job.id} ${job.state}`)
          if (args.follow === false) return 0
          return follow(c, job.id, asJson)
        }
        case "jobs": {
          const jobs = await c.jobs()
          if (asJson) return json(jobs)
          if (!jobs.length) return void out("No jobs yet.")
          table([["ID", "STATE", "ENGINE", "METHOD", "CREATED"], ...jobs.map((j) => [j.id, j.state, String(j.spec?.engine ?? ""), String(j.spec?.method ?? ""), j.created_at])])
          return
        }
        case "logs": {
          const id = needId()
          if (args.follow === true) return follow(c, id, asJson, args.after as number)
          const page = await c.events(id, args.after as number)
          if (asJson) return json(page)
          for (const ev of page.events) out(fmtEvent(ev))
          return
        }
        case "cancel": {
          const j = await c.cancel(needId())
          if (asJson) return json(j)
          out(`Job ${j.id} ${j.state}`)
          return
        }
        case "outputs": {
          const id = needId()
          const outs = await c.outputs(id)
          const registered: Array<{ kind: string; model_id?: string }> = []
          if (args.register) for (const o of outs) registered.push({ kind: o.kind, ...(await c.register(id, o.kind)) })
          if (asJson) return json({ outputs: outs, ...(args.register ? { registered } : {}) })
          if (!outs.length) return void out("No outputs yet.")
          table([["KIND", "BYTES", "PATH"], ...outs.map((o) => [o.kind, String(o.bytes ?? ""), o.path])])
          for (const r of registered) out(`registered ${r.kind} as model ${r.model_id ?? "?"}`)
          return
        }
      }
    }, asJson)
  },
})
