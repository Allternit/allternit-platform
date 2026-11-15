import { describe, test, expect } from "bun:test"
import { ProviderOffer } from "../../src/runtime/providers/offer"
import { buildPickerRows } from "../../src/cli/ui/ink-app/utils/model/modelPickerModel"

describe("ProviderOffer (the picker rule Desktop uses)", () => {
  test("OpenRouter is never a standalone entry, even with a key", () => {
    expect(ProviderOffer.isOffered({ id: "openrouter", source: "config" }, { connected: true })).toBe(false)
    expect(ProviderOffer.isOffered({ id: "openrouter-byok" }, { connected: true })).toBe(false)
  })

  test("Cloud, CLI sign-ins, Subscription Fabric and local runtimes are always offered", () => {
    expect(ProviderOffer.isOffered({ id: "allternit" })).toBe(true)
    expect(ProviderOffer.kind({ id: "claude-cli" })).toBe("subscription")
    expect(ProviderOffer.isOffered({ id: "codex-cli" })).toBe(true)
    expect(ProviderOffer.kind({ id: "subs-claude", options: { runtime: "fabric" } })).toBe("subscription")
    expect(ProviderOffer.kind({ id: "local-mlx", baseURL: "http://127.0.0.1:8080/v1" })).toBe("local")
    expect(ProviderOffer.kind({ id: "ollama", source: "subprocess" })).toBe("local")
  })

  test("an API provider is offered only once connected", () => {
    expect(ProviderOffer.kind({ id: "deepseek", source: "env" })).toBe("api")
    expect(ProviderOffer.isOffered({ id: "deepseek", source: "env" })).toBe(false)
    expect(ProviderOffer.isOffered({ id: "deepseek", source: "env" }, { connected: true })).toBe(true)
  })

  test("CLI brains come from the CLI catalog, not a fixed list", () => {
    expect(ProviderOffer.isCliBrain("claude-cli")).toBe(true)
    expect(ProviderOffer.isCliBrain("kimi-cli")).toBe(true)
    expect(ProviderOffer.isCliBrain("allternit")).toBe(false)
    expect(ProviderOffer.isCliBrain("openrouter")).toBe(false)
  })
})

describe("TUI picker sections", () => {
  test("sections follow Desktop: Cloud, subscriptions, API keys, local — whatever the input order", () => {
    const meta: Record<string, any> = {
      "local-mlx/q": { providerId: "local-mlx", providerName: "Local MLX", source: "local", kind: "local" },
      "deepseek/v4": { providerId: "deepseek", providerName: "DeepSeek", source: "api", kind: "api" },
      "claude-cli/claude-sonnet-5": { providerId: "claude-cli", providerName: "Claude Code", source: "subprocess", kind: "subscription" },
      "allternit/kimi-k3": { providerId: "allternit", providerName: "Allternit Cloud", source: "platform", kind: "cloud" },
    }
    const { rows } = buildPickerRows(
      Object.keys(meta).map((value) => ({ value, label: value })),
      { metaFor: (v) => meta[v] },
    )
    const headers = rows.filter((r) => r.kind === "header").map((r) => (r as any).title)
    expect(headers).toEqual(["Kimi · via Allternit Cloud", "Claude Code · your subscription", "DeepSeek · API key", "Local"])
  })
})
