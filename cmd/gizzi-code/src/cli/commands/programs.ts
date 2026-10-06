/**
 * AllternitOS minimal equivalent for gizzi-code.
 *
 * The web surface has a full installable-program OS; the CLI has no windowing
 * kernel, but its agents, skills, and commands are the closest equivalent of
 * "programs". This command exposes them as an installable-program registry.
 *
 * Usage:
 *   gizzi programs list          # list available programs (agents/skills/commands)
 *   gizzi programs info <name>   # show program description
 *   gizzi programs run <name>    # run the matching command/agent
 */

import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { Instance } from "@/runtime/context/project/instance"
import { AgentManager } from "@/runtime/loop/manager"
import { EOL } from "os"

const PROGRAM_REGISTRY: Record<string, { description: string; command: string }> = {
  research: { description: "Research and analysis workspace", command: "gizzi research" },
  code: { description: "Code mode workspace", command: "gizzi code" },
  cowork: { description: "Cowork task runner", command: "gizzi workspace tasks" },
  tasks: { description: "Task queue and scheduling", command: "gizzi workspace tasks queue" },
  agents: { description: "Agent hub", command: "gizzi agents templates" },
  skills: { description: "Skill registry", command: "gizzi skills list" },
}

const ProgramsListCommand = cmd({
  command: "list",
  describe: "list available AllternitOS-equivalent programs",
  async handler() {
    await Instance.provide({
      directory: process.cwd(),
      async fn() {
        UI.println(UI.Style.TEXT_BOLD + "AllternitOS Programs" + UI.Style.TEXT_NORMAL)
        UI.println("")

        // Built-in command programs
        UI.println("Built-in command programs:")
        for (const [name, meta] of Object.entries(PROGRAM_REGISTRY)) {
          UI.println(`  ${UI.Style.TEXT_BOLD}${name}${UI.Style.TEXT_NORMAL}: ${meta.description}`)
          UI.println(`    run: ${UI.Style.TEXT_DIM}${meta.command}${UI.Style.TEXT_NORMAL}`)
        }

        // Agents as installable programs
        try {
          const agents = await AgentManager.list()
          if (agents.length > 0) {
            UI.println("")
            UI.println("Installed agents:")
            for (const agent of agents) {
              UI.println(`  ${UI.Style.TEXT_BOLD}${agent.name}${UI.Style.TEXT_NORMAL}: ${agent.description || agent.mode}`)
            }
          }
        } catch (error) {
          UI.println("")
          UI.println(UI.Style.TEXT_DIM + "Could not load installed agents." + UI.Style.TEXT_NORMAL)
        }
      },
    })
  },
})

const ProgramsInfoCommand = cmd({
  command: "info <name>",
  describe: "show details for a program",
  builder: (yargs: Argv) =>
    yargs.positional("name", {
      type: "string",
      describe: "program name",
      demandOption: true,
    }),
  async handler(args) {
    const name = args.name as string
    const builtIn = PROGRAM_REGISTRY[name]
    if (builtIn) {
      UI.println(`${UI.Style.TEXT_BOLD}${name}${UI.Style.TEXT_NORMAL}`)
      UI.println(`  ${builtIn.description}`)
      UI.println(`  Run: ${builtIn.command}`)
      return
    }

    await Instance.provide({
      directory: process.cwd(),
      async fn() {
        const agents = await AgentManager.list()
        const agent = agents.find((a) => a.name === name)
        if (agent) {
          UI.println(`${UI.Style.TEXT_BOLD}${agent.name}${UI.Style.TEXT_NORMAL}`)
          UI.println(`  ${agent.description || "No description"}`)
          UI.println(`  Mode: ${agent.mode}`)
          UI.println(`  Native: ${agent.native ? "yes" : "no"}`)
        } else {
          UI.println(UI.Style.TEXT_DANGER + `Unknown program: ${name}` + UI.Style.TEXT_NORMAL)
          process.exit(1)
        }
      },
    })
  },
})

const ProgramsRunCommand = cmd({
  command: "run <name>",
  describe: "run an AllternitOS-equivalent program",
  builder: (yargs: Argv) =>
    yargs.positional("name", {
      type: "string",
      describe: "program name",
      demandOption: true,
    }),
  async handler(args) {
    const name = args.name as string
    const builtIn = PROGRAM_REGISTRY[name]
    if (builtIn) {
      UI.println(`Launching ${name}...`)
      UI.println(`Run ${UI.Style.TEXT_DIM}${builtIn.command}${UI.Style.TEXT_NORMAL} to use this program.`)
      return
    }

    await Instance.provide({
      directory: process.cwd(),
      async fn() {
        const agents = await AgentManager.list()
        const agent = agents.find((a) => a.name === name)
        if (agent) {
          UI.println(`Launching agent program: ${agent.name}`)
          UI.println(`Use ${UI.Style.TEXT_DIM}gizzi agents modes select ${agent.name}${UI.Style.TEXT_NORMAL} then start a chat to run it.`)
        } else {
          UI.println(UI.Style.TEXT_DANGER + `Unknown program: ${name}` + UI.Style.TEXT_NORMAL)
          process.exit(1)
        }
      },
    })
  },
})

export const ProgramsCommand = cmd({
  command: "programs",
  describe: "AllternitOS-equivalent program registry",
  builder: (yargs: Argv) =>
    yargs
      .command(ProgramsListCommand)
      .command(ProgramsInfoCommand)
      .command(ProgramsRunCommand)
      .demandCommand(),
  async handler() {},
})
