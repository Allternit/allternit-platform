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
          {
            command: "new [text]",
            describe: "plan work into nodes (prints the new dag_id and root node_id)",
            mutation: true,
            options: {
              template: { type: "string", describe: "a template id or path instead of [text]" },
              param: { type: "array", describe: "a template param <name>=<value> (repeatable)" },
            },
          },
          { command: "refine", describe: "refine a plan", mutation: true },
        ]),
      )
      .command(
        engineGroup(P, "node", "nodes (each with its own folder)", [
          {
            command: "add",
            describe: "add a node under a parent (--dag --parent --title)",
            mutation: true,
            options: {
              dag: { type: "string", describe: "the DAG (from `plan new`)" },
              parent: { type: "string", describe: "the parent node id" },
              title: { type: "string", describe: "the node's title" },
              kind: { type: "string", describe: "node kind (default: task)" },
              description: { type: "string", describe: "the node's prompt" },
              executor: { type: "string", describe: "bot:<slug> or ao:<harness> (what `drive` spawns)" },
            },
          },
          {
            command: "list",
            describe: "list open nodes (ready, claimed, waiting)",
            options: {
              dag: { type: "string", describe: "only this DAG" },
              ready: { type: "boolean", describe: "only nodes ready to claim" },
              mine: { type: "boolean", describe: "only nodes this bot owns" },
            },
          },
          {
            command: "claim <node>",
            describe: "claim a node (<node> or <dag>/<node>); defaults to you as the owner",
            mutation: true,
            options: {
              dag: { type: "string", describe: "the node's DAG, when the node id alone is ambiguous" },
              agent: { type: "string", describe: "who claims it (default: this bot, else you)" },
            },
          },
          {
            command: "handoff <node>",
            describe: "hand a claimed node (<dag>/<node>) to another agent, with a note",
            mutation: true,
            options: {
              to: { type: "string", describe: "the new owner (bot@team or an agent id)" },
              note: { type: "string", describe: "a note written into the node's PROGRESS.md" },
            },
          },
          {
            // The engine closes the claim (WIH), not the node id: `node show <dag> <node>` prints wih.id.
            command: "close <wih> <status> [evidence..]",
            describe: "close a claimed node by its WIH id (from `node show`), e.g. close wih_12 DONE <PR url>",
            mutation: true,
            options: {
              output: { type: "string", describe: "file whose text becomes the node output (counts as evidence)" },
              actor: { type: "string", describe: "who is closing: user:<id> or agent:<id> (unset = the worker)" },
            },
          },
        ]),
      )
      .command(engineVerb(P, { command: "approve <node>", describe: "approve what's waiting on you", mutation: true }))
      .command(
        engineGroup(P, "proof", "proof receipts", [
          {
            command: "add <node> <line> <file>",
            describe: "copy a file into the node's proof/ as evidence for one Proof contract line (text or 1-based number)",
            mutation: true,
            options: { dag: { type: "string", describe: "the node's DAG" } },
          },
          { command: "show <node>", describe: "show a node's proof", options: { dag: { type: "string", describe: "the node's DAG" } } },
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
