import { describe, expect, test } from "bun:test"
import { usesFirstPartyAuth } from "../../src/cli/ui/ink-app/components/PromptInput/Notifications"

describe("usesFirstPartyAuth", () => {
  test("bare and allternit-prefixed models need the first-party login", () => {
    expect(usesFirstPartyAuth("opus")).toBe(true)
    expect(usesFirstPartyAuth("allternit/some-model")).toBe(true)
    expect(usesFirstPartyAuth(undefined)).toBe(true)
  })

  test("other providers authenticate on their own", () => {
    expect(usesFirstPartyAuth("openrouter/~deepseek/deepseek-flash-latest")).toBe(false)
    expect(usesFirstPartyAuth("kimi-cli/kimi-k2")).toBe(false)
    expect(usesFirstPartyAuth("local-mlx/qwen3.6-35b-a3b-4bit")).toBe(false)
  })
})
