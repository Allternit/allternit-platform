import { afterEach, describe, expect, test } from "bun:test"
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import {
  clearLiveModelCache,
  liveModels,
  LIVE_MODELS_TTL_MS,
  parseCodexModels,
  parseGrokModels,
  parseOllamaModels,
  parseTsvModels,
} from "../../src/runtime/providers/discovery/live-models"
import { cliSignedIn } from "../../src/runtime/providers/discovery/sign-in"
import { SUBPROCESS_PROVIDERS } from "../../src/runtime/providers/discovery/subprocess"

// Built from the real `codex debug models` output (field names and values as
// Codex prints them; the other ~40 fields per model are left out).
const CODEX_JSON = JSON.stringify({
  models: [
    { slug: "gpt-6-luna", display_name: "GPT-6-Luna", context_window: 272000, visibility: "list", priority: 6 },
    { slug: "gpt-reserve", display_name: "GPT-Reserve", context_window: 272000, visibility: "hide", priority: 5 },
    { slug: "gpt-6.1-sol", display_name: "GPT-6.1-Sol", context_window: 500000, visibility: "list", priority: 1 },
    { slug: "codex-auto-review", display_name: "Codex Auto Review", context_window: 272000, visibility: "hide", priority: 10 },
    { slug: "gpt-6-astra", display_name: "GPT-6-Astra", context_window: 272000, visibility: "list", priority: 2 },
    { slug: "gpt-6.1-terra", display_name: "GPT-6.1 Terra", context_window: 500000, visibility: "list", priority: 3 },
  ],
})

// `agy models`, verbatim.
const AGY_MODELS = [
  "Fetching available models...",
  "gemini-3.8-flash-high\tGemini 3.8 Flash (High)",
  "gemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)",
  "gemini-3.8-flash-low\tGemini 3.8 Flash (Low)",
  "claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)",
  "claude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)",
  "gpt-oss-120b-medium\tGPT-OSS 120B (Medium)",
  "",
].join("\n")

// `grok models`, verbatim (signed out).
const GROK_MODELS = "You are not authenticated.\n\nDefault model: grok-4.6\n\nAvailable models:\n  * grok-4.6 (default)\n  - grok-4.5\n"

const OLLAMA_LIST = "NAME               ID              SIZE      MODIFIED    \nllama3.2:latest    a80c4f17acd5    2.0 GB    8 weeks ago    \n"

const spec = (id: string) => SUBPROCESS_PROVIDERS.find((s) => s.id === id)!

describe("live model parsers", () => {
  test("codex: hidden models are dropped and the rest follow Codex's priority", () => {
    const rows = parseCodexModels(CODEX_JSON)
    expect(rows.map((r) => r.id)).toEqual(["gpt-6.1-sol", "gpt-6-astra", "gpt-6.1-terra", "gpt-6-luna"])
    expect(rows.map((r) => r.id)).not.toContain("gpt-reserve")
    expect(rows.map((r) => r.id)).not.toContain("codex-auto-review")
    expect(rows[0]).toEqual({ id: "gpt-6.1-sol", name: "GPT-6.1-Sol", context: 500000, output: 32768 })
  })

  test("agy: one row per id<TAB>name line, the status line skipped", () => {
    const rows = parseTsvModels(AGY_MODELS)
    expect(rows.map((r) => r.id)).toEqual([
      "gemini-3.8-flash-high",
      "gemini-3.8-flash-medium",
      "gemini-3.8-flash-low",
      "claude-sonnet-4-6",
      "claude-opus-4-6-thinking",
      "gpt-oss-120b-medium",
    ])
    expect(rows[3]).toEqual({ id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6 (Thinking)" })
  })

  test("grok: bullets under Available models, (default) marks the default", () => {
    expect(parseGrokModels(GROK_MODELS)).toEqual([
      { id: "grok-4.6", name: "grok-4.6", default: true },
      { id: "grok-4.5", name: "grok-4.5" },
    ])
    expect(parseGrokModels("You are not authenticated.\n")).toEqual([])
  })

  test("ollama: model names from `ollama list`", () => {
    expect(parseOllamaModels(OLLAMA_LIST)).toEqual([{ id: "llama3.2:latest", name: "llama3.2:latest", context: 128000, output: 8192 }])
  })
})

/** A fake CLI that logs each run to `calls` and prints `outFile` (or fails). */
function fakeCli(dir: string, outFile: string | null) {
  const bin = path.join(dir, "cli")
  const calls = path.join(dir, "calls")
  writeFileSync(
    bin,
    outFile ? `#!/bin/sh\necho run >> "${calls}"\ncat "${outFile}"\n` : `#!/bin/sh\necho run >> "${calls}"\nexit 1\n`,
  )
  chmodSync(bin, 0o755)
  const runs = () => (existsSync(calls) ? readFileSync(calls, "utf8").trim().split("\n").length : 0)
  return { bin, runs }
}

describe("liveModels", () => {
  const saved = process.env.GIZZI_MODELS_REFRESH
  afterEach(() => {
    if (saved === undefined) delete process.env.GIZZI_MODELS_REFRESH
    else process.env.GIZZI_MODELS_REFRESH = saved
  })

  function setup(output: string | null) {
    const dir = mkdtempSync(path.join(os.tmpdir(), "gizzi-live-"))
    const out = path.join(dir, "out.txt")
    if (output !== null) writeFileSync(out, output)
    return { dir, cacheFile: path.join(dir, "cache", "cli-models.json"), ...fakeCli(dir, output === null ? null : out) }
  }

  test("codex: live order, catalog limits and the catalog default", async () => {
    const t = setup(CODEX_JSON)
    const models = await liveModels(spec("codex-cli"), t.bin, { cacheFile: t.cacheFile })
    expect(models.map((m) => [m.id, m.rank])).toEqual([
      ["gpt-6.1-sol", 0],
      ["gpt-6-astra", 1],
      ["gpt-6.1-terra", 2],
      ["gpt-6-luna", 3],
    ])
    expect(models.filter((m) => m.default).map((m) => m.id)).toEqual(["gpt-6.1-sol"])
    expect(models[1]).toMatchObject({ context: 272000, output: 32768 })
  })

  test("grok: the CLI's own (default) marker wins; a live list without the catalog default flags its first row", async () => {
    const grok = await liveModels(spec("grok"), setup(GROK_MODELS).bin, { cacheFile: setup(null).cacheFile })
    expect(grok.filter((m) => m.default).map((m) => m.id)).toEqual(["grok-4.6"])
    expect(grok.find((m) => m.id === "grok-4.5")).toMatchObject({ context: 200000, output: 64000, rank: 1 })

    const t = setup("Fetching available models...\nnew-model\tNew\ngemini-3.8-flash-low\tLow\n")
    const agy = await liveModels(spec("antigravity"), t.bin, { cacheFile: t.cacheFile })
    expect(agy.map((m) => [m.id, !!m.default])).toEqual([
      ["new-model", true],
      ["gemini-3.8-flash-low", false],
    ])
    expect(agy[1]).toMatchObject({ context: 1000000, output: 65536 })
  })

  test("a fresh cache entry is used without running the CLI; GIZZI_MODELS_REFRESH=1 runs it", async () => {
    const t = setup(AGY_MODELS)
    let clock = 1_000_000
    const opts = { cacheFile: t.cacheFile, now: () => clock }
    await liveModels(spec("antigravity"), t.bin, opts)
    expect(t.runs()).toBe(1)
    clock += LIVE_MODELS_TTL_MS - 1
    const again = await liveModels(spec("antigravity"), t.bin, opts)
    expect(t.runs()).toBe(1)
    expect(again).toHaveLength(6)
    process.env.GIZZI_MODELS_REFRESH = "1"
    await liveModels(spec("antigravity"), t.bin, opts)
    expect(t.runs()).toBe(2)
  })

  test("a failed live call falls back to the stale cache entry, then to the catalog", async () => {
    const good = setup(GROK_MODELS)
    let clock = 0
    await liveModels(spec("grok"), good.bin, { cacheFile: good.cacheFile, now: () => clock })

    // The same CLI path now fails, after the entry went stale.
    writeFileSync(good.bin, "#!/bin/sh\nexit 1\n")
    clock += LIVE_MODELS_TTL_MS + 1
    const stale = await liveModels(spec("grok"), good.bin, { cacheFile: good.cacheFile, now: () => clock })
    expect(stale.map((m) => m.id)).toEqual(["grok-4.6", "grok-4.5"])

    clearLiveModelCache(good.cacheFile)
    const fallback = await liveModels(spec("grok"), good.bin, { cacheFile: good.cacheFile, now: () => clock })
    expect(fallback).toEqual(spec("grok").models)

    // Ollama has no static list: nothing at all.
    const none = setup(null)
    expect(await liveModels(spec("ollama-cli"), none.bin, { cacheFile: none.cacheFile })).toEqual([])
  })
})

describe("cliSignedIn", () => {
  function home(...files: string[][]) {
    const dir = mkdtempSync(path.join(os.tmpdir(), "gizzi-signin-"))
    for (const parts of files) {
      mkdirSync(path.join(dir, ...parts.slice(0, -1)), { recursive: true })
      writeFileSync(path.join(dir, ...parts), "{}")
    }
    return dir
  }
  // linux: file rules only (the macOS Keychain rule has its own test below).
  const check = (id: string, h: string, env: NodeJS.ProcessEnv = {}) =>
    cliSignedIn(spec(id), "/nonexistent", { home: h, env, platform: "linux" })

  test("each CLI's sign-in file, or its API key", async () => {
    const empty = home()
    for (const id of ["codex-cli", "kimi-cli", "grok", "antigravity", "qwen-cli"]) {
      expect(await check(id, empty)).toBe(false)
    }
    expect(await check("codex-cli", home([".codex", "auth.json"]))).toBe(true)
    expect(await check("codex-cli", empty, { OPENAI_API_KEY: "sk" })).toBe(true)
    const codexHome = home(["auth.json"])
    expect(await check("codex-cli", empty, { CODEX_HOME: codexHome })).toBe(true)
    expect(await check("kimi-cli", home([".kimi", "credentials", "kimi-code.json"]))).toBe(true)
    expect(await check("kimi-cli", home([".kimi-code", "config.json"]))).toBe(true)
    expect(await check("grok", home([".grok", "auth.json"]))).toBe(true)
    expect(await check("grok", empty, { XAI_API_KEY: "x" })).toBe(true)
    expect(await check("antigravity", home([".gemini", "antigravity-cli", "credentials.enc"]))).toBe(true)
    expect(await check("qwen-cli", home([".config", "qwen", "oauth_creds.json"]))).toBe(true)
  })

  test("qwen: an API-key setup picked in its settings counts as signed in", async () => {
    const dir = home()
    mkdirSync(path.join(dir, ".qwen"), { recursive: true })
    writeFileSync(path.join(dir, ".qwen", "settings.json"), JSON.stringify({ security: { auth: { selectedType: "openai" } } }))
    expect(await check("qwen-cli", dir)).toBe(true)
    writeFileSync(path.join(dir, ".qwen", "settings.json"), JSON.stringify({ ui: {} }))
    expect(await check("qwen-cli", dir)).toBe(false)
  })

  test("antigravity on macOS: the Keychain item is the sign-in (no credentials file)", async () => {
    const dir = mkdtempSync(path.join(os.tmpdir(), "gizzi-agy-"))
    const security = path.join(dir, "security")
    const run = (exit: number, cacheName: string) => {
      writeFileSync(security, `#!/bin/sh\nexit ${exit}\n`)
      chmodSync(security, 0o755)
      return cliSignedIn(spec("antigravity"), "/nonexistent", {
        home: home(),
        env: {},
        platform: "darwin",
        securityBin: security,
        cacheFile: path.join(dir, cacheName),
      })
    }
    expect(await run(0, "found.json")).toBe(true)
    expect(await run(44, "missing.json")).toBe(false)
  })

  test("catalog auth blocks count; CLIs without a rule are unknown", async () => {
    const empty = home()
    expect(await check("gemini-cli", empty)).toBe(false)
    expect(await check("gemini-cli", home([".gemini", "oauth_creds.json"]))).toBe(true)
    expect(await check("gemini-cli", empty, { GEMINI_API_KEY: "" })).toBe(false)
    expect(await check("gemini-cli", empty, { GEMINI_API_KEY: "k" })).toBe(true)
    expect(await check("cursor-agent", empty)).toBeUndefined()
  })

  test("claude: `auth status` loggedIn, cached for 10 minutes", async () => {
    const dir = mkdtempSync(path.join(os.tmpdir(), "gizzi-claude-"))
    const out = path.join(dir, "status.json")
    writeFileSync(out, JSON.stringify({ loggedIn: false }))
    const cli = fakeCli(dir, out)
    let clock = 0
    const opts = { home: dir, env: { PATH: process.env.PATH }, cacheFile: path.join(dir, "cache.json"), now: () => clock }
    expect(await cliSignedIn(spec("claude-cli"), cli.bin, opts)).toBe(false)
    writeFileSync(out, JSON.stringify({ loggedIn: true, email: "a@b.c" }))
    expect(await cliSignedIn(spec("claude-cli"), cli.bin, opts)).toBe(false) // cached
    expect(cli.runs()).toBe(1)
    clock += 10 * 60 * 1000 + 1
    expect(await cliSignedIn(spec("claude-cli"), cli.bin, opts)).toBe(true)
    expect(cli.runs()).toBe(2)
  })
})
