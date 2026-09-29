import { describe, expect, test } from "bun:test"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { refreshCliPath } from "@/runtime/drivers/local-cli-driver"

describe("refreshCliPath", () => {
  test("a CLI whose launcher link vanished is found again at its current install", async () => {
    const home = mkdtempSync(path.join(os.tmpdir(), "gizzi-clipath-"))
    const versions = path.join(home, ".local", "share", "claude", "versions")
    mkdirSync(versions, { recursive: true })
    const bin = path.join(versions, "9.9.9")
    writeFileSync(bin, "#!/bin/sh\necho 9.9.9\n")
    chmodSync(bin, 0o755)
    const prevHome = process.env.HOME
    process.env.HOME = home
    try {
      // Discovery saved the native installer's link, which no longer exists.
      const cli = { name: "claude-cli", path: path.join(home, ".local", "bin", "claude") }
      await refreshCliPath(cli)
      expect(cli.path).not.toBe(path.join(home, ".local", "bin", "claude"))
      expect(existsSync(cli.path)).toBe(true)
    } finally {
      process.env.HOME = prevHome
    }
  })

  test("an existing path is left alone", async () => {
    const cli = { name: "claude-cli", path: process.execPath }
    await refreshCliPath(cli)
    expect(cli.path).toBe(process.execPath)
  })
})
