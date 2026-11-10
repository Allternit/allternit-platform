import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { bootstrap } from "@/cli/bootstrap/bootstrap"
import { buildSessionStatus } from "@/cli/ui/ink-app/utils/statusModel"
import { getDefaultMainLoopModelSetting, getRuntimeMainLoopModel } from "@/cli/ui/ink-app/utils/model/model"
import { getCwdState } from "@/cli/ui/ink-app/bootstrap/state"

export const StatusCommand = cmd({
  command: "status",
  describe: "show current session status",
  builder: (yargs: Argv) => {
    return yargs
      .option("json", {
        describe: "output status as JSON",
        type: "boolean",
        default: false,
      })
      .option("inline", {
        describe: "output status as plain text",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const defaultModel = getDefaultMainLoopModelSetting()
      const model = getRuntimeMainLoopModel({
        permissionMode: 'default',
        mainLoopModel: defaultModel,
        exceeds200kTokens: false,
      })

      const status = await buildSessionStatus({
        getAppState: () => ({
          messages: [],
          toolPermissionContext: { mode: 'default' },
        }),
        options: {
          mainLoopModel: model,
          mcpClients: [],
        },
      } as any)

      if (args.json) {
        process.stdout.write(JSON.stringify(status, null, 2) + "\n")
        process.exit(0)
      }

      if (args.inline) {
        process.stdout.write(`Model:     ${status.model}\n`)
        process.stdout.write(`Directory: ${status.directory}\n`)
        process.stdout.write(`Session:   ${status.sessionId}\n`)
        process.stdout.write(`Version:   ${status.version}\n`)
        process.stdout.write(`Harness:   ${status.harness.mode} (enabled: ${status.harness.enabled})\n`)
        process.stdout.write(`Context:   ${status.context.used ?? 0} / ${status.context.total}\n`)
        process.exit(0)
      }

      process.stdout.write(`gizzi status\n`)
      process.stdout.write(`  Model:     ${status.model}\n`)
      process.stdout.write(`  Directory: ${status.directory}\n`)
      process.stdout.write(`  Version:   ${status.version}\n`)
      process.stdout.write(`  Harness:   ${status.harness.mode} (enabled: ${status.harness.enabled})\n`)
      const pause = await import("@/cli/work/api").then((m) => m.getBotPause()).catch(() => undefined)
      if (pause?.paused) {
        const when = pause.since ? ` since ${new Date(pause.since * 1000).toLocaleString()}` : ""
        process.stdout.write(`  Bots:      paused${when}${pause.by ? ` (by ${pause.by})` : ""}\n`)
      }
      process.stdout.write(`\nRun with --json for machine-readable output.\n`)
      process.exit(0)
    })
  },
})
