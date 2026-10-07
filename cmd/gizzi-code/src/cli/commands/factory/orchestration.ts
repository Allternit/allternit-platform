/**
 * `gizzi orchestration` — the Factory's orchestration part (SPEC §7,
 * OpenRig "Coordination").
 *
 * Engine verbs forward to `allternit-factory orchestration …`. Folded in:
 *   mail send-external|email-status   the agent-email rail (rest of the old `gizzi mail`)
 *   ac …                              local channels/keys/git bundles (old `gizzi ac`;
 *                                     its send/read are `send` / `feed` now)
 * Hosted bots' own threads (the old `gizzi thread`) are under
 * `gizzi agents bot threads`.
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { engineGroup, engineVerb } from "@/cli/factory/forward"
import { OrchestrationThreadsCommand } from "@/cli/commands/thread"
import { platformRequest } from "@/runtime/bots/platform-api"
import { withLocalMailCommands } from "@/cli/commands/mail"
import { AcCommand } from "@/cli/commands/ac"

const P = "orchestration" as const

/**
 * `gizzi orchestration coordinate <project> "…"`: hand a request to the
 * project's Coordinator (Al), who plans it into threads for the project's
 * bots. The Coordinator lives with the account in allternit-api, so this
 * uses the person's own sign-in; the threads it opens show up in
 * `gizzi orchestration threads list` and on the project page.
 */
const CoordinateCommand = cmd({
  command: "coordinate <project> <text..>",
  describe: "Hand a request to the project's Coordinator, who plans it into threads",
  builder: (y: Argv) =>
    y
      .positional("project", { type: "string", demandOption: true, describe: "project id" })
      .positional("text", { type: "string", array: true, demandOption: true })
      .option("dry-run", { type: "boolean", default: false, describe: "print what would be sent and send nothing" })
      .option("json", { type: "boolean", default: false, describe: "print the Coordinator's reply as JSON" }),
  handler: async (args) => {
    const project = String(args.project)
    const text = (args.text as string[]).join(" ").trim()
    if (!text) {
      console.error("error: coordinate needs the request text")
      process.exit(64)
    }
    if (args["dry-run"]) {
      console.log(JSON.stringify({ dryRun: true, plan: { project, post: "the request to the project's Coordinator", text } }))
      return
    }
    try {
      const res = await platformRequest<{ message: unknown }>(
        "POST",
        `/api/v1/projects/${encodeURIComponent(project)}/messages`,
        { text },
      )
      if (args.json) {
        console.log(JSON.stringify(res))
        return
      }
      const m = res.message as { content?: string; text?: string } | string
      console.log(typeof m === "string" ? m : (m?.content ?? m?.text ?? JSON.stringify(m)))
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      if (args.json) console.log(JSON.stringify({ error: { code: /not found/i.test(msg) ? "not_found" : "transport", fact: msg, action: "Check the project id (gizzi workspace campaign list) and that you're signed in." } }))
      else console.error(`error: ${msg}`)
      process.exit(/not found/i.test(msg) ? 2 : 3)
    }
  },
})

export const OrchestrationCommand = cmd({
  command: "orchestration",
  describe: "Factory orchestration: send work, read panes, threads, mail, the feed",
  builder: (yargs: Argv) =>
    yargs
      .command(
        engineVerb(P, {
          command: "send <to> <text..>",
          describe: "send to a bot (bot@team): verified, or queued and says so",
          mutation: true,
          options: { queue: { type: "boolean", describe: "queue if the bot can't take it now" } },
        }),
      )
      .command(engineVerb(P, { command: "capture <bot> [lines]", describe: "the last lines of a bot's pane" }))
      .command(engineVerb(P, { command: "transcript <bot>", describe: "a bot's full transcript" }))
      .command(
        engineVerb(P, {
          command: "drain <bot>",
          describe: "deliver a bot's queued messages (oldest first, each once its paste is verified)",
          mutation: true,
          options: { all: { type: "boolean", describe: "every queued message, not just the oldest" } },
        }),
      )
      .command(OrchestrationThreadsCommand)
      .command({
        command: "mail",
        describe: "Factory mail (list, read, send, decide) and the agent-email rail",
        builder: (y: Argv) => {
          let b = y
          for (const v of [
            { command: "list", describe: "list mail threads" },
            { command: "read <thread>", describe: "read a mail thread" },
            { command: "send <thread> <body>", describe: "reply in a mail thread", mutation: true },
            { command: "decide <thread>", describe: "approve or reject a pending review", mutation: true },
          ])
            b = b.command(engineVerb(P, v))
          return withLocalMailCommands(b).demandCommand(1, "Specify a mail command")
        },
        handler: () => {},
      })
      .command(engineVerb(P, { command: "feed", describe: "the live event feed (one document with --json)", interactive: "unless-json" }))
      .command(
        engineGroup(P, "attention", "what needs a person", [
          { command: "list", describe: "list open attention items" },
          { command: "ack <id>", describe: "acknowledge an item", mutation: true },
        ]),
      )
      .command(
        engineGroup(P, "steer", "steer running work", [
          { command: "checkpoint", describe: "record a checkpoint", mutation: true },
          { command: "consult", describe: "ask for a consult", mutation: true },
        ]),
      )
      .command(CoordinateCommand)
      .command(AcCommand)
      .demandCommand(1, "Specify an orchestration command (see gizzi orchestration --help)"),
  handler: () => {},
})
