/**
 * Cron daemon supervision: platform autostart units (launchd / systemd
 * --user), pidfile lifecycle, and crash recording.
 *
 * All path-producing and string-producing functions are pure so tests can
 * snapshot the generated units and exercise pidfile handling in tmpdirs.
 */

import fs from "fs/promises"
import fsSync from "fs"
import path from "path"
import os from "os"
import { Global } from "@/runtime/context/global"

export const CRON_DAEMON_LABEL = "com.allternit.gizzi.cron"

// ═══════════════════════════════════════════════════════════════════════════════
// launchd (macOS)
// ═══════════════════════════════════════════════════════════════════════════════

export type LaunchdPlistOptions = {
  label: string
  /** Absolute path to the gizzi binary. */
  program: string
  args: string[]
  stdoutLog: string
  stderrLog: string
}

/** Generate a LaunchAgent plist that keeps the cron daemon alive. */
export function launchdPlist(opts: LaunchdPlistOptions): string {
  const args = opts.args.map((a) => `    <string>${a}</string>`).join("\n")
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${opts.label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${opts.program}</string>
${args}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>${opts.stdoutLog}</string>
  <key>StandardErrorPath</key>
  <string>${opts.stderrLog}</string>
</dict>
</plist>
`
}

export function launchdPlistPath(home: string = os.homedir()): string {
  return path.join(home, "Library", "LaunchAgents", `${CRON_DAEMON_LABEL}.plist`)
}

// ═══════════════════════════════════════════════════════════════════════════════
// systemd --user (Linux)
// ═══════════════════════════════════════════════════════════════════════════════

export type SystemdUnitOptions = {
  description: string
  /** Full ExecStart line, e.g. "/usr/local/bin/gizzi workflows wake jobs start". */
  execStart: string
}

/** Generate a systemd --user unit for the cron daemon. */
export function systemdUserUnit(opts: SystemdUnitOptions): string {
  return `[Unit]
Description=${opts.description}

[Service]
ExecStart=${opts.execStart}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
`
}

export function systemdUnitPath(home: string = os.homedir()): string {
  return path.join(home, ".config", "systemd", "user", `${CRON_DAEMON_LABEL}.service`)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Task Scheduler (Windows)
// ═══════════════════════════════════════════════════════════════════════════════

export type WindowsTaskOptions = {
  description: string
  /** Absolute path of the installed gizzi.exe. */
  program: string
  args: string[]
}

const xmlEscape = (v: string): string =>
  v.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")

/** Quote one argument for a Windows command line (CommandLineToArgvW rules). */
export function windowsQuoteArg(arg: string): string {
  if (arg !== "" && !/[\s"]/.test(arg)) return arg
  return `"${arg.replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/, "$1$1")}"`
}

/**
 * Task Scheduler definition for the cron daemon: starts at logon for the
 * current user (no admin rights, no stored password), restarts on failure,
 * no time limit. The program runs under `conhost.exe --headless` so no
 * console window opens on the user's desktop.
 */
export function windowsTaskXml(opts: WindowsTaskOptions): string {
  const commandLine = ["--headless", opts.program, ...opts.args].map(windowsQuoteArg).join(" ")
  return `<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>${xmlEscape(opts.description)}</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author"><Exec><Command>%SystemRoot%\\System32\\conhost.exe</Command><Arguments>${xmlEscape(commandLine)}</Arguments></Exec></Actions>
</Task>
`
}

/** Where the task definition is kept (also the "installed" marker, like the plist/unit). */
export function windowsTaskXmlPath(home: string = os.homedir()): string {
  return path.join(home, "AppData", "Local", "gizzi", "autostart", `${CRON_DAEMON_LABEL}.xml`)
}

/** Task Scheduler name (a folder keeps it out of the root task list). */
export const WINDOWS_TASK_NAME = `Allternit\\${CRON_DAEMON_LABEL}`

// ═══════════════════════════════════════════════════════════════════════════════
// State dir (pidfile + crash record)
// ═══════════════════════════════════════════════════════════════════════════════

export function cronStateDir(): string {
  const override = process.env.GIZZI_CRON_STATE_DIR?.trim()
  if (override) return override
  return path.join(Global.Path.state, "cron")
}

export function pidfilePath(stateDir: string = cronStateDir()): string {
  return path.join(stateDir, "daemon.pid")
}

export type CrashRecord = {
  at: string
  pid: number
  message: string
  stack?: string
}

export function crashRecordPath(stateDir: string = cronStateDir()): string {
  return path.join(stateDir, "last-crash.json")
}

// ═══════════════════════════════════════════════════════════════════════════════
// Pidfile lifecycle — a crashed daemon must never leave a stale pidfile that
// claims it is alive.
// ═══════════════════════════════════════════════════════════════════════════════

export type PidfileState =
  | { status: "none" }
  | { status: "running"; pid: number }
  | { status: "stale"; pid: number }

const pidAlive = (pid: number): boolean => {
  try {
    process.kill(pid, 0)
    return true
  } catch (e: any) {
    return e?.code === "EPERM"
  }
}

/**
 * Validate the pidfile. A pidfile pointing at a dead process is stale: it is
 * removed and reported as such, so callers never trust a dead daemon's
 * pidfile. Call this on daemon startup and on `cron status`.
 */
export async function validatePidfile(
  pidfile: string,
  isAlive: (pid: number) => boolean = pidAlive,
): Promise<PidfileState> {
  let raw: string
  try {
    raw = await fs.readFile(pidfile, "utf8")
  } catch {
    return { status: "none" }
  }
  const pid = Number.parseInt(raw.trim(), 10)
  if (!Number.isInteger(pid) || pid <= 0) {
    await fs.rm(pidfile, { force: true })
    return { status: "none" }
  }
  if (isAlive(pid)) return { status: "running", pid }
  await fs.rm(pidfile, { force: true })
  return { status: "stale", pid }
}

export async function writePidfile(pidfile: string, pid: number = process.pid): Promise<void> {
  await fs.mkdir(path.dirname(pidfile), { recursive: true })
  await fs.writeFile(pidfile, `${pid}\n`)
}

export async function clearPidfile(pidfile: string): Promise<void> {
  await fs.rm(pidfile, { force: true })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Crash recording
// ═══════════════════════════════════════════════════════════════════════════════

/** Record a daemon crash so `cron status` can report the last one. */
export async function recordDaemonCrash(
  error: unknown,
  stateDir: string = cronStateDir(),
  pid: number = process.pid,
): Promise<CrashRecord> {
  const record: CrashRecord = {
    at: new Date().toISOString(),
    pid,
    message: error instanceof Error ? error.message : String(error),
    stack: error instanceof Error ? error.stack : undefined,
  }
  await fs.mkdir(stateDir, { recursive: true })
  await fs.writeFile(crashRecordPath(stateDir), JSON.stringify(record, null, 2) + "\n")
  return record
}

export async function readLastCrash(stateDir: string = cronStateDir()): Promise<CrashRecord | null> {
  try {
    return JSON.parse(await fs.readFile(crashRecordPath(stateDir), "utf8")) as CrashRecord
  } catch {
    return null
  }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Supervision install / remove / status
// ═══════════════════════════════════════════════════════════════════════════════

export type SupervisionPlatform = "launchd" | "systemd" | "schtasks" | "unsupported"

export type SupervisionState = {
  platform: SupervisionPlatform
  supported: boolean
  launchdPlist: string | null
  systemdUnit: string | null
  /** Windows: the saved Task Scheduler definition when autostart is installed. */
  scheduledTask: string | null
}

export function supervisionPlatform(platform: NodeJS.Platform = process.platform): SupervisionPlatform {
  if (platform === "darwin") return "launchd"
  if (platform === "linux") return "systemd"
  if (platform === "win32") return "schtasks"
  return "unsupported"
}

export async function supervisionState(home: string = os.homedir()): Promise<SupervisionState> {
  const platform = supervisionPlatform()
  const plist = launchdPlistPath(home)
  const unit = systemdUnitPath(home)
  const task = windowsTaskXmlPath(home)
  return {
    platform,
    supported: platform !== "unsupported",
    launchdPlist: platform === "launchd" && fsSync.existsSync(plist) ? plist : null,
    systemdUnit: platform === "systemd" && fsSync.existsSync(unit) ? unit : null,
    scheduledTask: platform === "schtasks" && fsSync.existsSync(task) ? task : null,
  }
}

const SUPERVISABLE_BINARIES = new Set(["bun", "node", "bunx", "npx", "tsx", "deno"])

/**
 * True when execPath is a standalone installed binary (not a JS runtime) —
 * required because launchd/systemd/Task Scheduler must be able to exec the program directly.
 */
export function isSupervisableExec(execPath: string): boolean {
  // Split on both separators: path.basename is platform-specific, and a
  // Windows-style path may be inspected on a POSIX host.
  let base = (execPath.toLowerCase().split(/[\\/]/).pop() ?? "")
  // Runtimes also ship as bun.exe / node.exe on Windows — strip before matching.
  if (base.endsWith(".exe")) base = base.slice(0, -4)
  if (base.length === 0 || SUPERVISABLE_BINARIES.has(base)) return false
  if (base.endsWith(".js") || base.endsWith(".ts") || base.endsWith(".mjs") || base.endsWith(".cjs")) return false
  return true
}

export type EnableSupervisionResult = {
  ok: boolean
  platform: SupervisionPlatform
  path?: string
  message: string
}

/**
 * Install boot/login autostart for the cron daemon using the platform's
 * standard mechanism. Best-effort: the unit/plist is always written; the
 * load/enable step warns instead of failing when the OS tool is missing.
 */
export async function enableSupervision(
  execPath: string,
  home: string = os.homedir(),
): Promise<EnableSupervisionResult> {
  const platform = supervisionPlatform()

  if (platform === "unsupported") {
    return {
      ok: false,
      platform,
      message: `cron autostart is not supported on ${process.platform}. Start the daemon manually with \`gizzi workflows wake jobs start\`.`,
    }
  }
  if (!isSupervisableExec(execPath)) {
    return {
      ok: false,
      platform,
      message:
        "autostart requires the installed gizzi binary, not a JS runtime " +
        `(${path.basename(execPath)}). Re-run \`gizzi workflows wake jobs enable\` from the installed binary.`,
    }
  }

  const logDir = path.join(cronStateDir(), "logs")
  if (platform === "launchd") {
    const plistPath = launchdPlistPath(home)
    const plist = launchdPlist({
      label: CRON_DAEMON_LABEL,
      program: execPath,
      args: ["workflows", "wake", "jobs", "start"],
      stdoutLog: path.join(logDir, "daemon.out.log"),
      stderrLog: path.join(logDir, "daemon.err.log"),
    })
    await fs.mkdir(path.dirname(plistPath), { recursive: true })
    await fs.writeFile(plistPath, plist)
    let message = `LaunchAgent installed: ${plistPath}`
    try {
      const { $ } = await import("bun")
      await $`launchctl bootout gui/${process.getuid?.() ?? 0}/${CRON_DAEMON_LABEL}`.quiet().nothrow()
      const res = await $`launchctl bootstrap gui/${process.getuid?.() ?? 0} ${plistPath}`.quiet().nothrow()
      if (res.exitCode === 0) {
        message += " (loaded — starts on login, restarts on crash)"
      } else {
        message += ` (load it with: launchctl bootstrap gui/$UID ${plistPath})`
      }
    } catch {
      message += ` (load it with: launchctl bootstrap gui/$UID ${plistPath})`
    }
    return { ok: true, platform, path: plistPath, message }
  }

  if (platform === "schtasks") {
    const xmlPath = windowsTaskXmlPath(home)
    const xml = windowsTaskXml({
      description: "Gizzi cron daemon (scheduled workflows)",
      program: execPath,
      args: ["workflows", "wake", "jobs", "start"],
    })
    await fs.mkdir(path.dirname(xmlPath), { recursive: true })
    // schtasks /XML reads UTF-16 reliably; write it with a BOM to match the declaration.
    await fs.writeFile(xmlPath, Buffer.from("\ufeff" + xml, "utf16le"))
    const manual = `schtasks /Create /TN "${WINDOWS_TASK_NAME}" /XML "${xmlPath}" /F`
    try {
      const { $ } = await import("bun")
      const res = await $`schtasks /Create /TN ${WINDOWS_TASK_NAME} /XML ${xmlPath} /F`.quiet().nothrow()
      if (res.exitCode !== 0) {
        await fs.rm(xmlPath, { force: true })
        const detail = (res.stderr.toString() || res.stdout.toString()).trim()
        return { ok: false, platform, message: `couldn't register the scheduled task${detail ? `: ${detail}` : ""}. Try: ${manual}` }
      }
      await $`schtasks /Run /TN ${WINDOWS_TASK_NAME}`.quiet().nothrow()
    } catch (err) {
      await fs.rm(xmlPath, { force: true })
      return { ok: false, platform, message: `couldn't run schtasks (${(err as Error).message}). Try: ${manual}` }
    }
    return {
      ok: true,
      platform,
      path: xmlPath,
      message: `Scheduled task installed: ${WINDOWS_TASK_NAME} (starts at logon, restarts on failure)`,
    }
  }

  const unitPath = systemdUnitPath(home)
  const unit = systemdUserUnit({
    description: "Gizzi cron daemon",
    execStart: `${execPath} workflows wake jobs start`,
  })
  await fs.mkdir(path.dirname(unitPath), { recursive: true })
  await fs.writeFile(unitPath, unit)
  let message = `systemd user unit installed: ${unitPath}`
  try {
    const { $ } = await import("bun")
    await $`systemctl --user daemon-reload`.quiet().nothrow()
    const res = await $`systemctl --user enable --now ${path.basename(unitPath)}`.quiet().nothrow()
    if (res.exitCode === 0) {
      message += " (enabled — starts on login, restarts on failure)"
    } else {
      message += ` (enable it with: systemctl --user enable --now ${path.basename(unitPath)})`
    }
  } catch {
    message += ` (enable it with: systemctl --user enable --now ${path.basename(unitPath)})`
  }
  return { ok: true, platform, path: unitPath, message }
}

export type DisableSupervisionResult = {
  removed: boolean
  path: string | null
  message: string
}

/** Remove autostart (best-effort unload/disable, then delete the unit file). */
export async function disableSupervision(home: string = os.homedir()): Promise<DisableSupervisionResult> {
  if (process.platform === "darwin") {
    const plistPath = launchdPlistPath(home)
    if (!fsSync.existsSync(plistPath)) {
      return { removed: false, path: null, message: "no LaunchAgent installed" }
    }
    try {
      const { $ } = await import("bun")
      await $`launchctl bootout gui/${process.getuid?.() ?? 0}/${CRON_DAEMON_LABEL}`.quiet().nothrow()
    } catch {
      // Unload is best-effort; the file removal below is the source of truth.
    }
    await fs.rm(plistPath, { force: true })
    return { removed: true, path: plistPath, message: `LaunchAgent removed: ${plistPath}` }
  }
  if (process.platform === "linux") {
    const unitPath = systemdUnitPath(home)
    if (!fsSync.existsSync(unitPath)) {
      return { removed: false, path: null, message: "no systemd user unit installed" }
    }
    try {
      const { $ } = await import("bun")
      await $`systemctl --user disable --now ${path.basename(unitPath)}`.quiet().nothrow()
      await $`systemctl --user daemon-reload`.quiet().nothrow()
    } catch {
      // Best-effort.
    }
    await fs.rm(unitPath, { force: true })
    return { removed: true, path: unitPath, message: `systemd user unit removed: ${unitPath}` }
  }
  if (process.platform === "win32") {
    const xmlPath = windowsTaskXmlPath(home)
    if (!fsSync.existsSync(xmlPath)) {
      return { removed: false, path: null, message: "no scheduled task installed" }
    }
    try {
      const { $ } = await import("bun")
      await $`schtasks /End /TN ${WINDOWS_TASK_NAME}`.quiet().nothrow()
      await $`schtasks /Delete /TN ${WINDOWS_TASK_NAME} /F`.quiet().nothrow()
    } catch {
      // Best-effort; the saved definition below is the source of truth.
    }
    await fs.rm(xmlPath, { force: true })
    return { removed: true, path: xmlPath, message: `Scheduled task removed: ${WINDOWS_TASK_NAME}` }
  }
  return { removed: false, path: null, message: `autostart is not supported on ${process.platform}` }
}
