import { describe, expect, test } from "bun:test"
import { ModelList } from "../../src/runtime/providers/model-list"
import type { DiscoveredProvider } from "../../src/runtime/providers/discovery"

const cloud: DiscoveredProvider = {
  id: "allternit",
  name: "Allternit Cloud",
  auth_type: "api_key",
  source: "platform",
  models: [
    { id: "zeta-fast", name: "Zeta Fast", rank: 0 },
    { id: "allternit-auto", name: "Allternit Auto", rank: 1, default: true },
    { id: "allternit-legacy", name: "Allternit Legacy", rank: 2, hidden: true },
  ],
}

// Discovery order shuffled on purpose: rows must follow the vendor's rank.
const claude: DiscoveredProvider = {
  id: "claude-cli",
  name: "Claude Code",
  auth_type: "subprocess",
  source: "subprocess",
  signedIn: true,
  defaultRank: 1,
  models: [
    { id: "claude-haiku-5-5", name: "Claude Haiku 5.5", rank: 3 },
    { id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5", rank: 2, default: true },
    { id: "claude-fable-5-2", name: "Claude Fable 5.2", rank: 0 },
    { id: "claude-opus-5-5", name: "Claude Opus 5.5", rank: 1 },
  ],
}

const kimi: DiscoveredProvider = {
  id: "kimi-cli",
  name: "Kimi Code",
  auth_type: "subprocess",
  source: "subprocess",
  defaultRank: 3,
  models: [{ id: "kimi-for-coding", name: "Kimi for Coding", rank: 0, default: true }],
}

const cursor: DiscoveredProvider = {
  id: "cursor-agent",
  name: "Cursor Agent",
  auth_type: "subprocess",
  source: "subprocess",
  signedIn: true,
  models: [{ id: "default", name: "Cursor default" }],
}

const ollama: DiscoveredProvider = {
  id: "ollama",
  name: "Ollama",
  auth_type: "none",
  source: "local",
  base_url: "http://127.0.0.1:11434/v1",
  models: [
    { id: "qwen3:8b", name: "qwen3:8b" },
    { id: "nomic-embed-text", name: "nomic-embed-text" },
  ],
}

const model = (id: string, name = id) => ({ id, name, limit: { context: 128000, output: 8192 } })

const configured: Record<string, ModelList.ConfiguredProvider> = {
  // The provider state merges discovered Cloud models in: one entry, not two.
  allternit: {
    id: "allternit",
    name: "Allternit Cloud",
    source: "custom",
    env: ["ALLTERNIT_API_KEY"],
    options: {},
    models: {
      "zeta-fast": model("zeta-fast", "Zeta Fast"),
      "allternit-auto": model("allternit-auto", "Allternit Auto"),
      "allternit-legacy": model("allternit-legacy", "Allternit Legacy"),
    },
  },
  openai: {
    id: "openai",
    name: "OpenAI",
    source: "config",
    key: "sk-test",
    env: [],
    options: {},
    models: {
      "gpt-6.1-sol": model("gpt-6.1-sol"),
      "text-embedding-3-large": model("text-embedding-3-large"),
    },
  },
  anthropic: {
    id: "anthropic",
    name: "Anthropic",
    source: "custom",
    env: ["GIZZI_TEST_UNSET_ANTHROPIC_KEY"],
    options: {},
    models: { "claude-sonnet-5-5": model("claude-sonnet-5-5") },
  },
  openrouter: {
    id: "openrouter",
    name: "OpenRouter",
    source: "config",
    key: "sk-or-test",
    env: [],
    options: {},
    models: { "openai/gpt-6.1-sol": model("openai/gpt-6.1-sol") },
  },
}

const discovered = [ollama, cursor, kimi, cloud, claude]

function compose(extra: Partial<ModelList.ComposeInput> = {}) {
  return ModelList.compose({ discovered, configured, currentRef: "claude-cli/claude-sonnet-5-5", ...extra })
}

function provider(sections: ModelList.Section[], id: string) {
  return sections.flatMap((s) => s.providers).find((p) => p.id === id)
}

describe("ModelList.compose", () => {
  test("sections follow the picker order", () => {
    expect(compose().map((s) => s.kind)).toEqual(["cloud", "subscription", "api", "local"])
  })

  test("ranked CLIs come first in rank order, then the rest", () => {
    const subs = compose().find((s) => s.kind === "subscription")!
    expect(subs.providers.map((p) => p.id)).toEqual(["claude-cli", "kimi-cli", "cursor-agent"])
    expect(subs.providers[0]!.signedIn).toBe(true)
  })

  test("models keep the vendor's order, not alphabetical", () => {
    const rows = provider(compose(), "claude-cli")!.models
    expect(rows.map((r) => r.modelID)).toEqual([
      "claude-fable-5-2",
      "claude-opus-5-5",
      "claude-sonnet-5-5",
      "claude-haiku-5-5",
    ])
  })

  test("default and current flags", () => {
    const rows = provider(compose(), "claude-cli")!.models
    expect(rows.filter((r) => r.default).map((r) => r.ref)).toEqual(["claude-cli/claude-sonnet-5-5"])
    expect(rows.filter((r) => r.current).map((r) => r.ref)).toEqual(["claude-cli/claude-sonnet-5-5"])
    const cloudRows = provider(compose(), "allternit")!.models
    expect(cloudRows.find((r) => r.default)?.modelID).toBe("allternit-auto")
    expect(cloudRows.some((r) => r.current)).toBe(false)
  })

  test("a provider found by discovery and in the config is one entry", () => {
    const cloudSection = compose().find((s) => s.kind === "cloud")!
    expect(cloudSection.providers.map((p) => p.id)).toEqual(["allternit"])
  })

  test("embedding models are not rows", () => {
    const refs = compose().flatMap((s) => s.providers.flatMap((p) => p.models.map((r) => r.ref)))
    expect(refs).not.toContain("openai/text-embedding-3-large")
    expect(refs).not.toContain("ollama/nomic-embed-text")
    expect(refs).toContain("ollama/qwen3:8b")
  })

  test("openrouter is never offered; unconnected API providers are hidden", () => {
    const api = compose().find((s) => s.kind === "api")!
    expect(api.providers.map((p) => p.id)).toEqual(["openai"])
    const everything = compose({ all: true })
    expect(provider(everything, "openrouter")).toBeDefined()
    expect(provider(everything, "anthropic")).toBeDefined()
  })

  test("hidden models are rows only when running or with all", () => {
    const ids = (sections: ModelList.Section[]) => provider(sections, "allternit")!.models.map((r) => r.modelID)
    expect(ids(compose())).toEqual(["zeta-fast", "allternit-auto"])

    const running = provider(compose({ currentRef: "allternit/allternit-legacy" }), "allternit")!.models
    expect(running.map((r) => r.modelID)).toEqual(["zeta-fast", "allternit-auto", "allternit-legacy"])
    expect(running[2]).toMatchObject({ current: true, hidden: true })

    expect(ids(compose({ all: true }))).toEqual(["zeta-fast", "allternit-auto", "allternit-legacy"])
  })
})
