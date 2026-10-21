/**
 * `gizzi workspace` — the Factory's workspace part (SPEC §7, OpenRig
 * "Workspace").
 *
 * Engine verbs forward to `allternit-factory workspace …`. Folded in:
 *   tasks …   the task queue (was `gizzi cowork`)
 *   team …    a shared team workspace's board and skills (was `gizzi cowork-team`)
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { engineGroup, engineVerb } from "@/cli/factory/forward"
import { CoworkCommand } from "@/cli/commands/cowork"
import { CoworkTeamCommand } from "@/cli/commands/cowork-team"

const P = "workspace" as const

export const WorkspaceCommand = cmd({
  command: "workspace",
  describe: "Factory workspace: campaigns, nodes, proof, approvals, the board",
  builder: (yargs: Argv) =>
    yargs
      .command(
        engineGroup(P, "campaign", "campaigns", [
          { command: "new", describe: "start a campaign", mutation: true },
          { command: "list", describe: "list campaigns" },
          { command: "status [campaign]", describe: "a campaign's progress" },
          { command: "pause <campaign>", describe: "pause a campaign", mutation: true },
          { command: "finish <campaign>", describe: "finish a campaign", mutation: true },
        ]),
      )
      .command(
        engineGroup(P, "plan", "plans", [
          { command: "new", describe: "plan work into nodes", mutation: true },
          { command: "refine", describe: "refine a plan", mutation: true },
        ]),
      )
      .command(
        engineGroup(P, "node", "nodes (each with its own folder)", [
          { command: "add", describe: "add a node", mutation: true },
          {
            command: "list",
            describe: "list nodes",
            options: { mine: { type: "boolean", describe: "only nodes this bot owns" } },
          },
          { command: "claim <node>", describe: "claim a node", mutation: true },
          {
            command: "handoff <node>",
            describe: "hand a claimed node (<dag>/<node>) to another agent, with a note",
            mutation: true,
            options: {
              to: { type: "string", describe: "the new owner (bot@team or an agent id)" },
              note: { type: "string", describe: "a note written into the node's PROGRESS.md" },
            },
          },
          { command: "close <node>", describe: "close a node", mutation: true },
        ]),
      )
      .command(engineVerb(P, { command: "approve <node>", describe: "approve what's waiting on you", mutation: true }))
      .command(
        engineGroup(P, "proof", "proof receipts", [
          { command: "add <node>", describe: "attach proof to a node", mutation: true },
          { command: "show <node>", describe: "show a node's proof" },
        ]),
      )
      .command(
        engineGroup(P, "judge", "the judge's verdicts", [
          { command: "show <node>", describe: "show a verdict" },
          { command: "resolve <node>", describe: "resolve a needs-human verdict", mutation: true },
        ]),
      )
      .command(engineVerb(P, { command: "board [campaign]", describe: "the board: needs you, now, next, proof" }))
      .command(CoworkCommand)
      .command(CoworkTeamCommand)
      .demandCommand(1, "Specify a workspace command (see gizzi workspace --help)"),
  handler: () => {},
})
