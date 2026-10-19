import { describe, expect, test } from "bun:test"
import { CodexContext } from "@/runtime/session/codex-context"
import { ContextBreakdown } from "@/runtime/session/context-breakdown"

const est = ContextBreakdown.estimate
const base = "You are Codex." + " x".repeat(2_000)
const skills = "<skills_instructions>\n## Skills\n### Available skills\n- deploy: ship it (file: a/SKILL.md)\n- quote: price it (file: b/SKILL.md)\n</skills_instructions>"
const agents = "# AGENTS.md instructions\n\n<INSTRUCTIONS>\nBe brief.\n</INSTRUCTIONS>"
const perms = "<permissions instructions>sandbox</permissions instructions>"
const user = "hello there"

function rollout(firstInput: number) {
  const msg = (role: string, text: string) => ({ type: "response_item", payload: { type: "message", role, content: [{ type: "input_text", text }] } })
  return [
    { type: "session_meta", payload: { base_instructions: { text: base } } },
    msg("developer", skills),
    msg("developer", perms),
    msg("user", agents),
    msg("user", user),
    { type: "event_msg", payload: { type: "token_count", info: { last_token_usage: { input_tokens: firstInput } } } },
    msg("assistant", "hi"),
    msg("developer", perms),
  ]
    .map((line) => JSON.stringify(line))
    .join("\n")
}

const prompt = est(base) + est(skills) + est(perms) + est(agents)

describe("CodexContext", () => {
  test("fills every row from the rollout, MCP status and the latest total", () => {
    const mcpStatus = { data: [{ name: "ops", tools: { brain_read: { name: "brain_read", description: "read", inputSchema: { type: "object" } } } }] }
    const mcp = CodexContext.mcpTools(mcpStatus)[0].tokens
    const firstInput = prompt + est(user) + 3_000
    const snapshot = CodexContext.build({
      rollout: rollout(firstInput),
      mcpStatus,
      agentFiles: { "/home/me/.codex/AGENTS.md": "Be brief." },
      used: 50_000,
      size: 400_000,
      sessionID: "ses_1",
      providerID: "codex-cli",
      modelID: "gpt",
    })!
    const tokens = (id: string) => snapshot.categories.find((c) => c.id === id)?.tokens
    expect(snapshot.source).toBe("brain")
    expect(tokens("system_prompt")).toBe(est(base) + est(perms))
    expect(tokens("skills")).toBe(est(skills))
    expect(snapshot.skills?.map((s) => s.name).sort()).toEqual(["deploy", "quote"])
    expect(tokens("memory_files")).toBe(est(agents))
    expect(snapshot.memoryFiles[0].path).toBe("/home/me/.codex/AGENTS.md")
    expect(tokens("mcp_tools")).toBe(mcp)
    expect(tokens("system_tools")).toBe(3_000 - mcp)
    expect(snapshot.usedTokens).toBe(50_000)
    expect(tokens("messages")).toBe(50_000 - prompt - 3_000)
  })

  test("more MCP schema than the request held → deferred", () => {
    const big = { data: [{ name: "apps", tools: { a: { name: "a", description: "d".repeat(40_000) } } }] }
    const snapshot = CodexContext.build({ rollout: rollout(prompt + est(user) + 500), mcpStatus: big, used: 30_000, size: 400_000, sessionID: "s", providerID: "codex-cli", modelID: "gpt" })!
    expect(snapshot.categories.find((c) => c.id === "mcp_tools")?.tokens).toBe(0)
    expect(snapshot.categories.find((c) => c.id === "mcp_tools_deferred")?.tokens).toBeGreaterThan(9_000)
    expect(snapshot.categories.find((c) => c.id === "system_tools")?.tokens).toBe(500)
  })
})

describe("CodexContext blocks", () => {
  test("splits a developer message's blocks, gizzi's memory files by path", () => {
    const gizzi = "You are allternit-shell." + " y".repeat(500) + "\nInstructions from: /repo/AGENTS.md\n" + "m".repeat(800)
    const line = (payload: unknown) => JSON.stringify({ type: "response_item", payload })
    const rollout = [
      line({ type: "message", role: "developer", content: [{ text: gizzi }, { text: skills }, { text: perms }] }),
      line({ type: "message", role: "user", content: [{ text: agents }, { text: "<environment_context>cwd</environment_context>" }] }),
    ].join("\n")
    const snapshot = CodexContext.build({ rollout, used: 20_000, size: 200_000, sessionID: "s", providerID: "codex-cli", modelID: "gpt" })!
    const tokens = (id: string) => snapshot.categories.find((c) => c.id === id)?.tokens
    expect(tokens("skills")).toBe(est(skills))
    expect(snapshot.memoryFiles.map((f) => f.path).sort()).toEqual(["/repo/AGENTS.md", "AGENTS.md"])
    expect(tokens("memory_files")).toBeGreaterThan(est(agents))
  })
})
