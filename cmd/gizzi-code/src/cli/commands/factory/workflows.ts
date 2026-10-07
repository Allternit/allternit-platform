/**
 * `gizzi workflows` — the Factory's workflows part (SPEC §7, OpenRig
 * "Workflows").
 *
 * Engine verbs forward to `allternit-factory workflows …`. Folded in:
 *   wake jobs …   Gizzi's local scheduled jobs and bot routines (was `gizzi cron`)
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { engineGroup, engineVerb } from "@/cli/factory/forward"
import { CronCommand } from "@/cli/commands/cron"

const P = "workflows" as const

export const WorkflowsCommand = cmd({
  command: "workflows",
  describe: "Factory workflows: run templates, drive DAGs, Wake, gates",
  builder: (yargs: Argv) =>
    yargs
      .command(
        engineVerb(P, {
          command: "run <template>",
          describe: "plan a DAG from a template and drive it",
          mutation: true,
          options: { param: { type: "array", describe: "template parameter k=v (repeatable)" } },
        }),
      )
      .command(
        engineVerb(P, {
          command: "drive <dag>",
          describe: "drive a DAG in the foreground",
          mutation: true,
          interactive: "unless-json",
          options: { once: { type: "boolean", describe: "advance one step and stop" } },
        }),
      )
      .command(
        engineGroup(P, "template", "workflow templates", [
          { command: "list", describe: "list templates" },
          { command: "show <template>", describe: "show a template" },
          { command: "check <path>", describe: "validate a template" },
          {
            command: "save <path>",
            describe: "check a template file and save it to this workspace",
            mutation: true,
            options: {
              id: { type: "string", describe: "template id (default: the file name)" },
              force: { type: "boolean", describe: "replace a workspace template with the same id" },
            },
          },
        ]),
      )
      .command(
        engineGroup(
          P,
          "wake",
          "Wake: scheduled checks on Factory work, plus Gizzi's local jobs",
          [
            { command: "list", describe: "list Wake schedules" },
            { command: "due", describe: "what's due now" },
            { command: "run [id]", describe: "run due checks now", mutation: true },
          ],
          [CronCommand as never],
        ),
      )
      .command(
        engineGroup(P, "gate", "human wait-gates", [
          { command: "add", describe: "add a gate to a node", mutation: true },
          { command: "resolve <gate>", describe: "resolve a gate", mutation: true },
          { command: "list", describe: "list gates" },
        ]),
      )
      .command(engineVerb(P, { command: "status <run>", describe: "a run's DAG and node states" }))
      .demandCommand(1, "Specify a workflows command (see gizzi workflows --help)"),
  handler: () => {},
})
