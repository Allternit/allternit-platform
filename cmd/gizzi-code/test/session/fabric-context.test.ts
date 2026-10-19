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
    // system prompt + tools estimated from Anthropic's published prompt; connector schemas stay hidden
    expect(s.estimatedRows).toEqual(["system_prompt", "system_tools"])
    expect(s.hidden).toEqual(["mcp_tools"])
    expect(s.categories.find((c) => c.id === "system_prompt")?.tokens).toBe(6_780)
    expect(s.usedTokens).toBe(10_500 + 6_780 + 10_586)
    expect(s.mcpTools[0].server).toBe("drive")
  })

  test("ChatGPT: thinking vs instant tables, connectors as MCP tools", () => {
    const base = { rows: { messages: 100, system_tools: 0, memory_files: 0, skills: 0, system_prompt: 0, mcp_tools: 0, mcp_instructions: 0 }, memory_files: [], mcp_tools: [], system_tools: [], skills: [], hidden: ["system_prompt", "system_tools", "mcp_tools"] }
    const thinking = FabricContext.estimateHidden({ ...base, vendor: "chatgpt", model: "gpt-6-thinking" })
    expect(thinking.report.rows.system_prompt).toBe(2_570)
    expect(thinking.report.system_tools.find((t) => t.name === "web")?.tokens).toBe(9_785)
    expect(thinking.report.hidden).toEqual(["mcp_tools"])
    const instant = FabricContext.estimateHidden({ ...base, vendor: "chatgpt", model: "gpt-6-instant" })
    expect(instant.report.rows.system_prompt).toBe(6_030)
    expect(instant.report.mcp_tools.map((t) => t.name)).toEqual(["gmail", "gcal", "gcontacts"])
    expect(instant.estimated).toEqual(["system_prompt", "system_tools", "mcp_tools"])
  })
})
