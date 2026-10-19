import { describe, expect, test } from "bun:test"
import { BrainRelay } from "@/runtime/session/brain-relay"

describe("BrainRelay.toBuild", () => {
  test("OpenAI chat: system, tools (server_ tools as MCP), messages", () => {
    const r = BrainRelay.toBuild(
      {
        model: "m",
        messages: [
          { role: "system", content: "You are opencode.\nInstructions from: /repo/AGENTS.md\nrules" },
          { role: "user", content: "hi" },
        ],
        tools: [
          { type: "function", function: { name: "bash", description: "run" } },
          { type: "function", function: { name: "ops_brain_read", description: "read" } },
        ],
      },
      ["ops"],
    )
    expect(r.model).toBe("m")
    expect(Object.keys(r.tools)).toEqual(["bash", "mcp__ops__brain_read"])
    expect(r.instructions[1].startsWith("Instructions from: /repo/AGENTS.md")).toBe(true)
    expect(r.messages).toHaveLength(1)
  })

  test("Gemini (Code Assist wrapper): systemInstruction, functionDeclarations, contents, Context from markers", () => {
    const r = BrainRelay.toBuild({
      model: "gemini",
      request: {
        systemInstruction: { parts: [{ text: "Base.\n--- Context from: /h/GEMINI.md ---\nmine" }] },
        tools: [{ functionDeclarations: [{ name: "read_file" }] }],
        contents: [{ role: "user", parts: [{ text: "hi" }] }],
      },
    })
    expect(Object.keys(r.tools)).toEqual(["read_file"])
    expect(r.instructions.some((b) => b.startsWith("Instructions from: /h/GEMINI.md"))).toBe(true)
    expect(r.messages).toHaveLength(1)
  })

  test("Anthropic: system blocks and input_schema tools", () => {
    const r = BrainRelay.toBuild({ system: [{ type: "text", text: "S" }], tools: [{ name: "mcp__x__y", input_schema: {} }], messages: [{ role: "user", content: "q" }] })
    expect(r.system).toEqual(["S"])
    expect(Object.keys(r.tools)).toEqual(["mcp__x__y"])
  })
})
