import { describe, expect, test } from "bun:test"
import path from "path"
import { planUpdate } from "../../src/cli/commands/upgrade"
import { checkUpdates, findExtraGizzis, removeCommandFor } from "../../src/cli/commands/doctorChecks"
import { planDesktopUninstall } from "../../src/cli/commands/uninstallPlan"

describe("gizzi update --dry-run plan", () => {
  test("desktop is managed by Desktop", () => {
    const plan = planUpdate("desktop")
    expect(plan.kind).toBe("managed")
    expect(plan.line).toContain("Allternit Desktop manages this copy")
  })
  test("brew prints the exact command", () => {
    const plan = planUpdate("brew", "1.2.3")
    expect(plan).toMatchObject({ kind: "command", command: "brew upgrade gizzi-code" })
    expect(plan.line).toContain("brew")
  })
  test("npm and curl name the method and would apply", () => {
    const npm = planUpdate("npm", "1.2.3")
    expect(npm.kind).toBe("apply")
    expect(npm.line).toContain("Method npm")
    const curl = planUpdate("curl", "1.2.3")
    expect(curl.kind).toBe("apply")
    expect(curl.line).toContain("Method curl")
  })
})

describe("doctor update lines", () => {
  const base = {
    method: "curl",
    channel: "stable",
    state: {
      lastCheckAt: "2026-11-27T10:00:00Z",
      lastCheckResult: "ok",
      latestVersion: "2.4.0",
      minVersion: "2.0.0",
      lastUpdateAt: "2026-11-20T10:00:00Z",
      lastUpdateFrom: "2.3.0",
      lastUpdateTo: "2.3.1",
    },
    keyMissing: false,
    current: "2.3.1",
    extras: [],
  }
  test("shows method, channel, last check, last update, latest/required, key", () => {
    const text = checkUpdates(base).map((c) => c.message).join("\n")
    expect(text).toContain("Install method: curl")
    expect(text).toContain("Channel: stable")
    expect(text).toContain("Last check: 2026-11-27T10:00:00Z, ok")
    expect(text).toContain("Last update: 2.3.0 -> 2.3.1")
    expect(text).toContain("Latest version: 2.4.0, required: 2.0.0")
    expect(text).toContain("Feed key: set")
    expect(text).not.toContain("not checked yet")
  })
  test("lists a duplicate PATH binary with its remove command", () => {
    const files: Record<string, string> = {
      "/a/bin/gizzi": "/a/real/gizzi",
      "/b/bin/gizzi": "/b/lib/node_modules/@allternit/gizzi-code/bin/gizzi",
      "/c/bin/gizzi": "/a/real/gizzi",
      "/d/bin/gizzi": "/opt/homebrew/Cellar/gizzi-code/1/bin/gizzi",
      "/e/bin/gizzi": "/e/bin/gizzi",
    }
    const extras = findExtraGizzis({
      pathEnv: "/a/bin:/b/bin:/c/bin:/d/bin:/e/bin",
      execPath: "/a/real/gizzi",
      platform: "linux",
      exists: (p) => p in files,
      realpath: (p) => files[p] ?? p,
    })
    expect(extras.map((e) => e.path)).toEqual(["/b/bin/gizzi", "/d/bin/gizzi", "/e/bin/gizzi"])
    expect(extras.map((e) => e.removeCommand)).toEqual([
      "npm rm -g @allternit/gizzi-code",
      "brew uninstall gizzi-code",
      "rm /e/bin/gizzi",
    ])
    const lines = checkUpdates({ ...base, extras }).filter((c) => c.id.startsWith("update-extra:"))
    expect(lines).toHaveLength(3)
    expect(lines[2].message).toContain("Remove it with: rm /e/bin/gizzi")
  })
  test("removeCommandFor", () => {
    expect(removeCommandFor("/x/gizzi", "/x/gizzi")).toBe("rm /x/gizzi")
  })
})

describe("uninstall on a Desktop copy", () => {
  const app = "/Applications/Allternit.app/Contents/Resources/bin/gizzi-code"
  test("only the ~/.local/bin symlink is ever a target", () => {
    const link = "/home/u/.local/bin/gizzi"
    const plan = planDesktopUninstall({
      execPath: link,
      argv1: link,
      home: "/home/u",
      env: {},
      realpath: (p) => (p === link ? app : p),
      isSymlink: (p) => p === link,
    })
    expect(plan.desktop).toBe(true)
    if (!plan.desktop) return
    expect(plan.link).toBe(link)
    expect(plan.realPath).toBe(app)
    expect(plan.link!.includes(".app")).toBe(false)
    expect(path.dirname(plan.link!)).toBe("/home/u/.local/bin")
  })
  test("no link target when ~/.local/bin/gizzi is not a symlink", () => {
    const plan = planDesktopUninstall({
      execPath: app,
      argv1: app,
      home: "/home/u",
      env: {},
      realpath: (p) => p,
      isSymlink: () => false,
    })
    expect(plan).toMatchObject({ desktop: true, link: undefined })
  })
  test("a normal curl install is not Desktop", () => {
    const plan = planDesktopUninstall({
      execPath: "/home/u/.local/bin/gizzi",
      argv1: "/home/u/.local/bin/gizzi",
      home: "/home/u",
      env: {},
      realpath: (p) => p,
    })
    expect(plan.desktop).toBe(false)
  })
})
