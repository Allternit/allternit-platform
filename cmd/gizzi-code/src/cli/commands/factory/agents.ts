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
      .command(
        engineVerb(P, {
          command: "down <target>",
          describe: "stop a team's bots, or one bot's pane (bot@team)",
          mutation: true,
          options: { "rm-worktree": { type: "boolean", describe: "also remove the worktree it was started in" } },
        }),
      )
      .command(engineVerb(P, { command: "whoami", describe: "which bot and team this terminal is" }))
      .command(
        engineVerb(P, {
          command: "recover [bot]",
          describe: "plan (or --apply) restarts of dead, unfinished sessions you own, with the harness's resume",
          mutation: true,
          options: {
            apply: { type: "boolean", describe: "apply the recovery plan" },
            lead: { type: "string", describe: "act as this lead (default $ALLTERNIT_FACTORY_LEAD, else your user)" },
            "as-human": { type: "boolean", describe: "override the owning-lead check" },
          },
        }),
      )
      .command(engineVerb(P, { command: "snapshot", describe: "snapshot the whole team", mutation: true }))
      .command(engineVerb(P, { command: "restore", describe: "restore a team snapshot", mutation: true }))
      .command(
        engineVerb(P, {
          command: "model <bot> [model]",
          describe: "set a Terminal bot's model (bot@team); applies on its next start",
          mutation: true,
          options: {
            restart: { type: "boolean", describe: "relaunch a running bot now, resuming its conversation" },
            clear: { type: "boolean", describe: "drop the override and use team.yaml's model" },
          },
        }),
      )
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
