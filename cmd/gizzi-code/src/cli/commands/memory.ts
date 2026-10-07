import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { MemoryDrive } from "@/runtime/memory/drive/drive"
import { DrivePlatform } from "@/runtime/memory/drive/platform"
import { applyMemdirImport, legacyMemdirRoots, planMemdirImport } from "@/runtime/memory/drive/import"
import { formatDreams, formatFileView, formatHistory, formatOverview, formatSearch, syncLine } from "@/runtime/memory/drive/report"
import { PlatformApiError, PlatformSignedOutError } from "@/runtime/bots/platform-api"

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
 */

function fail(error: unknown): never {
  const message =
    error instanceof PlatformSignedOutError
      ? "Not signed in to Allternit. Run `gizzi login` first."
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
      .command(UndoDreamCommand),
  handler: async () => {},
})
