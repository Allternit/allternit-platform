/**
 * `gizzi agents` — the Factory's agents part (SPEC §7, OpenRig "Rig").
 *
 * Engine verbs forward to `allternit-factory agents …`. Folded in from
 * Gizzi's older commands (implementations unchanged, registration moved):
 *   modes      Gizzi's agent modes (was `gizzi agent`)
 *   templates  agent templates (was `gizzi agent-hub`)
 *   harness    engine `list|sync` + Gizzi's local runtime registry (was `gizzi runtime`)
 *   bot        hosted bot profiles, chats, threads, routines (was `gizzi bot`)
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { engineGroup, engineVerb } from "@/cli/factory/forward"
import { AgentModesCommand } from "@/cli/commands/agent"
import { AgentHubCommand } from "@/cli/commands/agent-hub"
import { RuntimeLocalCommands } from "@/cli/commands/runtime"
import { BotCommand } from "@/cli/commands/bot"

const P = "agents" as const

export const AgentsCommand = cmd({
  command: "agents",
  describe: "Factory agents: teams, bots of every binding, the live wall",
  builder: (yargs: Argv) =>
    yargs
      .command(
        engineVerb(P, {
          command: "up [team]",
          describe: "bring a team up (spawn and bind its bots)",
          mutation: true,
          options: {
            preset: { type: "string", describe: "team preset" },
            on: { type: "string", describe: "computer to run on" },
          },
        }),
      )
      .command(engineVerb(P, { command: "ps", describe: "every bot of every binding: state, node, proof" }))
      .command(engineVerb(P, { command: "down [team]", describe: "stop a team's bots", mutation: true }))
      .command(engineVerb(P, { command: "whoami", describe: "which bot and team this terminal is" }))
      .command(
        engineVerb(P, {
          command: "recover",
          describe: "find dead or orphaned panes and plan a handover",
          mutation: true,
          options: { apply: { type: "boolean", describe: "apply the recovery plan" } },
        }),
      )
      .command(engineVerb(P, { command: "snapshot", describe: "snapshot the whole team", mutation: true }))
      .command(engineVerb(P, { command: "restore", describe: "restore a team snapshot", mutation: true }))
      .command(engineVerb(P, { command: "model <bot> <model>", describe: "set a bot's model", mutation: true }))
      .command(engineVerb(P, { command: "handoff <bot>", describe: "hand a bot's seat to a fresh session", mutation: true }))
      .command(
        engineGroup(
          P,
          "harness",
          "agent harnesses (claude, codex, kimi, grok, agy, gizzi) and Gizzi's local runtime registry",
          [
            { command: "list", describe: "harnesses the engine can drive" },
            { command: "sync", describe: "re-detect installed harnesses", mutation: true },
          ],
          RuntimeLocalCommands,
        ),
      )
      .command(engineVerb(P, { command: "pack", describe: "pack the team into a bundle", mutation: true }))
      .command(engineVerb(P, { command: "install <source>", describe: "install a team bundle (path or GitHub URL)", mutation: true }))
      .command(AgentHubCommand)
      .command(engineVerb(P, { command: "wall [team]", describe: "open the live wall of agent terminals", interactive: "always" }))
      .command(engineVerb(P, { command: "attach <bot>", describe: "drop into a Terminal bot's pane (bot@team)", interactive: "always" }))
      .command(engineVerb(P, { command: "doctor", describe: "check panes, harnesses and the registry against reality" }))
      .command(AgentModesCommand)
      .command(BotCommand)
      .demandCommand(1, "Specify an agents command (see gizzi agents --help)"),
  handler: () => {},
})
