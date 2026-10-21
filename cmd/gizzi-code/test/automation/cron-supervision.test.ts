import { describe, expect, test } from "bun:test"
import path from "path"
import { tmpdir } from "../fixture/fixture"
import {
  CRON_DAEMON_LABEL,
  clearPidfile,
  enableSupervision,
  isSupervisableExec,
  launchdPlist,
  launchdPlistPath,
  pidfilePath,
  readLastCrash,
  recordDaemonCrash,
  supervisionState,
  systemdUnitPath,
  systemdUserUnit,
  validatePidfile,
  writePidfile,
} from "../../src/runtime/automation/cron/supervision"

describe("launchdPlist", () => {
  test("generates a valid LaunchAgent plist (string snapshot)", () => {
    expect(
      launchdPlist({
        label: CRON_DAEMON_LABEL,
        program: "/usr/local/bin/gizzi",
        args: ["cron", "start"],
        stdoutLog: "/tmp/gizzi/cron/logs/daemon.out.log",
        stderrLog: "/tmp/gizzi/cron/logs/daemon.err.log",
      }),
    ).toBe(`<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.allternit.gizzi.cron</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/gizzi</string>
    <string>cron</string>
    <string>start</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>/tmp/gizzi/cron/logs/daemon.out.log</string>
  <key>StandardErrorPath</key>
  <string>/tmp/gizzi/cron/logs/daemon.err.log</string>
</dict>
</plist>
`)
  })

  test("plist path lives in ~/Library/LaunchAgents", () => {
    expect(launchdPlistPath("/home/u")).toBe(
      path.join("/home/u", "Library", "LaunchAgents", `${CRON_DAEMON_LABEL}.plist`),
    )
  })
})

describe("systemdUserUnit", () => {
  test("generates a systemd --user unit (string snapshot)", () => {
    expect(
      systemdUserUnit({
        description: "Gizzi cron daemon",
        execStart: "/usr/local/bin/gizzi cron start",
      }),
    ).toBe(`[Unit]
Description=Gizzi cron daemon

[Service]
ExecStart=/usr/local/bin/gizzi cron start
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
`)
  })

  test("unit path lives under ~/.config/systemd/user", () => {
    expect(systemdUnitPath("/home/u")).toBe(
      path.join("/home/u", ".config", "systemd", "user", `${CRON_DAEMON_LABEL}.service`),
    )
  })
})

describe("pidfile lifecycle", () => {
  test("reports none when no pidfile exists", async () => {
    await using tmp = await tmpdir()
    expect(await validatePidfile(path.join(tmp.path, "daemon.pid"))).toEqual({ status: "none" })
  })

  test("reports running when the pid is alive and keeps the file", async () => {
    await using tmp = await tmpdir()
    const pidfile = path.join(tmp.path, "daemon.pid")
    await writePidfile(pidfile, 4242)
    const state = await validatePidfile(pidfile, () => true)
    expect(state).toEqual({ status: "running", pid: 4242 })
    expect(await Bun.file(pidfile).exists()).toBe(true)
  })

  test("removes a stale pidfile and reports stale", async () => {
    await using tmp = await tmpdir()
    const pidfile = path.join(tmp.path, "daemon.pid")
    await writePidfile(pidfile, 4242)
    const state = await validatePidfile(pidfile, () => false)
    expect(state).toEqual({ status: "stale", pid: 4242 })
    expect(await Bun.file(pidfile).exists()).toBe(false)
  })

  test("treats an unparseable pidfile as none and removes it", async () => {
    await using tmp = await tmpdir()
    const pidfile = path.join(tmp.path, "daemon.pid")
    await Bun.write(pidfile, "not-a-pid")
    expect(await validatePidfile(pidfile)).toEqual({ status: "none" })
    expect(await Bun.file(pidfile).exists()).toBe(false)
  })

  test("clearPidfile removes the pidfile", async () => {
    await using tmp = await tmpdir()
    const pidfile = path.join(tmp.path, "daemon.pid")
    await writePidfile(pidfile)
    await clearPidfile(pidfile)
    expect(await Bun.file(pidfile).exists()).toBe(false)
  })
})

describe("crash recording", () => {
  test("records and reads back the last crash", async () => {
    await using tmp = await tmpdir()
    const before = Date.now()
    const record = await recordDaemonCrash(new Error("boom"), tmp.path, 1234)
    expect(record.message).toBe("boom")
    expect(record.pid).toBe(1234)
    expect(Date.parse(record.at)).toBeGreaterThanOrEqual(before)

    const read = await readLastCrash(tmp.path)
    expect(read?.message).toBe("boom")
    expect(read?.stack).toContain("boom")
  })

  test("returns null when no crash has been recorded", async () => {
    await using tmp = await tmpdir()
    expect(await readLastCrash(tmp.path)).toBeNull()
  })
})

describe("supervision state", () => {
  test("reports the installed unit for the current platform", async () => {
    await using tmp = await tmpdir()
    const home = path.join(tmp.path, "home")
    const state = await supervisionState(home)
    expect(state.supported).toBe(process.platform !== "win32")
    if (process.platform === "darwin") {
      expect(state.launchdPlist).toBeNull()
      await Bun.$`mkdir -p ${path.dirname(launchdPlistPath(home))}`
      await Bun.write(launchdPlistPath(home), launchdPlist({
        label: CRON_DAEMON_LABEL,
        program: "/usr/local/bin/gizzi",
        args: ["cron", "start"],
        stdoutLog: "/tmp/out.log",
        stderrLog: "/tmp/err.log",
      }))
      const after = await supervisionState(home)
      expect(after.launchdPlist).toBe(launchdPlistPath(home))
      expect(after.systemdUnit).toBeNull()
    }
  })

  test("isSupervisableExec rejects JS runtimes", () => {
    expect(isSupervisableExec("/Users/u/.bun/bin/bun")).toBe(false)
    expect(isSupervisableExec("/usr/local/bin/node")).toBe(false)
    expect(isSupervisableExec("C:\\opt\\bun\\bin\\bun.exe")).toBe(false)
    expect(isSupervisableExec("C:\\Program Files\\nodejs\\node.exe")).toBe(false)
    expect(isSupervisableExec("/Users/u/.gizzi/bin/gizzi-code")).toBe(true)
    expect(isSupervisableExec("/usr/local/bin/gizzi")).toBe(true)
  })

  test("enableSupervision refuses to supervise a JS runtime", async () => {
    await using tmp = await tmpdir()
    const result = await enableSupervision("/Users/u/.bun/bin/bun", path.join(tmp.path, "home"))
    expect(result.ok).toBe(false)
    expect(result.message).toContain("installed gizzi binary")
  })
})
