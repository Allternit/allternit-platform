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
import { withLocalMailCommands } from "@/cli/commands/mail"
import { AcCommand } from "@/cli/commands/ac"

const P = "orchestration" as const

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
        engineGroup(P, "threads", "Factory threads (standing / task) across every binding", [
          { command: "list", describe: "list threads" },
          { command: "show <thread>", describe: "show a thread" },
          { command: "new", describe: "open a thread", mutation: true },
        ]),
      )
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
      .command(engineVerb(P, { command: "coordinate <project> <text..>", describe: "hand a goal to the Coordinator", mutation: true }))
      .command(AcCommand)
      .demandCommand(1, "Specify an orchestration command (see gizzi orchestration --help)"),
  handler: () => {},
})
