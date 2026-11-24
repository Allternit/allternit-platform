import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { bootstrap } from "@/cli/bootstrap"
import { listTeleport, runTeleport } from "@/runtime/teleport/run"
import type { LeftOut } from "@/runtime/teleport/bundle"

function printLeftOut(leftOut: LeftOut[] | undefined) {
  if (!leftOut?.length) return
  UI.println(`Left out ${leftOut.length} file(s) that look like secrets (never sent):`)
  for (const f of leftOut) UI.println(`  ${f.path}  (${f.reason})`)
}
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
      .option("base", { describe: "with --to local: the commit the session left this Mac on", type: "string" })
      .option("dry-run", { describe: "with --to cloud: list the files that would travel and the ones left out as secrets, then stop", type: "boolean", default: false }),
  handler: async (args) => {
    const harness = args.harness as TeleportHarness
    await bootstrap(process.cwd(), async () => {
      try {
        if (args["dry-run"]) {
          if (args.to !== "cloud") throw new Error("--dry-run lists what a move to the cloud would send; use it with --to cloud.")
          const listing = await listTeleport({ session: args.session as string })
          UI.println(`Workspace ${listing.cwd}, base ${listing.base.slice(0, 12)}. Would send the session file plus:`)
          for (const f of listing.changed) UI.println(`  changed    ${f}`)
          for (const f of listing.untracked) UI.println(`  untracked  ${f}`)
          if (listing.changed.length + listing.untracked.length === 0) UI.println("  (no changes beyond the base commit)")
          printLeftOut(listing.leftOut)
          return
        }
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
        printLeftOut(result.leftOut)
      } catch (e) {
        UI.error(e instanceof Error ? e.message : String(e))
        process.exit(1)
      }
    })
  },
})
