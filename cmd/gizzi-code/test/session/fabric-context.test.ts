import { describe, expect, test } from "bun:test"
import { FabricContext } from "@/runtime/session/fabric-context"

describe("FabricContext.snapshot", () => {
  test("a signed-in app's report becomes every row, hidden rows named", () => {
    const s = FabricContext.snapshot({
      sessionID: "s",
      providerID: "subs-claude",
      modelID: "standard",
      contextWindow: 200_000,
      report: {
        vendor: "claude",
        model: "claude-opus-5-5",
        rows: { messages: 9_000, system_tools: 0, memory_files: 1_200, skills: 300, system_prompt: 0, mcp_tools: 0, mcp_instructions: 0 },
        memory_files: [{ path: "Claude memory", tokens: 1_200 }],
        mcp_tools: [{ name: "mcp__drive__search", server: "drive", tokens: 0 }],
        system_tools: [{ name: "web search", tokens: 0 }],
        skills: [{ name: "pdf", tokens: 300 }],
        hidden: ["system_prompt", "system_tools", "mcp_tools", "bogus"],
      },
    })!
    expect(s.source).toBe("brain")
    expect(s.modelID).toBe("claude-opus-5-5")
    expect(s.usedTokens).toBe(10_500)
    expect(s.hidden).toEqual(["system_prompt", "system_tools", "mcp_tools"])
    expect(s.mcpTools[0].server).toBe("drive")
  })
})
