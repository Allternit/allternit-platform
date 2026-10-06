/**
 * `gizzi agents modes` — Gizzi's own agent modes (build, plan, general,
 * explore, plus any defined in config or the workspace's .gizzi agents).
 *
 *   gizzi agents modes list            every mode, the default marked
 *   gizzi agents modes select <name>   make <name> the default mode
 *   gizzi agents modes status          the default mode and its settings
 *
 * Folded in from the old `gizzi agent`, whose list/status printed four
 * hardcoded names. Everything here reads the real agent registry.
 */

import { cmd } from "@/cli/commands/cmd"
import { bootstrap } from "@/cli/bootstrap"
import { UI } from "@/cli/ui"
import { Agent } from "@/runtime/loop/agent"
import { AgentManager } from "@/runtime/loop/manager"

type ModeRow = { name: string; mode: string; hidden: boolean; native: boolean; description: string; isDefault: boolean }

export function toModeRows(agents: Agent.Info[], defaultName: string | null): ModeRow[] {
  return agents.map((a) => ({
    name: a.name,
    mode: a.mode,
    hidden: a.hidden === true,
    native: a.native === true,
    description: (a.description ?? "").split("\n")[0] ?? "",
    isDefault: a.name === defaultName,
  }))
}

async function defaultModeName(): Promise<string | null> {
  try {
    return await Agent.defaultAgent()
  } catch {
    return null
  }
}

const ModesListCommand = cmd({
  command: "list",
  describe: "list Gizzi's agent modes (built-in, config and workspace)",
  builder: (y) =>
    y
      .option("all", { type: "boolean", default: false, describe: "include hidden modes" })
      .option("json", { type: "boolean", default: false, describe: "print JSON" }),
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const rows = toModeRows(await AgentManager.list(process.cwd()), await defaultModeName()).filter((r) => args.all || !r.hidden)
      if (args.json) {
        process.stdout.write(JSON.stringify({ modes: rows }, null, 2) + "\n")
        return
      }
      if (rows.length === 0) {
        UI.println("No agent modes are defined.")
        return
      }
      const width = Math.max(...rows.map((r) => r.name.length))
      for (const r of rows) {
        const mark = r.isDefault ? UI.Style.TEXT_SUCCESS_BOLD + "●" + UI.Style.TEXT_NORMAL : " "
        const kind = UI.Style.TEXT_DIM + r.mode.padEnd(8) + UI.Style.TEXT_NORMAL
        const desc = r.description.length > 72 ? r.description.slice(0, 71) + "…" : r.description
        UI.println(`${mark} ${r.name.padEnd(width)}  ${kind}  ${desc}`)
      }
      UI.println("")
      UI.println(UI.Style.TEXT_DIM + "● default · change it with `gizzi agents modes select <name>`" + UI.Style.TEXT_NORMAL)
    })
  },
})

const ModesSelectCommand = cmd({
  command: "select <name>",
  describe: "make an agent mode the default for new sessions",
  builder: (y) => y.positional("name", { type: "string", describe: "mode name", demandOption: true }),
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await AgentManager.setDefault(args.name as string)
      } catch (err) {
        UI.error((err as Error).message)
        process.exitCode = 2
        return
      }
      UI.success(`Default agent mode is now ${args.name}`)
    })
  },
})

const ModesStatusCommand = cmd({
  command: "status",
  describe: "show the default agent mode",
  builder: (y) => y.option("json", { type: "boolean", default: false, describe: "print JSON" }),
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const name = await defaultModeName()
      const info = name ? await Agent.get(name) : undefined
      if (args.json) {
        process.stdout.write(
          JSON.stringify(
            { default: name, mode: info?.mode ?? null, model: info?.model ?? null, description: info?.description ?? null },
            null,
            2,
          ) + "\n",
        )
        return
      }
      if (!name || !info) {
        UI.error("No usable default agent mode — check `default_agent` in your gizzi config.")
        process.exitCode = 2
        return
      }
      UI.println(`Default mode: ${UI.Style.TEXT_NORMAL_BOLD}${name}${UI.Style.TEXT_NORMAL} (${info.mode})`)
      if (info.model) UI.println(`Model: ${info.model.providerID}/${info.model.modelID}`)
      if (info.description) UI.println(UI.Style.TEXT_DIM + info.description.split("\n")[0] + UI.Style.TEXT_NORMAL)
    })
  },
})

export const AgentModesCommand = cmd({
  command: "modes",
  describe: "Gizzi's agent modes: list, select, status",
  builder: (yargs) =>
    yargs
      .command(ModesListCommand)
      .command(ModesSelectCommand)
      .command(ModesStatusCommand)
      .demandCommand(1, "Specify a modes command: list | select | status"),
  handler: async () => {},
})
