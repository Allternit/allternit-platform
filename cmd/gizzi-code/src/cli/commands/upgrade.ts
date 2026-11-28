import type { Argv } from "yargs"
import { UI } from "@/cli/ui"
import * as prompts from "@clack/prompts"
import { Installation } from "@/shared/installation"
import { ReleaseFeed } from "@/shared/installation/feed"
import { Updater } from "@/shared/installation/updater"

export type UpdatePlan =
  | { kind: "managed"; method: Installation.Method; line: string }
  | { kind: "command"; method: Installation.Method; line: string; command: string }
  | { kind: "apply"; method: Installation.Method; line: string }

/**
 * What `gizzi update` does for an install method (PLAN.md D2). Pure, so tests
 * can name the method for every install type without touching the network.
 */
export function planUpdate(method: Installation.Method, version = "latest"): UpdatePlan {
  const action = Updater.actionFor(method)
  if (action === "managed") {
    return { kind: "managed", method, line: "Allternit Desktop manages this copy; it updates with Desktop." }
  }
  if (action === "command") {
    const command = Updater.commandFor(method, version) ?? "gizzi update"
    return { kind: "command", method, command, line: `Method ${method}: run this to update: ${command}` }
  }
  return {
    kind: "apply",
    method,
    line:
      method === "npm"
        ? "Method npm: would check the signed feed, then run npm i -g for the new version."
        : `Method ${method}: would check the signed feed, verify the download, and swap the binary.`,
  }
}

export const UpgradeCommand = {
  command: ["upgrade [target]", "update [target]"],
  describe: "upgrade gizzi to the latest or a specific version",
  builder: (yargs: Argv) => {
    return yargs
      .positional("target", {
        describe: "version to upgrade to, for ex '0.1.48' or 'v0.1.48'",
        type: "string",
      })
      .option("method", {
        alias: "m",
        describe: "installation method to use",
        type: "string",
        choices: ["curl", "npm", "pnpm", "bun", "brew", "scoop"],
      })
      .option("channel", {
        describe: "release channel to follow (saved)",
        type: "string",
        choices: [...ReleaseFeed.CHANNELS],
      })
      .option("dry-run", {
        describe: "print the method and what would happen, change nothing",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args: { target?: string; method?: string; channel?: string; dryRun?: boolean }) => {
    UI.empty()
    UI.println(UI.logo("  "))
    UI.empty()
    prompts.intro("Upgrade")
    if (args.channel && ReleaseFeed.isChannel(args.channel)) {
      if (!args.dryRun) ReleaseFeed.setChannel(args.channel)
      prompts.log.info(`Channel: ${args.channel}${args.dryRun ? " (dry run, not saved)" : " (saved)"}`)
    }
    const detectedMethod = await Installation.method()
    const method = (args.method as Installation.Method) ?? detectedMethod

    // No explicit version: follow the signed feed per install method.
    if (!args.target && method !== "unknown") {
      const plan = planUpdate(method)
      prompts.log.info(`Install method: ${method}`)
      if (plan.kind !== "apply") {
        prompts.log.info(plan.line)
        prompts.outro("Done")
        return
      }
      if (args.dryRun) {
        prompts.log.info(plan.line)
        prompts.outro("Dry run: no changes made")
        return
      }
      const spinner = prompts.spinner()
      spinner.start("Checking the signed release feed...")
      // An explicit `gizzi update` applies even when background auto-update is off.
      const env = { ...process.env }
      delete env.GIZZI_DISABLE_AUTOUPDATE
      delete env.DISABLE_AUTOUPDATER
      const status = await Updater.check({ method, env })
      spinner.stop(status.message)
      if (status.state === "staged") {
        const swapped = Updater.applyStaged()
        prompts.log[swapped ? "success" : "warn"](
          swapped ? `Updated to ${status.latest}` : "The staged update could not be applied; it will retry on next launch.",
        )
      } else if (status.state === "scheduled") {
        const started = Updater.runPendingPackage()
        prompts.log[started ? "success" : "warn"](
          started ? `Installing ${status.latest} with npm in the background.` : `Run: ${status.command}`,
        )
      }
      prompts.outro("Done")
      return
    }
    if (args.dryRun) {
      prompts.log.info(`Method ${method}: would install ${args.target ?? "the latest version"}`)
      prompts.outro("Dry run: no changes made")
      return
    }
    if (method === "unknown") {
      prompts.log.error(`gizzi is installed to ${process.execPath} and may be managed by a package manager`)
      const install = await prompts.select({
        message: "Install anyways?",
        options: [
          { label: "Yes", value: true },
          { label: "No", value: false },
        ],
        initialValue: false,
      })
      if (!install) {
        prompts.outro("Done")
        return
      }
    }
    prompts.log.info("Using method: " + method)
    const target = args.target ? args.target.replace(/^v/, "") : await Installation.latest()

    if (Installation.VERSION === target) {
      prompts.log.warn(`gizzi upgrade skipped: ${target} is already installed`)
      prompts.outro("Done")
      return
    }

    prompts.log.info(`From ${Installation.VERSION} → ${target}`)
    const spinner = prompts.spinner()
    spinner.start("Upgrading...")
    const err = await Installation.upgrade(method, target).catch((err) => err)
    if (err) {
      spinner.stop("Upgrade failed", 1)
      if (err instanceof Installation.UpgradeFailedError) {
        // necessary because choco only allows install/upgrade in elevated terminals
        const stderr = (err.data as { stderr?: string } | undefined)?.stderr
        if (method === "choco" && stderr?.includes("not running from an elevated command shell")) {
          prompts.log.error("Please run the terminal as Administrator and try again")
        } else if (stderr) {
          prompts.log.error(stderr)
        }
      } else if (err instanceof Error) prompts.log.error(err.message)
      prompts.outro("Done")
      return
    }
    spinner.stop("Upgrade complete")
    prompts.outro("Done")
  },
}
