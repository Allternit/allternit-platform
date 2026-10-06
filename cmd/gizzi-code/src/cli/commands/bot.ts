/**
 * `gizzi agents bot` — Bot Mode command group (phases B1+B2+B3 — see
 * docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md).
 *
 * A Bot is a profile (D1): `~/.gizzi/bots/<name>/` holds `bot.json`
 * (identity), `SOUL.md` (persona), and `memory/` (bot-scoped notes). This
 * group manages those profiles, opens canonical chats (D2/D6), and manages
 * routines (B3/D3): cron jobs of type `agent` with `config.bot` set that
 * deliver their prompt into the bot's canonical chat, namespaced
 * `[bot:<name>] <label>`.
 */
import { readFile } from "node:fs/promises"
import { existsSync } from "node:fs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import * as prompts from "@clack/prompts"
import { bootstrap } from "@/cli/bootstrap"
import { RunCommand } from "@/cli/commands/run"
import { BotThreadsCommand } from "@/cli/commands/thread"
import { TuiThreadCommand } from "@/cli/ui/ink-app/thread"
import { describeSchedule } from "@/runtime/automation/cron/parser"
import {
  BotStoreError,
  botDir,
  cloneBot,
  createBot,
  deleteBot,
  getBot,
  listBotMemory,
  listBots,
  readSoul,
  updateBot,
  type Bot,
} from "@/runtime/bots/bot-store"
import {
  ensureCapabilityEpoch,
  openCanonicalChat,
} from "@/runtime/bots/canonical-chat"
import {
  addBotRoutine,
  botRoutineLabel,
  cronServiceDeps,
  listBotRoutines,
  removeBotRoutine,
} from "@/runtime/bots/bot-routines"
import { classifyFailure } from "@/runtime/bots/failure-reasons"
import { Bus } from "@/shared/bus"
import { Session } from "@/runtime/session"

async function guard(fn: () => Promise<void>): Promise<void> {
  try {
    await fn()
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e)
    UI.println(UI.Style.TEXT_ERROR + `❌ ${message}` + UI.Style.RESET)
    if (process.env.GIZZI_BOT_DEBUG) console.error(e)
    process.exit(1)
  }
}

function exitOk(): never {
  process.exit(0)
}

/* -------------------------------------------------------------------------- */
/* create                                                                     */
/* -------------------------------------------------------------------------- */

export const BotCreateCommand = cmd({
  command: "create <name>",
  describe: "Create a bot profile (bot.json + SOUL.md + memory/)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name (lowercase slug: [a-z0-9-])",
        type: "string",
        demandOption: true,
      })
      .option("title", {
        type: "string",
        describe: "display title for the bot",
      })
      .option("description", {
        type: "string",
        describe: "what the bot is for",
      })
      .option("model", {
        type: "string",
        describe: "model pin (provider/model), e.g. anthropic/claude-sonnet-4-5",
      })
      .option("soul", {
        type: "string",
        describe: "path to a SOUL.md file to seed the persona (default: starter template)",
      })
      .example("gizzi agents bot create research-buddy --title 'Research Buddy'", "create a bot")
      .example(
        "gizzi agents bot create reviewer --model anthropic/claude-sonnet-4-5 --soul ./SOUL.md",
        "with a model pin and custom persona",
      ),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      let soul: string | undefined
      const soulPath = args.soul as string | undefined
      if (soulPath) {
        if (!existsSync(soulPath)) {
          throw new BotStoreError(`--soul file not found: ${soulPath}`)
        }
        soul = await readFile(soulPath, "utf8")
      }
      const bot = await createBot({
        name,
        title: (args.title as string | undefined) ?? name,
        description: args.description as string | undefined,
        model: args.model as string | undefined,
        soul,
      })
      UI.println(
        UI.Style.TEXT_SUCCESS + `🤖 Bot '${bot.name}' created` + UI.Style.RESET,
      )
      UI.println(`  Home: ${botDir(bot.name)}`)
      UI.println("  Edit SOUL.md to give the bot a persona, then `gizzi agents bot chat " + bot.name + "`.")
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* list                                                                       */
/* -------------------------------------------------------------------------- */

export const BotListCommand = cmd({
  command: ["list", "ls"],
  describe: "List bot profiles",
  builder: (yargs) => yargs,
  handler: async () => {
    await guard(async () => {
      const bots = await listBots()
      if (bots.length === 0) {
        UI.println("No bots yet.")
        UI.println("Create one with: gizzi agents bot create <name>")
        exitOk()
      }
      const rows = bots.map((bot) => [
        bot.name,
        bot.title,
        bot.model ?? "—",
        bot.canonicalSession ? `pinned:${bot.canonicalSession.sessionId.slice(0, 8)}` : "—",
      ])
      const widths = [0, 1, 2].map((i) =>
        Math.max(...rows.map((r) => r[i].length), ["NAME", "TITLE", "MODEL"][i].length),
      )
      const line = (cols: string[]) =>
        cols.map((c, i) => c.padEnd(widths[i])).join("  ").trimEnd()
      UI.println(UI.Style.TEXT_INFO_BOLD + line(["NAME", "TITLE", "MODEL", "CHAT"]) + UI.Style.RESET)
      for (const row of rows) UI.println(line(row))
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* show                                                                       */
/* -------------------------------------------------------------------------- */

export const BotShowCommand = cmd({
  command: "show <name>",
  describe: "Show a bot's full identity, SOUL preview, and memory note count",
  builder: (yargs) =>
    yargs.positional("name", {
      describe: "bot name",
      type: "string",
      demandOption: true,
    }),
  handler: async (args) => {
    await guard(async () => {
      const bot = await getBot(args.name as string)
      if (!bot) {
        throw new BotStoreError(`bot '${args.name}' not found`)
      }
      await printBot(bot)
      exitOk()
    })
  },
})

async function printBot(bot: Bot): Promise<void> {
  const soul = await readSoul(bot.name)
  const memory = await listBotMemory(bot.name)
  UI.println(UI.Style.TEXT_INFO_BOLD + `Bot: ${bot.name}` + UI.Style.RESET)
  UI.println(`  Title:             ${bot.title}`)
  UI.println(`  Description:       ${bot.description || "—"}`)
  UI.println(`  Model:             ${bot.model ?? "—"}`)
  UI.println(`  Avatar:            ${bot.avatar ? bot.avatar.kind : "—"}`)
  UI.println(
    `  Canonical session: ${
      bot.canonicalSession
        ? `${bot.canonicalSession.sessionId} @ ${bot.canonicalSession.projectPath}`
        : "not pinned (lands in Phase B2)"
    }`,
  )
  UI.println(`  Capability epoch:  ${bot.capabilityEpoch ?? "—"}`)
  UI.println(`  Created:           ${bot.createdAt}`)
  UI.println(`  Updated:           ${bot.updatedAt}`)
  UI.println(`  Home:              ${botDir(bot.name)}`)
  UI.println(`  Memory notes:      ${memory.length}`)
  UI.println(`  SOUL.md (${soul ? soul.split("\n").length : 0} lines):`)
  if (soul) {
    const preview = soul.split("\n").slice(0, 12)
    for (const lineText of preview) UI.println(`    ${lineText}`)
    if (soul.split("\n").length > preview.length) {
      UI.println("    …")
    }
  } else {
    UI.println("    (missing)")
  }
}

/* -------------------------------------------------------------------------- */
/* edit                                                                       */
/* -------------------------------------------------------------------------- */

export const BotEditCommand = cmd({
  command: "edit <name>",
  describe: "Edit a bot's identity metadata (title / description / model)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .option("title", {
        type: "string",
        describe: "new display title",
      })
      .option("description", {
        type: "string",
        describe: "new description",
      })
      .option("model", {
        type: "string",
        describe: "new model pin (provider/model); pass an empty string to clear",
      }),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      const hasPatch =
        args.title !== undefined ||
        args.description !== undefined ||
        args.model !== undefined
      if (!hasPatch) {
        throw new BotStoreError("nothing to update — pass at least one of --title, --description, --model")
      }
      const bot = await updateBot(name, {
        title: args.title as string | undefined,
        description: args.description as string | undefined,
        model: args.model !== undefined ? (args.model as string) || null : undefined,
      })
      UI.println(UI.Style.TEXT_SUCCESS + `✓ Bot '${bot.name}' updated` + UI.Style.RESET)
      UI.println(`  Title: ${bot.title}`)
      UI.println(`  Model: ${bot.model ?? "—"}`)
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* clone                                                                      */
/* -------------------------------------------------------------------------- */

export const BotCloneCommand = cmd({
  command: "clone <source> <new-name>",
  describe: "Clone a bot (identity, SOUL.md, memory/) — never its canonical session",
  builder: (yargs) =>
    yargs
      .positional("source", {
        describe: "existing bot name",
        type: "string",
        demandOption: true,
      })
      .positional("new-name", {
        describe: "name for the clone (lowercase slug)",
        type: "string",
        demandOption: true,
      }),
  handler: async (args) => {
    await guard(async () => {
      const clone = await cloneBot(args.source as string, args["new-name"] as string)
      const memoryCount = (await listBotMemory(clone.name)).length
      UI.println(
        UI.Style.TEXT_SUCCESS + `✓ Bot '${clone.name}' cloned from '${args.source}'` + UI.Style.RESET,
      )
      UI.println(`  Home: ${botDir(clone.name)}`)
      UI.println(`  Memory notes copied: ${memoryCount}`)
      UI.println("  Canonical session: not pinned (clones never inherit it)")
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* delete                                                                     */
/* -------------------------------------------------------------------------- */

export const BotDeleteCommand = cmd({
  command: ["delete <name>", "rm <name>"],
  describe: "Delete a bot profile (bot.json, SOUL.md, memory/)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .option("yes", {
        alias: "y",
        type: "boolean",
        describe: "skip the confirmation prompt",
        default: false,
      }),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      const bot = await getBot(name)
      if (!bot) {
        throw new BotStoreError(`bot '${name}' not found`)
      }
      if (!args.yes) {
        if (!process.stdin.isTTY) {
          // clack prompts never resolve without a TTY — refuse instead of hanging.
          throw new BotStoreError(
            `refusing to prompt in a non-interactive shell — re-run with --yes to delete '${bot.name}'`,
          )
        }
        const confirmed = await prompts.confirm({
          message: `Delete bot '${bot.name}' (${bot.title}) and everything under ${botDir(bot.name)}?`,
          initialValue: false,
        })
        if (prompts.isCancel(confirmed) || !confirmed) {
          UI.println("Cancelled")
          exitOk()
        }
      }
      const removed = await deleteBot(bot.name)
      UI.println(
        UI.Style.TEXT_SUCCESS +
          `✓ Deleted bot '${removed.name}' — removed ${botDir(removed.name)} (bot.json, SOUL.md, memory/)` +
          UI.Style.RESET,
      )
      exitOk()
    })
  },
})

/* -------------------------------------------------------------------------- */
/* chat (B2 — canonical bot chat, D2/D6)                                      */
/* -------------------------------------------------------------------------- */

export const BotChatCommand = cmd({
  command: "chat <name> [message..]",
  describe: "Chat with a bot in its canonical session (created + pinned on first use)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .positional("message", {
        describe: "message for the bot (omitted = interactive TUI)",
        type: "string",
        array: true,
      })
      .example("gizzi agents bot chat research-buddy", "open the bot's canonical chat in the TUI")
      .example("gizzi agents bot chat research-buddy 'summarize today's notes'", "one headless turn"),
  handler: async (args) => {
    await guard(async () => {
      const name = args.name as string
      const message = (args.message as string[] | undefined)?.join(" ")?.trim()
      const bot = await getBot(name)
      if (!bot) {
        throw new BotStoreError(`bot '${args.name}' not found`)
      }
      if (!message && !process.stdin.isTTY) {
        throw new BotStoreError(
          "no message given and stdin is not a terminal — usage: gizzi agents bot chat <name> [message..]",
        )
      }

      // Sessions belong under the project the bot was pinned in; the cwd is
      // the default project for a never-pinned bot.
      const projectPath = bot.canonicalSession?.projectPath ?? process.cwd()
      const opened = await bootstrap(projectPath, () => openCanonicalChat(bot, { projectPath }))
      const pinned = await getBot(bot.name)
      if (pinned) await ensureCapabilityEpoch(pinned)

      if (opened.created) {
        UI.println(
          UI.Style.TEXT_SUCCESS +
            `🤖 Canonical chat for '${bot.name}' created and pinned (session ${opened.sessionId}).` +
            UI.Style.RESET,
        )
      } else {
        UI.println(
          UI.Style.TEXT_DIM +
            `🤖 Resuming '${bot.name}' — canonical session ${opened.sessionId} @ ${opened.projectPath}` +
            UI.Style.RESET,
        )
      }

      if (!message) {
        // Interactive: the default command's -s/--session path. TuiThreadCommand
        // ends with process.exit(0) when the TUI closes.
        await (TuiThreadCommand.handler as any)?.({
          _: ["gizzi"],
          project: opened.projectPath,
          session: opened.sessionId,
          model: bot.model ?? undefined,
          "$0": "gizzi",
        })
        return
      }

      // Headless single turn into the canonical session, preserving continuity
      // (`gizzi run -s <id> --print` semantics). RunCommand self-exits in print
      // mode; UI banners above go to stderr so stdout stays pipe-clean.
      //
      // Typed failure taxonomy (B4/D4): print `[reason: <code>]` ahead of the
      // error text (Hermes format). Model errors surface as session.error bus
      // events — bus subscribers run synchronously at publish, before run.ts
      // renders the same event over SSE, so the reason line lands first.
      // Errors that reject the handler (bootstrap/prompt failures) are caught
      // and annotated below.
      let reasonPrinted = false
      let unsubscribe: () => void = () => {}
      try {
        // The failure-reason subscription and the headless run must both run
        // inside an Instance context: subscribing replays queued bus events,
        // and Session state (touched by subscribers) initializes lazily via
        // Instance.directory — outside a bootstrap() that throws
        // "No context found for instance" (B4 headless path).
        await bootstrap(projectPath, async () => {
          unsubscribe = Bus.subscribe(Session.Event.Error, (event) => {
            if (event.properties.sessionID !== opened.sessionId || reasonPrinted) return
            reasonPrinted = true
            UI.error(`[reason: ${classifyFailure(event.properties.error)}]`)
          })
          await (RunCommand.handler as any)?.({
            _: ["run"],
            message: [message],
            session: opened.sessionId,
            model: bot.model ?? undefined,
            print: true,
            outputFormat: "text",
            permissionMode: "dontAsk",
            "$0": "gizzi",
          })
        })
      } catch (e) {
        if (!reasonPrinted) {
          UI.error(`[reason: ${classifyFailure(e)}]`)
          reasonPrinted = true
        }
        throw e
      } finally {
        unsubscribe()
      }
    })
  },
})

/* -------------------------------------------------------------------------- */
/* routine (B3 — cron routines delivered into the canonical chat, D3)         */
/* -------------------------------------------------------------------------- */

export const BotRoutineAddCommand = cmd({
  command: "add <name>",
  describe: "Add a cron routine — delivers a prompt into the bot's canonical chat on schedule",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .option("label", {
        type: "string",
        describe: "routine label (default: the prompt text)",
      })
      .option("schedule", {
        type: "string",
        demandOption: true,
        describe: "cron expression or interval (e.g. 'daily at 9am', '*/15 * * * *')",
      })
      .option("prompt", {
        type: "string",
        demandOption: true,
        describe: "text delivered to the bot's canonical chat on each run",
      })
      .example(
        "gizzi agents bot routine add research-buddy --schedule 'daily at 9am' --prompt 'summarize new papers'",
        "daily routine for a bot",
      )
      .example(
        "gizzi agents bot routine add research-buddy --label 'paper sweep' --schedule '0 */6 * * *' --prompt '...'",
        "named routine on a cron expression",
      ),
  handler: async (args) => {
    await guard(async () => {
      const job = await addBotRoutine(args.name as string, {
        label: args.label as string | undefined,
        schedule: args.schedule as string,
        prompt: args.prompt as string,
      })
      UI.println(
        UI.Style.TEXT_SUCCESS +
          `✓ Routine added for bot '${args.name as string}'` +
          UI.Style.RESET,
      )
      UI.println(`  Job:      ${job.name} [${job.id}]`)
      UI.println(`  Schedule: ${describeSchedule(job.schedule)}`)
      UI.println(`  Bot:      ${(job.config as { bot?: string }).bot ?? "—"}`)
      UI.println("  Each run delivers the prompt into the bot's canonical chat.")
      exitOk()
    })
  },
})

export const BotRoutineListCommand = cmd({
  command: ["list <name>", "ls <name>"],
  describe: "List a bot's routines (schedule, status, next run)",
  builder: (yargs) =>
    yargs.positional("name", {
      describe: "bot name",
      type: "string",
      demandOption: true,
    }),
  handler: async (args) => {
    await guard(async () => {
      const jobs = await listBotRoutines(args.name as string)
      if (jobs.length === 0) {
        UI.println(`No routines for bot '${args.name as string}'.`)
        UI.println(
          "Add one with: gizzi agents bot routine add " + (args.name as string) + " --schedule <cron> --prompt <text>",
        )
        exitOk()
      }
      const rows = jobs.map((job) => [
        botRoutineLabel(job.name),
        describeSchedule(job.schedule),
        job.status,
        job.nextRunAt ?? "—",
      ])
      const headers = ["LABEL", "SCHEDULE", "STATUS", "NEXT RUN"]
      const widths = [0, 1, 2, 3].map((i) =>
        Math.max(...rows.map((r) => r[i].length), headers[i].length),
      )
      const line = (cols: string[]) =>
        cols.map((c, i) => c.padEnd(widths[i])).join("  ").trimEnd()
      UI.println(UI.Style.TEXT_INFO_BOLD + line(headers) + UI.Style.RESET)
      for (const row of rows) UI.println(line(row))
      exitOk()
    })
  },
})

export const BotRoutineRemoveCommand = cmd({
  command: ["remove <name> <jobId>", "rm <name> <jobId>"],
  describe: "Remove one of a bot's routines (refuses jobs owned by other bots)",
  builder: (yargs) =>
    yargs
      .positional("name", {
        describe: "bot name",
        type: "string",
        demandOption: true,
      })
      .positional("jobId", {
        describe: "cron job id (see `gizzi agents bot routine list <name>`)",
        type: "string",
        demandOption: true,
      }),
  handler: async (args) => {
    await guard(async () => {
      const job = await removeBotRoutine(args.name as string, args.jobId as string)
      UI.println(
        UI.Style.TEXT_SUCCESS +
          `✓ Removed routine '${job.name}' [${job.id}] from bot '${args.name as string}'` +
          UI.Style.RESET,
      )
      exitOk()
    })
  },
})

export const BotRoutineCommand = cmd({
  command: "routine",
  describe: "Bot routines — cron jobs that deliver prompts into a bot's canonical chat (D3)",
  builder: (yargs) =>
    yargs
      .command(BotRoutineAddCommand)
      .command(BotRoutineListCommand)
      .command(BotRoutineRemoveCommand)
      .demandCommand(1),
  handler: async () => {},
})

/* -------------------------------------------------------------------------- */
/* group                                                                      */
/* -------------------------------------------------------------------------- */

export const BotCommand = cmd({
  command: "bot",
  describe: "hosted bots: profiles, chats, threads, routines (Bot Mode)",
  builder: (yargs) =>
    yargs
      .command(BotCreateCommand)
      .command(BotListCommand)
      .command(BotShowCommand)
      .command(BotEditCommand)
      .command(BotCloneCommand)
      .command(BotDeleteCommand)
      .command(BotChatCommand)
      .command(BotThreadsCommand)
      .command(BotRoutineCommand),
  handler: async () => {},
})
