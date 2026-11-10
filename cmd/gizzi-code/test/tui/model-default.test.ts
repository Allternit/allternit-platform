// Run with: bun test --preload ./test/preload.ts test/tui/model-default.test.ts
// The preload points XDG_CONFIG_HOME at a temp dir, so the runtime config
// files written here are the ones Config.readGlobalConfigSync() reads.
import { afterEach, beforeAll, describe, expect, test } from "bun:test"
import fs from "fs"
import path from "path"
import { Global } from "../../src/runtime/context/global"
import { CLI_CATALOG } from "../../src/runtime/providers/discovery/subprocess"
import {
  getDefaultMainLoopModelSetting,
  getUserSpecifiedModelSetting,
} from "../../src/cli/ui/ink-app/utils/model/model"

const files = ["config.json", "gizzi.json", "gizzi.jsonc", "config.toml"]

function writeConfig(name: string, body: unknown) {
  fs.mkdirSync(Global.Path.config, { recursive: true })
  fs.writeFileSync(path.join(Global.Path.config, name), JSON.stringify(body))
}

function clearConfig() {
  for (const name of files) fs.rmSync(path.join(Global.Path.config, name), { force: true })
}

beforeAll(() => {
  // No Anthropic credentials: the TUI used to answer "antigravity/default" here.
  delete process.env.ANTHROPIC_API_KEY
  delete process.env.ANTHROPIC_AUTH_TOKEN
  delete process.env.GIZZI_CODE_OAUTH_TOKEN
  delete process.env.ANTHROPIC_MODEL
})

afterEach(clearConfig)

describe("TUI default model", () => {
  test("a mistaken nail-35 default is neither the user's model nor the default", () => {
    writeConfig("config.json", { model: "omlx/Nail-35b-a3b" })
    writeConfig("gizzi.json", { model_auto: false })

    expect(getUserSpecifiedModelSetting()).toBeUndefined()
    const model = getDefaultMainLoopModelSetting()
    expect(model).not.toBe("antigravity/default")
    expect(model.toLowerCase()).not.toContain("nail-35")
    // Before discovery answers, the catalog's best-ranked CLI default stands in.
    const first = CLI_CATALOG.filter((t) => !t.custom && t.defaultRank !== undefined).sort(
      (a, b) => a.defaultRank! - b.defaultRank!,
    )[0]!
    const flagged = first.models!.find((m) => m.default)!
    expect(model).toBe(`${first.id}/${flagged.id}`)
  })

  test("a pinned retired model resolves to its current id", () => {
    writeConfig("gizzi.json", { model: "claude-cli/claude-fable-5-1", model_auto: false })
    expect(getUserSpecifiedModelSetting()).toBe("claude-cli/claude-fable-5-2")
  })

  test("an automatic pick is not the user's model", () => {
    writeConfig("gizzi.json", { model: "codex-cli/gpt-6.1-sol", model_auto: true })
    expect(getUserSpecifiedModelSetting()).toBeUndefined()
  })
})

describe("GIZZI_CONFIG_DIR", () => {
  test("its gizzi.json is merged over the global config for the TUI", async () => {
    const { mkdtempSync, writeFileSync, rmSync } = await import("node:fs")
    const { tmpdir } = await import("node:os")
    const { join } = await import("node:path")
    const { Config } = await import("../../src/runtime/context/config/config")
    const dir = mkdtempSync(join(tmpdir(), "gizzi-config-dir-"))
    const saved = process.env.GIZZI_CONFIG_DIR
    try {
      writeFileSync(join(dir, "gizzi.json"), JSON.stringify({ model: "local/big", small_model: "local/small" }))
      process.env.GIZZI_CONFIG_DIR = dir
      expect(Config.configDirOverrideFiles()).toContain(join(dir, "gizzi.json"))
      const merged = Config.readGlobalConfigSync()
      expect(merged.model).toBe("local/big")
      expect(merged.small_model).toBe("local/small")
    } finally {
      if (saved === undefined) delete process.env.GIZZI_CONFIG_DIR
      else process.env.GIZZI_CONFIG_DIR = saved
      rmSync(dir, { recursive: true, force: true })
    }
    expect(Config.configDirOverrideFiles().some((file) => file.startsWith(dir))).toBe(false)
  })
})
