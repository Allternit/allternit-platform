import { describe, expect, test } from "bun:test"
import { ProviderTransform } from "@/runtime/providers/adapters/transform"

const base = {
  providerID: "fixture",
  name: "Fixture",
  status: "active",
  headers: {},
  options: {},
  release_date: "2026-01-01",
  capabilities: {
    temperature: true,
    reasoning: true,
    attachment: true,
    toolcall: true,
    input: { text: true, audio: false, image: true, video: false, pdf: true },
    output: { text: true, audio: false, image: false, video: false, pdf: false },
    interleaved: false,
  },
  cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
  limit: { context: 200_000, output: 64_000 },
  variants: {},
} as any

describe("cross-provider protocol goldens", () => {
  test("Kimi K2.5 keeps its audited sampling and Anthropic thinking contract", () => {
    const model = {
      ...base,
      id: "kimi-k2.5",
      providerID: "kimi",
      api: { id: "kimi-k2.5", npm: "@ai-sdk/anthropic", url: "https://api.moonshot.ai/anthropic" },
    }
    expect({
      temperature: ProviderTransform.temperature(model),
      topP: ProviderTransform.topP(model),
      options: ProviderTransform.options({ model, sessionID: "fixture", providerOptions: {} }),
    }).toEqual({
      temperature: 1,
      topP: 0.95,
      // O8: the session prompt-cache key is on by default for every provider.
      options: { promptCacheKey: "fixture", thinking: { type: "enabled", budgetTokens: 16_000 } },
    })
  })

  test("OpenAI Responses exposes stable reasoning variants and privacy defaults", () => {
    const model = {
      ...base,
      id: "gpt-5.2-codex",
      providerID: "openai",
      api: { id: "gpt-5.2-codex", npm: "@ai-sdk/openai", url: "https://api.openai.com/v1" },
      release_date: "2026-01-01",
    }
    expect(Object.keys(ProviderTransform.variants(model))).toEqual(["low", "medium", "high", "xhigh"])
    expect(ProviderTransform.options({ model, sessionID: "fixture", providerOptions: {} })).toEqual({
      store: false,
      promptCacheKey: "fixture",
      reasoningEffort: "medium",
      reasoningSummary: "auto",
    })
  })

  test("Anthropic removes blank messages and normalizes tool-call identifiers", () => {
    const model = {
      ...base,
      id: "claude-sonnet-4-6",
      providerID: "anthropic",
      api: { id: "claude-sonnet-4-6", npm: "@ai-sdk/anthropic", url: "https://api.allternit.com" },
    }
    const result = ProviderTransform.message([
      { role: "assistant", content: "" },
      { role: "assistant", content: [{ type: "tool-call", toolCallId: "bad:id", toolName: "read", input: {} }] },
    ] as any, model, {}) as any[]
    expect(result).toHaveLength(1)
    expect(result[0].content[0].toolCallId).toBe("bad_id")
  })

  test("Google schema normalization supplies array item schemas", () => {
    const model = {
      ...base,
      id: "gemini-3.1-pro",
      providerID: "google",
      api: { id: "gemini-3.1-pro", npm: "@ai-sdk/google", url: "https://generativelanguage.googleapis.com" },
    }
    expect(ProviderTransform.schema(model, {
      type: "object",
      properties: { values: { type: "array" } },
    } as any)).toMatchObject({
      properties: { values: { type: "array", items: {} } },
    })
  })
})
