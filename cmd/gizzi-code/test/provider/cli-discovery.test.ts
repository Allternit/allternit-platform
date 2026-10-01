import { describe, expect, test } from "bun:test"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { currentCliPath, knownInstallPaths, SUBPROCESS_PROVIDERS } from "../../src/runtime/providers/discovery/subprocess"
import type { Provider } from "../../src/runtime/providers/provider"
import { cliModel, stripProviderPrefix } from "../../src/runtime/providers/cli-model"

function exe(file: string) {
  mkdirSync(path.dirname(file), { recursive: true })
  writeFileSync(file, "#!/bin/sh\necho 1.0.0\n")
  chmodSync(file, 0o755)
}

describe("CLI discovery off PATH", () => {
  test("finds Claude Code's newest versioned binary when the ~/.local/bin launcher is missing", () => {
    const home = mkdtempSync(path.join(os.tmpdir(), "gizzi-home-"))
    const versions = path.join(home, ".local", "share", "claude", "versions")
    exe(path.join(versions, "2.1.9"))
    exe(path.join(versions, "2.1.283"))
    exe(path.join(versions, "2.1.276"))
    mkdirSync(path.join(versions, "3.0.0")) // a directory, not a binary
    const candidates = knownInstallPaths({ id: "claude-cli", bin: "claude" }, home)
    expect(candidates).toContain(path.join(versions, "2.1.283"))
    expect(candidates).not.toContain(path.join(versions, "2.1.9"))
    // The launcher and the legacy local install still come first.
    expect(candidates[0]).toBe(path.join(home, ".local", "bin", "claude"))
  })

  test("CLIs with their own installer dir are found there", () => {
    const home = "/Users/someone"
    expect(knownInstallPaths({ id: "kimi-cli", bin: "kimi" }, home)).toContain(path.join(home, ".kimi-code", "bin", "kimi"))
    expect(knownInstallPaths({ id: "grok", bin: "grok" }, home)).toContain(path.join(home, ".grok", "bin", "grok"))
  })

  test("CLIs installed by allternit-tools are found in ~/.allternit/tools/bin", () => {
    const home = "/Users/someone"
    const prev = process.env.ALLTERNIT_TOOLS_PREFIX
    delete process.env.ALLTERNIT_TOOLS_PREFIX
    try {
      for (const id of ["droid", "opencode", "gemini-cli"]) {
        const spec = SUBPROCESS_PROVIDERS.find((s) => s.id === id)
        expect(spec).toBeDefined()
        expect(knownInstallPaths(spec!, home)).toContain(path.join(home, ".allternit", "tools", "bin", spec!.bin))
      }
    } finally {
      if (prev !== undefined) process.env.ALLTERNIT_TOOLS_PREFIX = prev
    }
  })

  test("every CLI is also looked for in user bin dirs a Finder-launched app's PATH lacks", () => {
    const home = "/Users/someone"
    for (const spec of SUBPROCESS_PROVIDERS.filter((s) => !s.bin.includes("/"))) {
      const candidates = knownInstallPaths(spec, home)
      expect(candidates).toContain(path.join(home, ".local", "bin", spec.bin))
      expect(candidates).toContain(`/opt/homebrew/bin/${spec.bin}`)
    }
  })
})

describe("CLI models the built-in list doesn't have", () => {
  const template = {
    id: "codex-latest",
    providerID: "codex-cli",
    name: "Codex Latest",
    api: { id: "codex-latest", url: "", npm: "" },
    variants: {},
  } as unknown as Provider.Model
  const provider = (over: Partial<Provider.Info> = {}) =>
    ({ id: "codex-cli", name: "Codex CLI", source: "custom", env: [], options: {}, models: { "codex-latest": template }, auth_type: "subprocess", ...over }) as Provider.Info

  test("a CLI provider runs a model id it doesn't list (the CLI checks it)", () => {
    const p = provider()
    const model = cliModel(p, "gpt-6-astra")
    expect(model).toMatchObject({ id: "gpt-6-astra", api: { id: "gpt-6-astra" }, providerID: "codex-cli" })
    expect(p.models["gpt-6-astra"]).toBe(model!)
    expect(template.id).toBe("codex-latest") // the template isn't mutated
  })

  test("API providers still reject unknown models", () => {
    expect(cliModel(provider({ auth_type: undefined, subprocess_cmd: undefined }), "gpt-6-astra")).toBeUndefined()
    expect(cliModel(provider(), "openai/gpt-6")).toBeUndefined()
    expect(cliModel(provider(), "")).toBeUndefined()
  })

  test("a model id that repeats its provider names the model after it", () => {
    expect(stripProviderPrefix("claude-cli", "claude-cli/claude-sonnet-5")).toBe("claude-sonnet-5")
    expect(stripProviderPrefix("openrouter", "anthropic/claude-sonnet-4")).toBe("anthropic/claude-sonnet-4")
    expect(stripProviderPrefix("claude-cli", "claude-sonnet-5")).toBe("claude-sonnet-5")
  })

  test("a CLI path that vanished mid-session is found again", async () => {
    const home = mkdtempSync(path.join(os.tmpdir(), "gizzi-home-"))
    const bin = path.join(home, ".local", "share", "claude", "versions", "2.1.284")
    exe(bin)
    const saved = { HOME: process.env.HOME, PATH: process.env.PATH }
    process.env.HOME = home
    process.env.PATH = path.join(home, "empty-bin")
    try {
      const gone = path.join(home, ".local", "bin", "claude")
      // Found again: the versioned binary, or an earlier PATH hit from the
      // login shell (gizzi caches it) — either way, one that exists.
      const found = await currentCliPath("claude-cli", gone)
      expect(found).not.toBe(gone)
      expect(existsSync(found)).toBe(true)
      expect(await currentCliPath("claude-cli", bin)).toBe(bin)
      expect(await currentCliPath("not-a-cli", gone)).toBe(gone)
    } finally {
      process.env.HOME = saved.HOME
      process.env.PATH = saved.PATH
    }
  })
})
