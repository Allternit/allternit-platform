import { describe, expect, test } from "bun:test"
import { KimiContext } from "@/runtime/session/kimi-context"

const agents = "# Global\nBe brief and kind."
const prompt = `You are Kimi Code CLI.\n# Project information\n${agents}\n# Skills\n## Available skills\n- deploy: ship it\n- quote: price it\n# Environment\ncwd`
const wire = [
  { type: "profile.bind", agentId: "main", modelAlias: "kimi-code/kimi-for-coding", systemPrompt: prompt, agentsMdPaths: ["/h/.kimi-code/AGENTS.md"] },
  { type: "llm.tools_snapshot", agentId: "main", tools: [{ name: "Bash", description: "run" }, { name: "mcp__ops__brain_read", description: "read" }] },
  { type: "context.append_message", agentId: "main", message: { role: "user", content: [{ type: "text", text: "<session_instructions>\nYou are gizzi.\nInstructions from: /repo/AGENTS.md\nrepo rules\n</session_instructions>\n\nhello" }] } },
  { type: "context.append_message", agentId: "explore", message: { role: "user", content: "subagent" } },
].map((line) => JSON.stringify(line)).join("\n")

describe("KimiContext", () => {
  test("splits Kimi's own record into every row", () => {
    expect(KimiContext.agentPaths(wire)).toEqual(["/h/.kimi-code/AGENTS.md"])
    const s = KimiContext.build({ wire, agentFiles: { "/h/.kimi-code/AGENTS.md": agents }, size: 200_000, sessionID: "s", providerID: "kimi-cli", modelID: "k" })!
    const tokens = (id: string) => s.categories.find((c) => c.id === id)?.tokens ?? 0
    expect(s.source).toBe("brain")
    expect(s.modelID).toBe("kimi-code/kimi-for-coding")
    expect(s.memoryFiles.map((f) => f.path).sort()).toEqual(["/h/.kimi-code/AGENTS.md", "/repo/AGENTS.md"])
    expect(s.skills?.map((x) => x.name)).toEqual(["deploy", "quote"])
    expect(s.systemTools.map((t) => t.name)).toEqual(["Bash"])
    expect(s.mcpTools.map((t) => t.server)).toEqual(["ops"])
    for (const id of ["system_prompt", "memory_files", "skills", "system_tools", "mcp_tools", "messages"]) expect(tokens(id)).toBeGreaterThan(0)
  })
})
