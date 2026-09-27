import { afterEach, describe, expect, test } from "bun:test"
import { homedir } from "os"
import { join } from "path"
import { getGlobalClaudeFile as tuiGlobalFile } from "../../src/cli/ui/ink-app/utils/env"
import { getGizziConfigHomeDir as tuiHome } from "../../src/cli/ui/ink-app/utils/envUtils"
import { getGlobalClaudeFile as sharedGlobalFile } from "../../src/shared/utils/env"
import { getGizziConfigHomeDir as sharedHome } from "../../src/shared/utils/envUtils"

const original = process.env.GIZZI_CONFIG_DIR

function reset() {
  for (const fn of [tuiGlobalFile, sharedGlobalFile, tuiHome, sharedHome] as any[]) fn.cache?.clear?.()
}

afterEach(() => {
  if (original === undefined) delete process.env.GIZZI_CONFIG_DIR
  else process.env.GIZZI_CONFIG_DIR = original
  reset()
})

describe("global config path", () => {
  test("lives in the gizzi home, never Claude Code's ~/.claude.json", () => {
    process.env.GIZZI_CONFIG_DIR = "/tmp/gizzi-home-test"
    reset()
    for (const file of [tuiGlobalFile(), sharedGlobalFile()]) {
      expect(file).toBe(join("/tmp/gizzi-home-test", ".config.json"))
      expect(file).not.toBe(join(homedir(), ".claude.json"))
    }
  })

  test("defaults to ~/.gizzi/.config.json", () => {
    delete process.env.GIZZI_CONFIG_DIR
    reset()
    for (const file of [tuiGlobalFile(), sharedGlobalFile()]) {
      expect(file).toBe(join(homedir(), ".gizzi", ".config.json"))
    }
  })
})
