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
      .command(engineVerb(P, { command: "sessions", describe: "live agent sessions on this computer: address, status, what each is doing" }))
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
      .command(
        engineVerb(P, {
          command: "handoff <bot>",
          describe: "hand a Terminal bot's seat (bot@team) to a fresh session, with a note on where it left off",
          mutation: true,
          options: {
            note: { type: "string", describe: "a note for the fresh session" },
            lines: { type: "number", describe: "how many lines of the old pane to keep (default 200)" },
          },
        }),
      )
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
      .command(
        engineGroup(
          P,
          "tree",
          "Agent Trees: one leased worktree per agent, seeded from a warm base, checkpointed, landed and reaped",
          [
            {
              command: "new <branch>",
              describe: "make (or reuse) a tree: fresh origin/main, disk gate, lease, warm-base seed, its own env",
              options: {
                path: { type: "string", describe: "where to put it (default <repo>-<branch slug> next to the repo)" },
                owner: { type: "string", describe: "session:<id>, bot:<name> or human:<name>" },
                once: { type: "boolean", describe: "a one-off tree (ledger commit): source only, expires in an hour" },
                "source-only": { type: "boolean", describe: "skip dependency installs (code-only workers)" },
                plain: { type: "boolean", describe: "check out plainly instead of cloning the base" },
                "force-disk": { type: "boolean", describe: "ignore the free-disk gate" },
                offline: { type: "boolean", describe: "use the last fetched origin/main if the fetch fails" },
              },
            },
            {
              command: "list",
              describe: "every tree: owner, state, behind main, processes, overlaps, machine warnings",
              options: {
                all: { type: "boolean", describe: "include worktrees with no lease" },
                size: { type: "boolean", describe: "measure disk use (slow on big trees)" },
                path: { type: "string", describe: "only the tree containing this path" },
              },
            },
            { command: "env [tree]", describe: "print the tree's env: eval \"$(gizzi agents tree env)\"" },
            { command: "sync [tree]", describe: "forward-only update to origin/main" },
            {
              command: "checkpoint [tree]",
              describe: "save a checkpoint now (the CLI hooks do it after each edit)",
              options: { tool: { type: "string", describe: "what the checkpoint follows" } },
            },
            { command: "checkpoints [tree]", describe: "the tree's checkpoints, newest first" },
            {
              command: "rewind <n> [tree]",
              describe: "put the files back to checkpoint <n> (the current state is saved first)",
              options: { "as-owner": { type: "boolean", describe: "act on a tree you don't own" } },
            },
            {
              command: "pause [tree]",
              describe: "keep the tree for a later resume (pushes branch + checkpoints; never reaped)",
              options: { "as-owner": { type: "boolean", describe: "act on a tree you don't own" } },
            },
            {
              command: "resume [tree]",
              describe: "make a paused tree active again",
              options: { "as-owner": { type: "boolean", describe: "act on a tree you don't own" } },
            },
            {
              command: "done [tree]",
              describe: "land it: merge check, stop its processes, delete build output, branch and checkpoints, remove it",
              options: {
                abandon: { type: "boolean", describe: "not merged: push it to a salvage/ branch, then remove it" },
                "as-owner": { type: "boolean", describe: "act on a tree you don't own" },
              },
            },
            {
              command: "reap",
              describe: "merged + idle + clean rule over every worktree (a dry run unless --apply)",
              options: {
                apply: { type: "boolean", describe: "do it" },
                size: { type: "boolean", describe: "measure disk use" },
              },
            },
            { command: "overlap", describe: "files changed in more than one live tree" },
          ],
          [
            engineGroup(P, "base", "the warm base trees are cloned from", [
              { command: "status", describe: "where the base is, its commit, how far behind main" },
              {
                command: "refresh",
                describe: "make or fast-forward the base and install deps",
                options: { build: { type: "boolean", describe: "also run the repo's base build (through the build queue)" } },
              },
            ]),
            engineGroup(P, "reaper", "the scheduled reaper (launchd / systemd user timer)", [
              {
                command: "install",
                describe: "run `reap --apply` every few hours",
                options: { hours: { type: "number", describe: "how often (default 6)" } },
              },
              { command: "uninstall", describe: "remove the schedule" },
            ]),
          ],
        ),
      )
      .command(AgentModesCommand)
      .command(BotCommand)
      .demandCommand(1, "Specify an agents command (see gizzi agents --help)"),
  handler: () => {},
})
