import { afterEach, describe, expect, test } from "bun:test"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"

const memory = "Instructions from: /repo/AGENTS.md\n" + "a".repeat(4_000)
const autoMemory = "Auto-memory from: /home/me/.gizzi/MEMORY.md\n" + "b".repeat(400)
const mcpInstructions =
  "# MCP Server Instructions\n\nThe following MCP servers have provided instructions for how to use their tools and resources:\n\n## github\n" +
  "i".repeat(1_600)

function base(overrides: Partial<ContextBreakdown.BuildInput> = {}): ContextBreakdown.BuildInput {
  return {
    sessionID: "ses_1",
    messageID: "msg_1",
    providerID: "anthropic",
    modelID: "claude",
    contextWindow: 100_000,
    system: ["You are gizzi.\n" + "s".repeat(2_000) + "\n" + memory + "\n" + autoMemory + "\n" + mcpInstructions],
    instructions: ["Environment: linux", memory, autoMemory, mcpInstructions],
    tools: {
      read: { description: "r".repeat(400) },
      skill: { description: "k".repeat(800) },
      mcp__github__create_issue: { description: "g".repeat(1_200) },
      mcp__linear__list: { description: "l".repeat(40) },
      invalid: { description: "ignored" },
    },
    messages: [{ role: "user", content: "m".repeat(8_000) }],
    autoCompactBuffer: 0,
    now: 1,
    ...overrides,
  }
}

function tokens(snapshot: ContextBreakdown.Snapshot, id: ContextBreakdown.CategoryID) {
  return snapshot.categories.find((category) => category.id === id)?.tokens
}

describe("context breakdown", () => {
  afterEach(() => ContextBreakdown.clear())

  test("splits the request into categories that add up to the window", () => {
    const snapshot = ContextBreakdown.build(base({ autoCompactBuffer: 20_000 }))
    expect(snapshot.memoryFiles.map((file) => file.path)).toEqual(["/repo/AGENTS.md", "/home/me/.gizzi/MEMORY.md"])
    expect(snapshot.mcpTools.map((tool) => tool.server)).toEqual(["github", "linear"])
    expect(snapshot.systemTools.map((tool) => tool.name)).toEqual(["read"])
    expect(tokens(snapshot, "skills")).toBeGreaterThan(0)
    expect(tokens(snapshot, "memory_files")).toBe(snapshot.memoryFiles.reduce((sum, file) => sum + file.tokens, 0))
    expect(tokens(snapshot, "mcp_instructions")).toBeGreaterThan(400)
    // memory files and MCP instructions are counted once — not again inside the system prompt
    expect(tokens(snapshot, "system_prompt")).toBeLessThan(1_000)
    expect(tokens(snapshot, "autocompact_buffer")).toBe(20_000)
    const total = snapshot.categories.reduce((sum, category) => sum + category.tokens, 0)
    expect(total).toBe(100_000)
    expect(snapshot.usedTokens).toBe(total - 20_000 - tokens(snapshot, "free_space")!)
    expect(snapshot.basis).toBe("estimated")
  })

  test("orders categories like the panel and keeps every row, empty ones at 0", () => {
    const snapshot = ContextBreakdown.build(base({ tools: {}, instructions: [] }))
    expect(snapshot.categories.map((category) => category.id)).toEqual([
      "messages",
      "system_tools",
      "memory_files",
      "skills",
      "system_prompt",
      "mcp_tools",
      "mcp_instructions",
      "autocompact_buffer",
      "free_space",
      "system_tools_deferred",
      "mcp_tools_deferred",
    ])
    expect(tokens(snapshot, "mcp_tools")).toBe(0)
  })

  test("reconcile attributes the provider's extra tokens to messages", () => {
    const snapshot = ContextBreakdown.build(base())
    const reconciled = ContextBreakdown.reconcile(snapshot, {
      providerContextTokens: snapshot.usedTokens + 5_000,
      autoCompactBuffer: 13_000,
    })
    expect(reconciled.basis).toBe("provider")
    expect(reconciled.usedTokens).toBe(snapshot.usedTokens + 5_000)
    expect(tokens(reconciled, "messages")).toBe(tokens(snapshot, "messages")! + 5_000)
    expect(tokens(reconciled, "system_tools")).toBe(tokens(snapshot, "system_tools"))
    expect(tokens(reconciled, "mcp_instructions")).toBe(tokens(snapshot, "mcp_instructions"))
    expect(tokens(reconciled, "autocompact_buffer")).toBe(13_000)
    expect(reconciled.autoCompactEnabled).toBe(true)
  })

  test("reconcile never lowers messages below the local estimate", () => {
    const snapshot = ContextBreakdown.build(base())
    const reconciled = ContextBreakdown.reconcile(snapshot, { providerContextTokens: 10 })
    expect(tokens(reconciled, "messages")).toBe(tokens(snapshot, "messages"))
  })

  test("the buffer shrinks to what is left and free space never goes negative", () => {
    const snapshot = ContextBreakdown.build(base({ contextWindow: 5_000, autoCompactBuffer: 20_000 }))
    expect(tokens(snapshot, "free_space")).toBe(0)
    expect(tokens(snapshot, "autocompact_buffer")).toBe(5_000 - snapshot.usedTokens)

    const full = ContextBreakdown.build(base({ contextWindow: 1_000, autoCompactBuffer: 20_000 }))
    expect(tokens(full, "free_space")).toBe(0)
    expect(tokens(full, "autocompact_buffer")).toBe(0)
    expect(full.utilization).toBe(1)
  })

  test("fromUsage reports a provider-measured total when no request was recorded", () => {
    const snapshot = ContextBreakdown.fromUsage({
      sessionID: "ses_1",
      providerID: "openai",
      modelID: "gpt",
      contextWindow: 200_000,
      providerContextTokens: 50_000,
      autoCompactBuffer: 20_000,
    })
    expect(snapshot.usedTokens).toBe(50_000)
    expect(snapshot.utilization).toBe(0.25)
    expect(tokens(snapshot, "free_space")).toBe(130_000)
  })

  test("keeps the latest snapshot per session, bounded", () => {
    ContextBreakdown.record(ContextBreakdown.build(base({ messageID: "a" })))
    ContextBreakdown.record(ContextBreakdown.build(base({ messageID: "b" })))
    expect(ContextBreakdown.get("ses_1")?.messageID).toBe("b")
    for (let i = 0; i < 300; i++) ContextBreakdown.record(ContextBreakdown.build(base({ sessionID: `s${i}` })))
    expect(ContextBreakdown.get("ses_1")).toBeUndefined()
    expect(ContextBreakdown.get("s299")).toBeDefined()
  })

  test("lists each skill the skill tool offers", () => {
    const skill = {
      description:
        "Load a skill.\n<available_skills>\n  <skill>\n    <name>deploy</name>\n    <description>" +
        "d".repeat(400) +
        "</description>\n  </skill>\n  <skill>\n    <name>quote</name>\n    <description>q</description>\n  </skill>\n</available_skills>",
    }
    const snapshot = ContextBreakdown.build(base({ tools: { skill } }))
    expect(snapshot.skills?.map((item) => item.name)).toEqual(["deploy", "quote"])
    expect(snapshot.source).toBe("gizzi")
  })

  test("reads Claude Code's own /context report, deferred tools apart", () => {
    const markdown = [
      "## Context Usage",
      "",
      "**Model:** claude-haiku-5-5  ",
      "**Tokens:** 34k / 1m (3%)",
      "",
      "### Estimated usage by category",
      "",
      "| Category | Tokens | Percentage |",
      "|----------|--------|------------|",
      "| System prompt | 3.3k | 0.3% |",
      "| System tools | 611 | 0.1% |",
      "| MCP tools | 671 | 0.1% |",
      "| MCP server instructions | 717 | 0.1% |",
      "| MCP tools (deferred) | 1.4k | 0.1% |",
      "| System tools (deferred) | 14.1k | 1.4% |",
      "| Memory files | 5.8k | 0.6% |",
      "| Skills | 10k | 1.0% |",
      "| Messages | 13k | 1.3% |",
      "| Free space | 933k | 93.3% |",
      "| Autocompact buffer | 33k | 3.3% |",
      "",
      "### MCP Tools",
      "",
      "| Tool | Server | Tokens |",
      "|------|--------|--------|",
      "| mcp__docs__read | docs | 324 |",
      "",
      "### Memory Files",
      "",
      "| Type | Path | Tokens |",
      "|------|------|--------|",
      "| User | /Users/me/.claude/CLAUDE.md | 5.8k |",
      "",
      "### Skills",
      "",
      "| Skill | Source | Tokens |",
      "|-------|--------|--------|",
      "| deploy | User | ~140 |",
    ].join("\n")
    const snapshot = ContextBreakdown.fromClaudeContext({ markdown, sessionID: "ses_1", providerID: "claude-cli", modelID: "claude-haiku-5-5" })!
    expect(snapshot.source).toBe("brain")
    expect(snapshot.contextWindow).toBe(1_000_000)
    expect(tokens(snapshot, "messages")).toBe(13_000)
    expect(tokens(snapshot, "autocompact_buffer")).toBe(33_000)
    expect(tokens(snapshot, "system_tools_deferred")).toBe(14_100)
    // deferred tools are listed but not in the window
    expect(snapshot.usedTokens).toBe(3_300 + 611 + 671 + 717 + 5_800 + 10_000 + 13_000)
    expect(snapshot.mcpTools).toEqual([{ name: "mcp__docs__read", server: "docs", tokens: 324 }])
    expect(snapshot.memoryFiles).toEqual([{ path: "/Users/me/.claude/CLAUDE.md", tokens: 5_800 }])
    expect(snapshot.skills).toEqual([{ name: "deploy", tokens: 140 }])
    // a later context total from the same brain moves only Messages
    const next = ContextBreakdown.reconcile(snapshot, { providerContextTokens: snapshot.usedTokens + 5_000 })
    expect(tokens(next, "messages")).toBe(18_000)
    expect(tokens(next, "mcp_tools_deferred")).toBe(1_400)
  })
})

