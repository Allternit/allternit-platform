import { describe, expect, test } from "bun:test"
import { QwenContext } from "@/runtime/session/qwen-context"

const text = `## Context Usage

**Estimated pre-conversation overhead**
Model: qwen3.7-max  Context window: 1000.0k tokens

  System prompt                       5.7k tokens (0.6%)
  Built-in tools                     22.0k tokens (2.2%)
  MCP tools                           1.2k tokens (0.1%)
  Memory files                        3.9k tokens (0.4%)
  Skills                               472 tokens (0.0%)
  Messages                            8.1k tokens (0.8%)
`

describe("QwenContext", () => {
  test("reads Qwen Code's own /context rows", () => {
    const s = QwenContext.parse({ text, sessionID: "s", providerID: "qwen-cli", modelID: "q" })!
    const tokens = (id: string) => s.categories.find((c) => c.id === id)?.tokens
    expect(s.contextWindow).toBe(1_000_000)
    expect(s.modelID).toBe("qwen3.7-max")
    expect(tokens("system_tools")).toBe(22_000)
    expect(tokens("skills")).toBe(472)
    expect(tokens("messages")).toBe(8_100)
    expect(s.usedTokens).toBe(5_700 + 22_000 + 1_200 + 3_900 + 472 + 8_100)
    expect(s.source).toBe("brain")
  })
})
