import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd.js"
import { UI } from "@/cli/ui/index.js"
import * as prompts from "@clack/prompts"
import { existsSync, readdirSync, mkdirSync, rmSync } from "fs"
import path from "path"
import os from "os"
import { execSync } from "child_process"

function findExistingApp(): string | null {
  const candidates = [
    "/Applications/Allternit Desktop.app",
    path.join(os.homedir(), "Applications", "Allternit Desktop.app"),
    "/Applications/Allternit.app",
    path.join(os.homedir(), "Applications", "Allternit.app"),
  ]
  for (const c of candidates) {
    if (existsSync(c)) return c
  }
  return null
}

export const AllternitCommand = cmd({
  command: "allternit [path]",
  describe: "launch Allternit Desktop app, downloading/installing it if missing",
  builder: (yargs: Argv) => {
    return yargs
      .positional("path", {
        describe: "workspace path to open in Allternit Desktop",
        type: "string",
        default: ".",
      })
      .option("download-url", {
        describe: "custom download URL for the macOS DMG installer",
        type: "string",
      })
  },
  handler: async (args: { path: string; "download-url"?: string }) => {
    UI.empty()
    UI.println(UI.logo("  "))
    UI.empty()
    prompts.intro("Allternit Desktop Launcher")

    if (process.platform !== "darwin") {
      prompts.log.error("The allternit command is currently only supported on macOS.")
      prompts.outro("Done")
      return
    }

    const targetPath = path.resolve(args.path)
    const existingApp = findExistingApp()

    if (existingApp) {
      prompts.log.info(`Found Allternit Desktop installed at: ${existingApp}`)
      const spinner = prompts.spinner()
      spinner.start(`Opening workspace in Allternit Desktop...`)
      try {
        execSync(`open -a "${existingApp}" "${targetPath}"`)
        spinner.stop("Launched successfully")
      } catch (err: any) {
        spinner.stop("Failed to launch", 1)
        prompts.log.error(err.message || String(err))
      }
      prompts.outro("Done")
      return
    }

    prompts.log.warn("Allternit Desktop not found on this machine.")

    const arch = process.arch === "arm64" ? "arm64" : "x64"
    const defaultUrl = `https://github.com/Allternit/desktop/releases/download/v1.0.0/Allternit-Desktop-1.0.0-${arch}.dmg`
    const downloadUrl = args["download-url"] || defaultUrl

    const shouldDownload = await prompts.confirm({
      message: `Would you like to download and install Allternit Desktop from ${downloadUrl}?`,
      initialValue: true,
    })

    if (!shouldDownload) {
      prompts.outro("Installation cancelled")
      return
    }

    const tempDir = path.join(os.tmpdir(), `allternit-desktop-install-${Date.now()}`)
    mkdirSync(tempDir, { recursive: true })
    const dmgPath = path.join(tempDir, "Allternit-Desktop.dmg")

    const spinner = prompts.spinner()
    spinner.start("Downloading Allternit Desktop installer...")
    try {
      execSync(`curl -fL --retry 3 --retry-delay 1 -o "${dmgPath}" "${downloadUrl}"`)
      spinner.stop("Downloaded successfully")
    } catch (err: any) {
      spinner.stop("Download failed", 1)
      prompts.log.error(err.message || String(err))
      try { rmSync(tempDir, { recursive: true, force: true }) } catch { /* best-effort temp cleanup */ }
      prompts.outro("Failed")
      return
    }

    spinner.start("Mounting installer DMG...")
    let mountPoint = ""
    try {
      const mountOutput = execSync(`hdiutil attach -nobrowse -readonly "${dmgPath}"`).toString()
      const lines = mountOutput.split("\n")
      for (const line of lines) {
        if (line.includes("/Volumes/")) {
          const parts = line.split("\t")
          if (parts.length > 1) {
            mountPoint = parts[parts.length - 1].trim()
            break
          }
          const match = line.split(/\s+/).find(p => p.startsWith("/Volumes/"))
          if (match) {
            mountPoint = match
            break
          }
        }
      }
      if (!mountPoint) {
        throw new Error("Could not determine mount point from hdiutil output")
      }
      spinner.stop(`Mounted at: ${mountPoint}`)
    } catch (err: any) {
      spinner.stop("Mount failed", 1)
      prompts.log.error(err.message || String(err))
      try { rmSync(tempDir, { recursive: true, force: true }) } catch { /* best-effort temp cleanup */ }
      prompts.outro("Failed")
      return
    }

    spinner.start("Installing app bundle...")
    let destApp = ""
    try {
      const entries = readdirSync(mountPoint)
      const appEntry = entries.find(e => e.endsWith(".app"))
      if (!appEntry) {
        throw new Error("No .app bundle found inside the mounted DMG")
      }

      // Check if /Applications is writable, otherwise install to user Applications
      let destDir = "/Applications"
      try {
        const testFile = "/Applications/.allternit-test-write"
        execSync(`touch "${testFile}" && rm "${testFile}"`, { stdio: "ignore" })
      } catch {
        destDir = path.join(os.homedir(), "Applications")
        mkdirSync(destDir, { recursive: true })
      }

      destApp = path.join(destDir, appEntry)
      prompts.log.info(`Copying ${appEntry} to ${destDir}...`)
      execSync(`ditto "${path.join(mountPoint, appEntry)}" "${destApp}"`)
      spinner.stop("Installed successfully")
    } catch (err: any) {
      spinner.stop("Installation failed", 1)
      prompts.log.error(err.message || String(err))
    }

    spinner.start("Detaching installer...")
    try {
      execSync(`hdiutil detach "${mountPoint}"`)
      spinner.stop("Detached successfully")
    } catch (err: any) {
      spinner.stop("Failed to detach DMG", 1)
      prompts.log.error(err.message || String(err))
    }

    try { rmSync(tempDir, { recursive: true, force: true }) } catch { /* best-effort temp cleanup */ }

    if (destApp && existsSync(destApp)) {
      spinner.start(`Launching ${destApp}...`)
      try {
        execSync(`open -a "${destApp}" "${targetPath}"`)
        spinner.stop("Launched successfully")
      } catch (err: any) {
        spinner.stop("Failed to launch app", 1)
        prompts.log.error(err.message || String(err))
      }
    }

    prompts.outro("Done")
  },
})
