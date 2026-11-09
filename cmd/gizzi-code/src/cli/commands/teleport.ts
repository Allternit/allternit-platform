import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { VendorSession } from "@/runtime/session/vendor-session"
import { bootstrap } from "@/cli/bootstrap"
import { Session } from "@/runtime/session"
import { platformRequest } from "@/runtime/bots/platform-api"
import { teleportToCloud, teleportToLocal, type TeleportHarness } from "@/runtime/teleport/teleport"

interface Place {
  id: string | null
  label: string
  ready: boolean
}

async function pickTarget(explicit?: string): Promise<string> {
  if (explicit) return explicit
  const { places } = await platformRequest<{ places: Place[] }>("GET", "/api/v1/placements")
  const cloud = places.filter((p) => p.id && p.ready)
  if (cloud.length === 0) throw new Error("No cloud computer is connected. Connect one in Allternit, then retry.")
  if (cloud.length > 1) {
    throw new Error(`More than one computer is connected; pass --target. Options: ${cloud.map((p) => `${p.id} (${p.label})`).join(", ")}`)
  }
  return cloud[0]!.id!
}

export const TeleportCommand = cmd({
  command: "teleport <session>",
  describe: "move a live Claude or Codex session between this Mac and your cloud computer",
  builder: (yargs: Argv) =>
    yargs
      .positional("session", { describe: "gizzi session ID, or the harness's own session ID", type: "string", demandOption: true })
      .option("to", { describe: "where the session should run", choices: ["cloud", "local"] as const, demandOption: true })
      .option("harness", { describe: "which CLI owns the conversation", choices: ["claude", "codex"] as const, default: "claude" })
      .option("target", { describe: "connected computer id (when you have more than one)", type: "string" })
      .option("model", { describe: "model the session runs on (defaults to the session's)", type: "string" })
      .option("effort", { describe: "reasoning effort", type: "string" })
      .option("base", { describe: "with --to local: the commit the session left this Mac on", type: "string" }),
  handler: async (args) => {
    const harness = args.harness as TeleportHarness
    await bootstrap(process.cwd(), async () => {
      try {
        let nativeId = args.session as string
        let model = args.model
        let cwd = process.cwd()
        const known = await Session.get(args.session as string).catch(() => undefined)
        if (known) {
          cwd = known.directory
          nativeId = (await VendorSession.get(known.id, harness)) ?? nativeId
          model ??= known.defaultModel?.modelID
        }
        if (!model) throw new Error("Pass --model: this session has no default model recorded.")
        const input = {
          sessionID: known?.id ?? (args.session as string),
          harness,
          nativeId,
          cwd,
          model,
          reasoning: args.effort,
          targetId: await pickTarget(args.target),
        }
        const result =
          args.to === "cloud"
            ? await teleportToCloud(input)
            : await teleportToLocal({ ...input, gitBase: args.base })
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Teleported to ${result.direction === "cloud" ? "the cloud" : "this Mac"}.` + UI.Style.TEXT_NORMAL)
        UI.println(`Resume with: ${result.resume}`)
        if (result.patched) UI.println("Code changes from the cloud were applied to your working tree (staged).")
      } catch (e) {
        UI.error(e instanceof Error ? e.message : String(e))
        process.exit(1)
      }
    })
  },
})
