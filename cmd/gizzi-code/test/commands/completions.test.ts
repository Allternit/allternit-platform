import { describe, test, expect } from "bun:test"
import path from "path"
import {
  commandEntries,
  bashCompletions,
  zshCompletions,
  fishCompletions,
} from "../../src/cli/commands/completions"
import { COMMANDS } from "../../src/cli/commands/registry"

const REPO_ROOT = path.resolve(import.meta.dir, "../..")

const SAMPLE_COMMANDS = ["doctor", "serve", "auth", "agents", "orchestration", "workflows", "workspace"]

describe("completions generator derives from the yargs command registry", () => {
  test("commandEntries includes a sample of real command names", () => {
    const names = commandEntries().map((e) => e.name)
    for (const name of SAMPLE_COMMANDS) {
      expect(names).toContain(name)
    }
  })

  test("commandEntries has no duplicates and no $0 entries", () => {
    const names = commandEntries().map((e) => e.name)
    expect(new Set(names).size).toBe(names.length)
    expect(names).not.toContain("$0")
  })

  test("every registered module contributes its top-level command", () => {
    const names = commandEntries().map((e) => e.name)
    for (const mod of COMMANDS) {
      const raw = Array.isArray(mod.command) ? mod.command[0] : mod.command
      const name = raw.split(/\s+/)[0]
      if (name === "$0") continue
      if (mod.describe === false) {
        expect(names).not.toContain(name)
        continue
      }
      expect(names).toContain(name)
    }
  })

  test("generated scripts contain the sample commands", () => {
    for (const script of [bashCompletions(), zshCompletions(), fishCompletions()]) {
      for (const name of SAMPLE_COMMANDS) {
        expect(script).toContain(name)
      }
    }
  })
})

describe("gizzi completions <shell>", () => {
  for (const shell of ["bash", "zsh", "fish"] as const) {
    test(`gizzi completions ${shell} exits 0 and lists real commands`, async () => {
      const proc = Bun.spawn(["bun", "src/cli/main.ts", "completions", shell], {
        cwd: REPO_ROOT,
        stdout: "pipe",
        stderr: "pipe",
      })
      const [stdout, exitCode] = await Promise.all([
        new Response(proc.stdout).text(),
        proc.exited,
      ])
      expect(exitCode).toBe(0)
      for (const name of SAMPLE_COMMANDS) {
        expect(stdout).toContain(name)
      }
    }, 90000)
  }
})
