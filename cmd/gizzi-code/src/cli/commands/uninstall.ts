import type { Argv } from "yargs"
import { UI } from "@/cli/ui"
import * as prompts from "@clack/prompts"
import { Installation } from "@/shared/installation"
import { Global } from "@/runtime/context/global"
import { $ } from "bun"
import fs from "fs/promises"
import path from "path"
import os from "os"
import { Filesystem } from "@/shared/util/filesystem"
import {
  cleanShellConfigContent,
  removeMarkedBlock,
  POWERSHELL_BLOCK_BEGIN,
  POWERSHELL_BLOCK_END,
} from "@/cli/commands/uninstallShellConfig"
import {
  detectInstallMethod,
  gizziHome,
  packageUninstallCommand,
  planGizziHome,
  type GizziHomePlan,
  type InstallMethod,
} from "@/cli/commands/uninstallPlan"
import { isDaemonRunning, stopRemoteDaemon } from "@/runtime/automation/cron/daemon"

interface UninstallArgs {
  keepConfig: boolean
  keepData: boolean
  dryRun: boolean
  force: boolean
  yes: boolean
}

interface RemovalTargets {
  directories: Array<{ path: string; label: string; keep: boolean }>
  shellConfig: string | null
  binary: string | null
  gizziHome: GizziHomePlan
  packageHint: string | null
}

export const UninstallCommand = {
  command: "uninstall",
  describe: "uninstall gizzi and remove all related files",
  builder: (yargs: Argv) =>
    yargs
      .option("keep-config", {
        alias: "c",
        type: "boolean",
        describe: "keep configuration files",
        default: false,
      })
      .option("keep-data", {
        alias: "d",
        type: "boolean",
        describe: "keep session data and snapshots",
        default: false,
      })
      .option("dry-run", {
        type: "boolean",
        describe: "show what would be removed without removing",
        default: false,
      })
      .option("force", {
        alias: "f",
        type: "boolean",
        describe: "skip confirmation prompts",
        default: false,
      })
      .option("yes", {
        type: "boolean",
        describe: "answer yes to the confirmation prompt (required for non-interactive uninstall)",
        default: false,
      }),

  handler: async (args: UninstallArgs) => {
    UI.empty()
    UI.println(UI.logo("  "))
    UI.empty()
    prompts.intro("Uninstall GIZZI")

    // Path/argv detection first (authoritative, no subprocess); fall back to
    // the package-manager probe only when the path is inconclusive.
    const detectedMethod: InstallMethod = detectInstallMethod(process.execPath, process.argv)
    const method =
      detectedMethod !== "unknown" ? (detectedMethod as Installation.Method) : await Installation.method()
    prompts.log.info(`Installation method: ${method}`)

    // A non-interactive terminal cannot answer the confirmation prompt —
    // require an explicit --yes/--force instead of hanging on stdin.
    if (
      process.stdin.isTTY !== true &&
      !args.force &&
      !args.yes &&
      !args.dryRun
    ) {
      prompts.log.error("Non-interactive terminal: re-run with --yes to confirm the uninstall.")
      prompts.outro("Aborted")
      process.exit(1)
    }

    const targets = await collectRemovalTargets(args, method)

    await showRemovalSummary(targets, method)

    if (!args.force && !args.yes && !args.dryRun) {
      const confirm = await prompts.confirm({
        message: "Are you sure you want to uninstall?",
        initialValue: false,
      })
      if (!confirm || prompts.isCancel(confirm)) {
        prompts.outro("Cancelled")
        return
      }
    }

    if (args.dryRun) {
      prompts.log.warn("Dry run - no changes made")
      prompts.outro("Done")
      return
    }

    await executeUninstall(method, targets)

    prompts.outro("Done")
  },
}

async function collectRemovalTargets(args: UninstallArgs, method: Installation.Method): Promise<RemovalTargets> {
  const directories: RemovalTargets["directories"] = [
    { path: Global.Path.data, label: "Data", keep: args.keepData },
    { path: Global.Path.cache, label: "Cache", keep: false },
    { path: Global.Path.config, label: "Config", keep: args.keepConfig },
    { path: Global.Path.state, label: "State", keep: false },
    // ~/.gizzi: credentials fallback, plugins, curl-install bin, global
    // workspace, cron state. Removed with Config unless --keep-config.
    { path: gizziHome(os.homedir()), label: "Home", keep: args.keepConfig },
  ]

  const shellConfig = method === "curl" ? await getShellConfigFile() : null
  const binary = method === "curl" ? process.execPath : null
  const gizziHomePlan = await planGizziHome(os.homedir())
  const packageHint = packageUninstallCommand(
    detectInstallMethod(process.execPath, process.argv),
  )

  return { directories, shellConfig, binary, gizziHome: gizziHomePlan, packageHint }
}

async function showRemovalSummary(targets: RemovalTargets, method: Installation.Method) {
  prompts.log.message("The following will be removed:")

  for (const dir of targets.directories) {
    const exists = await fs
      .access(dir.path)
      .then(() => true)
      .catch(() => false)
    if (!exists) continue

    const size = await getDirectorySize(dir.path)
    const sizeStr = formatSize(size)
    const status = dir.keep ? UI.Style.TEXT_DIM + "(keeping)" : ""
    const prefix = dir.keep ? "○" : "✓"

    prompts.log.info(`  ${prefix} ${dir.label}: ${shortenPath(dir.path)} ${UI.Style.TEXT_DIM}(${sizeStr})${status}`)

    if (dir.label === "Home" && targets.gizziHome.exists && !dir.keep) {
      for (const entry of targets.gizziHome.entries) {
        prompts.log.info(`      ${shortenPath(entry.path)} ${UI.Style.TEXT_DIM}[${entry.kind}]`)
      }
    }
  }

  if (targets.binary) {
    prompts.log.info(`  ✓ Binary: ${shortenPath(targets.binary)}`)
  }

  if (targets.shellConfig) {
    prompts.log.info(`  ✓ Shell PATH in ${shortenPath(targets.shellConfig)}`)
  }

  if (targets.packageHint && method !== "curl" && method !== "unknown") {
    prompts.log.info(`  ✓ Package: ${targets.packageHint}`)
  }

  if (process.platform === "win32" && (method === "curl" || method === "unknown")) {
    prompts.log.warn(
      "If you installed via install.ps1, remove the gizzi install directory from your User PATH (System Properties → Environment Variables). It is not removed automatically.",
    )
  }
}

async function executeUninstall(method: Installation.Method, targets: RemovalTargets) {
  const spinner = prompts.spinner()
  const errors: string[] = []

  // Stop the cron daemon before its database/state directories go away —
  // otherwise it keeps running with deleted state and a stale port binding.
  if (await isDaemonRunning(3031)) {
    spinner.start("Stopping cron daemon...")
    const stopped = await stopRemoteDaemon(3031)
    if (stopped) {
      spinner.stop("Cron daemon stopped")
    } else {
      spinner.stop("Failed to stop cron daemon", 1)
      errors.push("Cron daemon: shutdown request failed — it may need `gizzi workflows wake jobs stop` manually")
    }
  }

  for (const dir of targets.directories) {
    if (dir.keep) {
      prompts.log.step(`Skipping ${dir.label} (--keep-${dir.label.toLowerCase()})`)
      continue
    }

    const exists = await fs
      .access(dir.path)
      .then(() => true)
      .catch(() => false)
    if (!exists) continue

    spinner.start(`Removing ${dir.label}...`)
    const err = await fs.rm(dir.path, { recursive: true, force: true }).catch((e) => e)
    if (err) {
      spinner.stop(`Failed to remove ${dir.label}`, 1)
      errors.push(`${dir.label}: ${err.message}`)
      continue
    }
    spinner.stop(`Removed ${dir.label}`)
  }

  if (targets.shellConfig) {
    spinner.start("Cleaning shell config...")
    const err = await cleanShellConfig(targets.shellConfig).catch((e) => e)
    if (err) {
      spinner.stop("Failed to clean shell config", 1)
      errors.push(`Shell config: ${err.message}`)
    } else {
      spinner.stop("Cleaned shell config")
    }
  }

  if (method !== "curl" && method !== "unknown") {
    const cmd =
      targets.packageHint?.split(" ") ??
      packageUninstallCommand(method as InstallMethod)?.split(" ") ??
      null

    if (cmd) {
      spinner.start(`Running ${cmd.join(" ")}...`)
      const result =
        method === "choco"
          ? await $`echo Y | choco uninstall gizzi -y -r`.quiet().nothrow()
          : await $`${cmd}`.quiet().nothrow()
      if (result.exitCode !== 0) {
        spinner.stop(`Package manager uninstall failed: exit code ${result.exitCode}`, 1)
        if (
          method === "choco" &&
          result.stdout.toString("utf8").includes("not running from an elevated command shell")
        ) {
          prompts.log.warn(`You may need to run '${cmd.join(" ")}' from an elevated command shell`)
        } else {
          prompts.log.warn(`You may need to run manually: ${cmd.join(" ")}`)
        }
      } else {
        spinner.stop("Package removed")
      }
    }
  }

  if (method === "curl" && targets.binary) {
    UI.empty()
    prompts.log.message("To finish removing the binary, run:")
    prompts.log.info(`  rm "${targets.binary}"`)

    const binDir = path.dirname(targets.binary)
    if (binDir.includes(".gizzi")) {
      prompts.log.info(`  rmdir "${binDir}" 2>/dev/null`)
    }
  }

  if (errors.length > 0) {
    UI.empty()
    prompts.log.warn("Some operations failed:")
    for (const err of errors) {
      prompts.log.error(`  ${err}`)
    }
  }

  UI.empty()
  prompts.log.success("Thank you for using GIZZI!")
}

async function getShellConfigFile(): Promise<string | null> {
  const shell = path.basename(process.env.SHELL || "bash")
  const home = os.homedir()
  const xdgConfig = process.env.XDG_CONFIG_HOME || path.join(home, ".config")

  const configFiles: Record<string, string[]> = {
    fish: [path.join(xdgConfig, "fish", "config.fish")],
    zsh: [
      path.join(home, ".zshrc"),
      path.join(home, ".zshenv"),
      path.join(xdgConfig, "zsh", ".zshrc"),
      path.join(xdgConfig, "zsh", ".zshenv"),
    ],
    bash: [
      path.join(home, ".bashrc"),
      path.join(home, ".bash_profile"),
      path.join(home, ".profile"),
      path.join(xdgConfig, "bash", ".bashrc"),
      path.join(xdgConfig, "bash", ".bash_profile"),
    ],
    ash: [path.join(home, ".ashrc"), path.join(home, ".profile")],
    sh: [path.join(home, ".profile")],
  }

  const candidates = configFiles[shell] || configFiles.bash

  for (const file of candidates) {
    const exists = await fs
      .access(file)
      .then(() => true)
      .catch(() => false)
    if (!exists) continue

    const content = await Filesystem.readText(file).catch(() => "")
    if (
      content.includes("# gizzi") ||
      content.includes("# gizzi-code begin") ||
      content.includes(".gizzi/bin")
    ) {
      return file
    }
  }

  return null
}

async function cleanShellConfig(file: string) {
  const content = await Filesystem.readText(file)

  // PowerShell $PROFILE: no gizzi installer writes it today, but if one
  // ever did, remove ONLY the marked block. Markers absent → warn and
  // leave the file untouched rather than rewriting user content.
  if (file.endsWith(".ps1")) {
    const { output, result } = removeMarkedBlock(content, POWERSHELL_BLOCK_BEGIN, POWERSHELL_BLOCK_END)
    if (output === null) {
      if (result.status === "unbalanced-begin") {
        prompts.log.warn(
          `${shortenPath(file)} has a '${POWERSHELL_BLOCK_BEGIN}' marker without a matching end marker; leaving it untouched.`,
        )
      } else if (result.status === "no-markers" && result.gizziReferences) {
        prompts.log.warn(
          `${shortenPath(file)} contains gizzi references but no gizzi marker block; leaving it untouched. Remove them manually if desired.`,
        )
      }
      return
    }
    await Filesystem.write(file, output)
    return
  }

  const { output, result } = cleanShellConfigContent(content)
  if (output === null) {
    if (result.status === "unbalanced-begin") {
      prompts.log.warn(
        `${shortenPath(file)} has a '# gizzi-code begin' marker without a matching end marker; leaving it untouched.`,
      )
    } else if (result.status === "no-markers" && result.gizziReferences) {
      prompts.log.warn(
        `${shortenPath(file)} contains .gizzi references but no gizzi marker block; leaving it untouched. Remove them manually if desired.`,
      )
    }
    return
  }
  await Filesystem.write(file, output)
}

async function getDirectorySize(dir: string): Promise<number> {
  let total = 0

  const walk = async (current: string) => {
    const entries = await fs.readdir(current, { withFileTypes: true }).catch(() => [])

    for (const entry of entries) {
      const full = path.join(current, entry.name)
      if (entry.isDirectory()) {
        await walk(full)
        continue
      }
      if (entry.isFile()) {
        const stat = await fs.stat(full).catch(() => null)
        if (stat) total += stat.size
      }
    }
  }

  await walk(dir)
  return total
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`
}

function shortenPath(p: string): string {
  const home = os.homedir()
  if (p.startsWith(home)) {
    return p.replace(home, "~")
  }
  return p
}
