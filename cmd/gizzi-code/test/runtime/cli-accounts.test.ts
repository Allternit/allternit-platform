import { afterEach, beforeEach, describe, expect, spyOn, test } from "bun:test"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { CliAccounts } from "@/runtime/drivers/cli-accounts"

describe("CliAccounts.env", () => {
  let home: string
  let spy: ReturnType<typeof spyOn>
  beforeEach(() => {
    home = fs.mkdtempSync(path.join(os.tmpdir(), "cli-accounts-"))
    spy = spyOn(os, "homedir").mockReturnValue(home)
  })
  afterEach(() => {
    spy.mockRestore()
    fs.rmSync(home, { recursive: true, force: true })
  })

  function account(id: string, tool?: string) {
    const dir = path.join(home, ".allternit", "cli-homes", id)
    fs.mkdirSync(dir, { recursive: true })
    if (tool) fs.writeFileSync(path.join(dir, ".allternit-cli-tool"), tool)
    return dir
  }

  test("a bot's account for this CLI sets its home", () => {
    const dir = account("ca_claude1", "claude-cli")
    expect(CliAccounts.env("claude-cli", undefined, "ca_claude1")).toEqual({ CLAUDE_CONFIG_DIR: dir })
  })

  test("another CLI's account is skipped (brain switched Claude → Codex)", () => {
    account("ca_claude1", "claude-cli")
    expect(CliAccounts.env("codex-cli", undefined, "ca_claude1")).toBeUndefined()
  })

  test("default and unknown ids keep the stock home", () => {
    expect(CliAccounts.env("claude-cli", "default")).toBeUndefined()
    expect(CliAccounts.env("claude-cli", undefined, "ca_missing")).toBeUndefined()
    expect(CliAccounts.env("claude-cli", "../escape")).toBeUndefined()
  })
})
