import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { MemoryDrive } from "@/runtime/memory/drive/drive"
import { DrivePlatform } from "@/runtime/memory/drive/platform"
import { applyMemdirImport, legacyMemdirRoots, planMemdirImport } from "@/runtime/memory/drive/import"
import { formatDreams, formatFileView, formatHistory, formatOverview, formatSearch, syncLine } from "@/runtime/memory/drive/report"
import { sleepApi, sleepStatusLine, type SleepOutcome } from "@/runtime/memory/sleep/client"
import { PlatformApiError, PlatformSignedOutError, platformRequest } from "@/runtime/bots/platform-api"

/**
 * `gizzi memory` — the user's Memory Drive from the terminal.
 *
 *   gizzi memory [status]          checkout path, sync state, files, recent history
 *   gizzi memory sync              commit checkout edits, fetch, push pending changes
 *   gizzi memory log               recent commits
 *   gizzi memory view <path>       print one drive file
 *   gizzi memory search <words>    search every mounted drive
 *   gizzi memory import            dry run of the legacy memdir import (default)
 *   gizzi memory import --apply    run it (one commit, idempotent)
 *   gizzi memory dreams            server Dream history
 *   gizzi memory undo-dream <id>   revert one Dream (later edits are kept)
 *   gizzi memory list              twin memory + facts, newest first (--source, --json)
 *   gizzi memory edit <id> "text"  edit one twin memory or fact
 *   gizzi memory forget <id>       delete one twin memory or fact
 *   gizzi memory sleep [status|run|report|undo|on|off]   Allternit Sleep
 *   gizzi memory disputes [resolve <id> a|b|both_scoped|neither]
 *   gizzi memory pages [<topic>]   memory pages Sleep wrote
 */

function isConnectionError(error: unknown): boolean {
  const text = error instanceof Error ? `${error.message} ${(error as any).code ?? ""} ${(error as any).cause?.code ?? ""}` : ""
  return /ECONNREFUSED|ConnectionRefused|Unable to connect|fetch failed|ENOTFOUND|TimeoutError|timed out/i.test(text)
}

function fail(error: unknown): never {
  const message =
    error instanceof PlatformSignedOutError
      ? "Not signed in to Allternit. Run `gizzi login` first."
      : isConnectionError(error)
        ? "Cannot reach the Allternit runtime. Start Allternit Desktop (or the local allternit-api) and try again."
      : error instanceof PlatformApiError
        ? error.message
        : error instanceof Error
          ? error.message
          : String(error)
  UI.println(UI.Style.TEXT_ERROR + message + UI.Style.RESET)
  process.exit(1)
}

function requireDrive() {
  if (!MemoryDrive.enabled()) {
    UI.println("The Memory Drive is turned off (GIZZI_MEMORY_DRIVE=0).")
    process.exit(1)
  }
}

const driveOption = (yargs: any) =>
  yargs.option("drive", { type: "string", describe: "drive ref (default personal), e.g. project:<id>" })

const StatusCommand = cmd({
  command: ["status", "$0"],
  describe: "show the Memory Drive checkout, sync state, files and recent history",
  builder: (yargs) => driveOption(yargs),
  handler: async (args) => {
    requireDrive()
    try {
      await MemoryDrive.prepare()
      UI.println(formatOverview(await MemoryDrive.overview((args.drive as string) || "personal", 10)))
    } catch (error) {
      fail(error)
    }
  },
})

const SyncCommand = cmd({
  command: "sync",
  describe: "commit edits in the checkout, then fetch and push",
  builder: (yargs) =>
    driveOption(yargs).option("dismiss", { type: "boolean", describe: "hide the 'edits were not saved' notice" }),
  handler: async (args) => {
    requireDrive()
    const ref = (args.drive as string) || "personal"
    try {
      const committed = await MemoryDrive.commitWorkingTree({ ref, message: "Save memory edits" })
      if (committed.error) UI.println(UI.Style.TEXT_WARNING + committed.error + UI.Style.RESET)
      const status = await MemoryDrive.sync(ref)
      if (args.dismiss) await (await MemoryDrive.open(ref)).dismissRejected()
      const overview = await MemoryDrive.overview(ref, 0)
      UI.println(syncLine(status, overview.signedIn))
      if (status.pending || status.lastError) process.exit(1)
    } catch (error) {
      fail(error)
    }
  },
})

const LogCommand = cmd({
  command: "log",
  describe: "recent Memory Drive commits",
  builder: (yargs) => driveOption(yargs).option("limit", { type: "number", default: 25, alias: "n" }),
  handler: async (args) => {
    requireDrive()
    try {
      const checkout = await MemoryDrive.open((args.drive as string) || "personal")
      await checkout.ensure()
      UI.println(formatHistory(await checkout.history(args.limit as number)))
    } catch (error) {
      fail(error)
    }
  },
})

const ViewCommand = cmd({
  command: "view <path>",
  describe: "print one Memory Drive file",
  builder: (yargs) => driveOption(yargs).positional("path", { type: "string", demandOption: true }),
  handler: async (args) => {
    requireDrive()
    const rel = String(args.path)
    const target = rel.endsWith(".md") ? rel : `${rel}.md`
    try {
      const checkout = await MemoryDrive.open((args.drive as string) || "personal")
      await checkout.ensure()
      UI.println(formatFileView(target, await checkout.readFile(target)))
    } catch (error) {
      fail(error)
    }
  },
})

const SearchCommand = cmd({
  command: "search [words..]",
  describe: "search every mounted Memory Drive",
  builder: (yargs) => yargs.positional("words", { type: "string", array: true }),
  handler: async (args) => {
    requireDrive()
    const query = ((args.words as string[] | undefined) ?? []).join(" ")
    try {
      UI.println(formatSearch(query, await MemoryDrive.search(query, 100)))
    } catch (error) {
      fail(error)
    }
  },
})

const ImportCommand = cmd({
  command: "import",
  describe: "import legacy gizzi memdir files into the Memory Drive (dry run unless --apply)",
  builder: (yargs) =>
    yargs
      .option("dry-run", { type: "boolean", describe: "show the plan only (default)" })
      .option("apply", { type: "boolean", describe: "write the import as one commit" })
      .option("json", { type: "boolean", describe: "print the plan as JSON" }),
  handler: async (args) => {
    requireDrive()
    try {
      const drive = await MemoryDrive.open("personal")
      const roots = legacyMemdirRoots()
      if (!args.apply) {
        const plan = await planMemdirImport(drive, roots)
        if (args.json) return UI.println(JSON.stringify(plan, null, 2))
        UI.println(`Legacy memory found under: ${roots.join(", ")}`)
        if (plan.already_imported) UI.println("Already imported into this drive (imports/gizzi-memdir.md exists); --apply would do nothing.")
        UI.println(`${plan.total} file(s): ${plan.converted} to import, ${plan.skipped} skipped.`)
        for (const row of plan.rows) {
          UI.println(
            `  ${row.status === "convert" ? "import" : "skip  "}  ${row.project}/${row.file.split("/").pop()}` +
              (row.status === "convert" ? ` → ${row.topic} (${row.entries})` : ` — ${row.reason}`),
          )
        }
        if (plan.topic_files.length) UI.println(`Topic files: ${plan.topic_files.join(", ")}`)
        UI.println("Nothing was written. Run `gizzi memory import --apply` to import.")
        return
      }
      const { plan, applied, result } = await applyMemdirImport(drive, roots)
      if (!applied) {
        UI.println(plan.already_imported ? "Already imported; nothing to do." : "Nothing to import.")
        return
      }
      UI.println(`Imported ${plan.converted} file(s) into ${plan.topic_files.length} topic file(s) as one commit.`)
      if (result?.pending) UI.println(UI.Style.TEXT_WARNING + `Saved locally; not synced yet: ${result.error ?? "will retry"}` + UI.Style.RESET)
      UI.println("The original files were not changed.")
    } catch (error) {
      fail(error)
    }
  },
})

const DreamsCommand = cmd({
  command: "dreams",
  describe: "list the server's nightly Dreams for this drive",
  builder: (yargs) => driveOption(yargs).option("limit", { type: "number", default: 20, alias: "n" }),
  handler: async (args) => {
    try {
      UI.println(formatDreams(await MemoryDrive.dreams((args.drive as string) || "personal", args.limit as number)))
    } catch (error) {
      fail(error)
    }
  },
})

const UndoDreamCommand = cmd({
  command: "undo-dream <id>",
  describe: "revert one Dream (keeps edits made after it; conflicts are reported)",
  builder: (yargs) => driveOption(yargs).positional("id", { type: "string", demandOption: true }),
  handler: async (args) => {
    try {
      const result = await MemoryDrive.undoDream(String(args.id), (args.drive as string) || "personal")
      UI.println(`Dream undone${result.revision ? ` (revision ${result.revision.slice(0, 7)})` : ""}.`)
    } catch (error) {
      if (error instanceof PlatformApiError && error.status === 409) {
        UI.println(UI.Style.TEXT_ERROR + `Could not undo: ${error.message}. Later edits touch the same lines; fix them by hand or undo those first.` + UI.Style.RESET)
        process.exit(1)
      }
      fail(error)
    }
  },
})

const QuestionsCommand = cmd({
  command: "questions",
  describe: "show a shared drive's questions board (team, project or swarm)",
  builder: (yargs) => yargs.option("drive", { type: "string", demandOption: true, describe: "team:<id>, project:<id> or swarm:<id>" }),
  handler: async (args) => {
    try {
      const list = await DrivePlatform.questions(String(args.drive))
      if (!list.length) return UI.println("No questions yet.")
      for (const q of list) {
        UI.println(`${q.id}  [${q.status}]  ${q.text}  — ${q.author}, ${q.added}`)
        for (const a of q.answers) UI.println(`    ↳ ${a.text}  — ${a.author}, ${a.added}`)
      }
    } catch (error) {
      fail(error)
    }
  },
})

const AskCommand = cmd({
  command: "ask <text>",
  describe: "ask a question on a shared drive's questions board",
  builder: (yargs) => yargs.option("drive", { type: "string", demandOption: true }).positional("text", { type: "string", demandOption: true }),
  handler: async (args) => {
    try {
      const r = await DrivePlatform.ask(String(args.drive), String(args.text))
      UI.println(`Asked (${r.question.id}).`)
    } catch (error) {
      fail(error)
    }
  },
})

const AnswerCommand = cmd({
  command: "answer <id> [text]",
  describe: "answer a question; add --resolve to mark it resolved",
  builder: (yargs) =>
    yargs
      .option("drive", { type: "string", demandOption: true })
      .option("resolve", { type: "boolean", default: false })
      .positional("id", { type: "string", demandOption: true })
      .positional("text", { type: "string" }),
  handler: async (args) => {
    try {
      const drive = String(args.drive)
      if (args.text) await DrivePlatform.answer(drive, String(args.id), String(args.text))
      if (args.resolve) await DrivePlatform.resolve(drive, String(args.id))
      if (!args.text && !args.resolve) return UI.println("Give an answer, or --resolve.")
      UI.println(args.resolve ? "Done. The question is resolved." : "Answered.")
    } catch (error) {
      fail(error)
    }
  },
})

// --- runtime memory: twin memory + facts (local allternit-api) ---

type MemorySource = "twin" | "facts"
interface MemoryRow {
  id: string
  source: MemorySource
  text: string
  kind?: string
  createdAt: string
}

const LOOKUP_MS = 8_000

async function loadTwin(): Promise<MemoryRow[]> {
  const res = await platformRequest<{ memory?: any[] }>("GET", "/api/v1/twin/memory", undefined, { timeoutMs: LOOKUP_MS })
  return (res.memory ?? []).map((m) => ({
    id: String(m.id),
    source: "twin" as const,
    text: [m.subject, m.content].filter(Boolean).join(": "),
    kind: m.kind,
    createdAt: String(m.updatedAt ?? m.createdAt ?? ""),
  }))
}

async function loadFacts(): Promise<MemoryRow[]> {
  const res = await platformRequest<{ facts?: any[] }>("GET", "/api/v1/memory/v2/facts?limit=500", undefined, { timeoutMs: LOOKUP_MS })
  return (res.facts ?? []).map((f) => ({
    id: String(f.id),
    source: "facts" as const,
    text: String(f.fact ?? ""),
    kind: f.memory_type,
    createdAt: String(f.valid_from ?? ""),
  }))
}

const sourceOption = (yargs: any) =>
  yargs.option("source", { type: "string", choices: ["twin", "facts"], describe: "twin memory or facts (default: detect from the id)" })

/** Run the twin call, then the facts call, unless --source pins one. A 404 means "not in that store". */
async function bySource<T>(
  source: string | undefined,
  twin: () => Promise<T>,
  facts: () => Promise<T>,
  id: string,
): Promise<T> {
  if (source === "twin") return twin()
  if (source === "facts") return facts()
  try {
    return await twin()
  } catch (error) {
    if (!(error instanceof PlatformApiError) || (error.status !== 404 && error.status !== 400)) throw error
  }
  try {
    return await facts()
  } catch (error) {
    if (error instanceof PlatformApiError && error.status === 404) throw new Error(`No memory with id ${id} (checked twin memory and facts).`)
    throw error
  }
}

const ListCommand = cmd({
  command: "list",
  describe: "list twin memory and facts from the runtime, newest first",
  builder: (yargs) => sourceOption(yargs).option("json", { type: "boolean", describe: "print JSON" }),
  handler: async (args) => {
    try {
      const source = args.source as string | undefined
      const rows = [...(source === "facts" ? [] : await loadTwin()), ...(source === "twin" ? [] : await loadFacts())]
      rows.sort((a, b) => b.createdAt.localeCompare(a.createdAt))
      if (args.json) {
        UI.println(JSON.stringify(rows, null, 2))
        return
      }
      if (rows.length === 0) {
        UI.println("No memories.")
        return
      }
      const idWidth = Math.min(36, Math.max(2, ...rows.map((r) => r.id.length)))
      UI.println(`${"ID".padEnd(idWidth)}  ${"SOURCE".padEnd(6)}  TEXT`)
      for (const r of rows) {
        const flat = r.text.replace(/\s+/g, " ").trim()
        UI.println(`${r.id.padEnd(idWidth)}  ${r.source.padEnd(6)}  ${flat.length > 80 ? flat.slice(0, 79) + "…" : flat}`)
      }
    } catch (error) {
      fail(error)
    }
  },
})

const EditCommand = cmd({
  command: "edit <id> <text>",
  describe: "replace the text of one twin memory or fact",
  builder: (yargs) =>
    sourceOption(yargs)
      .positional("id", { type: "string", demandOption: true })
      .positional("text", { type: "string", demandOption: true }),
  handler: async (args) => {
    const id = args.id as string
    const text = args.text as string
    if (!text.trim()) fail(new Error("Text cannot be empty."))
    try {
      const enc = encodeURIComponent(id)
      const done = await bySource(
        args.source as string | undefined,
        async () => {
          await platformRequest("PATCH", `/api/v1/twin/memory/${enc}`, { content: text }, { timeoutMs: LOOKUP_MS })
          return `Updated twin memory ${id}.`
        },
        async () => {
          const res = await platformRequest<{ id?: string }>("PATCH", `/api/v1/memory/v2/facts/${enc}`, { fact: text }, { timeoutMs: LOOKUP_MS })
          return `Updated fact ${id}. New id: ${res?.id ?? "(unknown)"}`
        },
        id,
      )
      UI.println(done)
    } catch (error) {
      fail(error)
    }
  },
})

const ForgetCommand = cmd({
  command: "forget <id>",
  describe: "delete one twin memory or fact",
  builder: (yargs) => sourceOption(yargs).positional("id", { type: "string", demandOption: true }),
  handler: async (args) => {
    const id = args.id as string
    try {
      const enc = encodeURIComponent(id)
      const done = await bySource(
        args.source as string | undefined,
        async () => {
          await platformRequest("DELETE", `/api/v1/twin/memory/${enc}`, undefined, { timeoutMs: LOOKUP_MS })
          return `Forgot twin memory ${id}.`
        },
        async () => {
          await platformRequest("DELETE", `/api/v1/memory/v2/facts/${enc}`, undefined, { timeoutMs: LOOKUP_MS })
          return `Forgot fact ${id}.`
        },
        id,
      )
      UI.println(done)
    } catch (error) {
      fail(error)
    }
  },
})

/** Print a Sleep outcome; refusals and older runtimes exit 2, errors exit 1. */
function sleepResult(outcome: SleepOutcome, json: boolean, render: (body: any) => string): void {
  if (outcome.kind === "ok") {
    UI.println(json ? JSON.stringify(outcome.body, null, 2) : render(outcome.body))
    return
  }
  UI.println(outcome.message)
  process.exit(outcome.kind === "error" ? 1 : 2)
}

const jsonOption = (yargs: any) => yargs.option("json", { type: "boolean", describe: "print the route body as JSON" })

const cycleLine = (c: any) =>
  `${c.id}  ${c.kind}  ${c.state}${c.undone_at ? " (undone)" : ""}  $${Number(c.spend_usd ?? 0).toFixed(2)}  ${c.started_at}`

const SleepStatusCommand = cmd({
  command: ["status", "$0"],
  describe: "Sleep on/off, debt, last sleep, spend today",
  builder: (yargs) => jsonOption(yargs),
  handler: async (args) => {
    try {
      sleepResult(await sleepApi.status(), !!args.json, (s) => {
        const lines = [sleepStatusLine(s)]
        if (s.open_disputes > 0) lines.push(`${s.open_disputes} memory conflicts need you. Run \`gizzi memory disputes\`.`)
        return lines.join("\n")
      })
    } catch (error) {
      fail(error)
    }
  },
})

const SleepRunCommand = cmd({
  command: "run",
  describe: "start a sleep now",
  builder: (yargs) =>
    yargs.option("kind", { type: "string", choices: ["nap", "daydream"], describe: "which sleep to run" }),
  handler: async (args) => {
    try {
      sleepResult(await sleepApi.run(args.kind as string | undefined), false, (b) => `Started: ${JSON.stringify(b)}`)
    } catch (error) {
      fail(error)
    }
  },
})

const SleepReportCommand = cmd({
  command: "report [id]",
  describe: "what a sleep did (latest by default)",
  builder: (yargs) => jsonOption(yargs).positional("id", { type: "string" }),
  handler: async (args) => {
    try {
      let id = args.id as string | undefined
      if (!id) {
        const list = await sleepApi.cycles()
        if (list.kind !== "ok") return sleepResult(list, false, () => "")
        id = list.body.cycles?.[0]?.id
        if (!id) {
          UI.println("No sleeps yet.")
          return
        }
      }
      sleepResult(await sleepApi.cycle(id), !!args.json, (b) =>
        [cycleLine(b.cycle), b.report ? JSON.stringify(b.report, null, 2) : "No report."].join("\n"),
      )
    } catch (error) {
      fail(error)
    }
  },
})

const SleepUndoCommand = cmd({
  command: "undo <id>",
  describe: "undo one sleep",
  builder: (yargs) => yargs.positional("id", { type: "string", demandOption: true }),
  handler: async (args) => {
    try {
      sleepResult(await sleepApi.undo(String(args.id)), false, () => `Undid sleep ${args.id}.`)
    } catch (error) {
      fail(error)
    }
  },
})

const SleepOnCommand = cmd({
  command: "on",
  describe: "turn Sleep on",
  handler: async () => {
    try {
      sleepResult(await sleepApi.setEnabled(true), false, () => "Sleep is on.")
    } catch (error) {
      fail(error)
    }
  },
})

const SleepOffCommand = cmd({
  command: "off",
  describe: "turn Sleep off",
  handler: async () => {
    try {
      sleepResult(await sleepApi.setEnabled(false), false, () => "Sleep is off.")
    } catch (error) {
      fail(error)
    }
  },
})

const SleepCommand = cmd({
  command: "sleep",
  describe: "Allternit Sleep: status, run, report, undo, on, off",
  builder: (yargs) =>
    yargs
      .command(SleepStatusCommand)
      .command(SleepRunCommand)
      .command(SleepReportCommand)
      .command(SleepUndoCommand)
      .command(SleepOnCommand)
      .command(SleepOffCommand),
  handler: async () => {},
})

const DisputeResolveCommand = cmd({
  command: "resolve <id> <choice>",
  describe: "settle one conflict: a, b, both_scoped or neither",
  builder: (yargs) =>
    yargs
      .positional("id", { type: "string", demandOption: true })
      .positional("choice", { type: "string", choices: ["a", "b", "both_scoped", "neither"], demandOption: true }),
  handler: async (args) => {
    try {
      sleepResult(
        await sleepApi.resolve(String(args.id), String(args.choice)),
        false,
        () => `Resolved ${args.id}: ${args.choice}.`,
      )
    } catch (error) {
      fail(error)
    }
  },
})

const DisputesCommand = cmd({
  command: ["disputes", "conflicts"],
  describe: "memory conflicts Sleep found",
  builder: (yargs) => jsonOption(yargs).command(DisputeResolveCommand),
  handler: async (args) => {
    try {
      sleepResult(await sleepApi.disputes(), !!args.json, (b) => {
        const list: any[] = b.disputes ?? []
        if (!list.length) return "No memory conflicts."
        return list
          .map((d) => `${d.id}  ${d.state}  ${d.side_a?.kind}:${d.side_a?.id} vs ${d.side_b?.kind}:${d.side_b?.id}${d.scope ? `  [${d.scope}]` : ""}`)
          .join("\n")
      })
    } catch (error) {
      fail(error)
    }
  },
})

const PagesCommand = cmd({
  command: "pages [topic]",
  describe: "memory pages Sleep wrote",
  builder: (yargs) => jsonOption(yargs).positional("topic", { type: "string" }),
  handler: async (args) => {
    try {
      const topic = args.topic as string | undefined
      sleepResult(topic ? await sleepApi.page(topic) : await sleepApi.pages(), !!args.json, (b) => {
        if (topic) return JSON.stringify(b, null, 2)
        const list: any[] = b.pages ?? []
        if (!list.length) return "No pages yet."
        return list.map((p) => `${p.topic_id}  v${p.version}  ${p.tokens ?? "?"} tokens  ${p.created_at}`).join("\n")
      })
    } catch (error) {
      fail(error)
    }
  },
})

export const MemoryCommand = cmd({
  command: "memory",
  describe: "your Memory Drive — status, sync, import, Dreams",
  builder: (yargs) =>
    yargs
      .command(StatusCommand)
      .command(SyncCommand)
      .command(LogCommand)
      .command(ViewCommand)
      .command(SearchCommand)
      .command(ImportCommand)
      .command(QuestionsCommand)
      .command(AskCommand)
      .command(AnswerCommand)
      .command(DreamsCommand)
      .command(UndoDreamCommand)
      .command(ListCommand)
      .command(EditCommand)
      .command(ForgetCommand)
      .command(SleepCommand)
      .command(DisputesCommand)
      .command(PagesCommand),
  handler: async () => {},
})
