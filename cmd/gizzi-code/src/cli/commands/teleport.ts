import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { bootstrap } from "@/cli/bootstrap"
import { runTeleport } from "@/runtime/teleport/run"
import type { TeleportHarness } from "@/runtime/teleport/teleport"

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
        const result = await runTeleport({
          session: args.session as string,
          to: args.to as "cloud" | "local",
          harness,
          targetId: args.target,
          model: args.model,
          effort: args.effort,
          base: args.base,
        })
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Teleported to ${result.direction === "cloud" ? "the cloud" : "this Mac"}.` + UI.Style.TEXT_NORMAL)
        UI.println(`Resume with: ${result.resume}`)
        if (result.conflicts) UI.println(`Some uncommitted work did not fit the cloud copy and was left as the cloud had it: ${result.conflicts.patch} patch conflict(s), ${result.conflicts.untracked} existing untracked file(s).`)
        if (result.patched) UI.println("Code changes from the cloud were applied to your working tree (staged).")
      } catch (e) {
        UI.error(e instanceof Error ? e.message : String(e))
        process.exit(1)
      }
    })
  },
})
