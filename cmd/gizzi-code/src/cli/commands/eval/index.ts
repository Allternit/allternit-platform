// gizzi eval case ... (eval loop CONTRACT.md section 8). Handlers are plain functions so tests can call them.
import type { Argv } from "yargs"
import fs from "fs/promises"
import path from "path"
import { cmd } from "@/cli/commands/cmd"
import { bootstrap } from "@/cli/bootstrap"
import { EvalStore } from "@/runtime/eval/store"
import { redactForExport, scanWorkspaceTar, type TraceEntry } from "@/runtime/eval/redact"

type Out = (line: string) => void

/** CLI-only problems (bad flags, blocked export). Engine problems stay EvalStore.EvalError. */
export class EvalCliError extends Error {}
const stdout: Out = (line) => console.log(line)

export function needsCheckReason(label: "good" | "bad", agrees: boolean): string {
  if (agrees) return ""
  return label === "good"
    ? "the saved chat did not pass its own checks"
    : "the checks pass this chat, so add a check that says what went wrong"
}

export async function runSave(
  args: { sessionID: string; good?: boolean; bad?: boolean; note?: string; through?: string; suite?: string; json?: boolean },
  out: Out = stdout,
) {
  if (!!args.good === !!args.bad) throw new EvalCliError("Pick one: --good or --bad.")
  const saved = await EvalStore.saveCase({
    sessionID: args.sessionID,
    label: args.good ? "good" : "bad",
    note: args.note,
    throughMessageID: args.through,
    suite: args.suite,
    surface: "cli",
  })
  if (args.json) return out(JSON.stringify(saved, null, 2))
  out(`Saved test case ${saved.case.id} (${saved.case.suite})`)
  out(
    saved.self_test.agrees_with_label
      ? "Self-test: agrees"
      : `Needs a check: ${needsCheckReason(saved.case.label, false)}`,
  )
}

export async function runList(args: { session?: string; suite?: string; json?: boolean }, out: Out = stdout) {
  const cases = await EvalStore.listCases({ sessionID: args.session, suite: args.suite })
  if (args.json) return out(JSON.stringify({ cases }, null, 2))
  if (cases.length === 0) return out("No test cases saved yet.")
  const rows = [["ID", "LABEL", "TITLE", "LAST RESULT", "NEEDS CHECK"]].concat(
    cases.map((c) => [c.id, c.label, c.title, c.last_result ?? "-", c.needs_check ? "yes" : "no"]),
  )
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)))
  for (const r of rows) out(r.map((v, i) => v.padEnd(widths[i]!)).join("  ").trimEnd())
}

export async function runShow(args: { id: string; json?: boolean }, out: Out = stdout) {
  const c = await EvalStore.getCase(args.id)
  if (args.json) return out(JSON.stringify({ case: c }, null, 2))
  out(`${c.id} (${c.suite})`)
  out(`Title: ${c.title}`)
  out(`Label: ${c.label === "good" ? "went well" : "went wrong"}`)
  if (c.note) out(`Note: ${c.note}`)
  out(`Needs a check: ${c.needs_check ? "yes" : "no"}`)
  out(`Saved: ${new Date(c.origin.saved_at).toISOString()} from ${c.origin.surface}`)
  out(`Workspace: ${c.workspace.kind}`)
  out(`Turns: ${c.turns.length}`)
}

export async function runRemove(args: { id: string }, out: Out = stdout) {
  await EvalStore.deleteCase(args.id)
  out(`Removed test case ${args.id}`)
}

export async function runExport(args: { id: string; out: string }, out: Out = stdout) {
  const c = await EvalStore.getCase(args.id)
  const src = path.join(EvalStore.root(), "suites", c.suite, "cases", c.id)
  const tar = path.join(src, "workspace.tar.gz")
  const hasTar = await fs.stat(tar).then(() => true, () => false)
  if (hasTar) {
    const { hits } = await scanWorkspaceTar(tar)
    if (hits.length > 0) {
      const names = hits.map((h) => `${h.file} (${h.kind})`).join(", ")
      throw new EvalCliError(`Export stopped: ${hits.length} files look like secrets: ${names}`)
    }
  }
  const trace = JSON.parse(await fs.readFile(path.join(src, "trace.json"), "utf8")) as TraceEntry[]
  const red = redactForExport(trace, c.turns)
  const dest = path.resolve(args.out, c.id)
  await fs.mkdir(dest, { recursive: true })
  const caseFile = { ...c, turns: red.turns.map((t) => ({ role: "user" as const, text: t.text })) }
  await fs.writeFile(path.join(dest, "case.json"), JSON.stringify(caseFile, null, 2))
  await fs.writeFile(path.join(dest, "trace.json"), JSON.stringify(red.entries))
  if (hasTar) await fs.copyFile(tar, path.join(dest, "workspace.tar.gz"))
  await fs.writeFile(path.join(dest, "redaction_report.json"), JSON.stringify(red.report, null, 2))
  out(`Exported test case ${c.id} to ${dest}`)
}

// Errors print the plain message and exit 1.
function guarded<A>(fn: (args: A) => Promise<void>) {
  return async (args: A) => {
    try {
      await bootstrap(process.cwd(), () => fn(args))
    } catch (err) {
      console.error(err instanceof EvalStore.EvalError || err instanceof EvalCliError ? err.message : `Something went wrong: ${String(err)}`)
      process.exit(1)
    }
  }
}

const CaseSaveCommand = cmd({
  command: "save <sessionID>",
  describe: "save a chat as a test case",
  builder: (y: Argv) =>
    y
      .positional("sessionID", { type: "string", demandOption: true })
      .option("good", { type: "boolean", describe: "the chat went well" })
      .option("bad", { type: "boolean", describe: "the chat went wrong" })
      .option("note", { type: "string", describe: "what should have happened" })
      .option("through", { type: "string", describe: "keep the chat up to this message id" })
      .option("suite", { type: "string", describe: "suite name (default my-cases)" })
      .option("json", { type: "boolean", describe: "print the route response" }),
  handler: guarded((a: any) => runSave(a)),
})

const CaseListCommand = cmd({
  command: "list",
  describe: "list saved test cases",
  builder: (y: Argv) =>
    y
      .option("session", { type: "string" })
      .option("suite", { type: "string" })
      .option("json", { type: "boolean" }),
  handler: guarded((a: any) => runList(a)),
})

const CaseShowCommand = cmd({
  command: "show <id>",
  describe: "show one test case",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }).option("json", { type: "boolean" }),
  handler: guarded((a: any) => runShow(a)),
})

const CaseRmCommand = cmd({
  command: "rm <id>",
  describe: "delete a test case",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }),
  handler: guarded((a: any) => runRemove(a)),
})

const CaseExportCommand = cmd({
  command: "export <id>",
  describe: "copy a test case to a folder with strict redaction",
  builder: (y: Argv) =>
    y.positional("id", { type: "string", demandOption: true }).option("out", { type: "string", demandOption: true, describe: "folder to write into" }),
  handler: guarded((a: any) => runExport(a)),
})

const CaseCommand = cmd({
  command: "case",
  describe: "saved test cases",
  builder: (y: Argv) =>
    y.command(CaseSaveCommand).command(CaseListCommand).command(CaseShowCommand).command(CaseRmCommand).command(CaseExportCommand).demandCommand(),
  async handler() {},
})

export const EvalCommand = cmd({
  command: "eval",
  describe: "test cases and checks for Gizzi",
  builder: (y: Argv) => y.command(CaseCommand).demandCommand(),
  async handler() {},
})
