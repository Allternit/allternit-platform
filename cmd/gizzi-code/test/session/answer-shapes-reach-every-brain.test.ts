import { describe, expect, test } from "bun:test"
import { composeTurnSystem } from "@/runtime/session/system-header"
import { extractSystemText } from "@/runtime/providers/adapters/loaders/system-text"
import { claudeSessionFlags, cliTurnMessage, codexDeveloperInstructions, withInstructions } from "@/runtime/drivers/cli-session-flags"

/**
 * allternit-api (answer_shapes.rs) and allternit-ai (answer-shapes.ts) send the
 * answer-shape block as the turn's "+" system text. gizzi adds nothing of its
 * own; these pin that the block reaches every brain unchanged: the in-process
 * models (Allternit Cloud, API keys) through the composed system list, and the
 * CLI brains through the system text the subprocess loader hands their driver.
 */
const SHAPES = "## Answer shape\nEvery reply follows one shape…\n- Open with the bottom line: one sentence that answers the question."
const lazy = () => [] as string[]

describe("answer shapes reach every brain", () => {
  test("the composed system keeps the appended block, normal and bot turns", async () => {
    const normal = await composeTurnSystem({ userSystem: `+${SHAPES}`, environment: lazy, instructions: lazy, workspace: lazy })
    expect(normal).toContain(SHAPES)
    const bot = await composeTurnSystem({
      userSystem: `+${SHAPES}`,
      bot: { agentId: "a1" } as any,
      environment: lazy,
      instructions: lazy,
      workspace: lazy,
    })
    expect(bot[0]).toBe(SHAPES)
  })

  test("CLI brains get it as their system prompt", () => {
    const system = extractSystemText([
      { role: "system", content: "Provider header" },
      { role: "system", content: SHAPES },
      { role: "user", content: "compare A and B" },
    ])
    expect(system).toContain(SHAPES)
    // Claude Code: appended (or, on a bot turn, the whole) system prompt.
    const claude = claudeSessionFlags({ systemPrompt: system })
    expect(claude[claude.indexOf("--append-system-prompt") + 1]).toContain(SHAPES)
    const claudeBot = claudeSessionFlags({ systemPrompt: system, bot: {} as any })
    expect(claudeBot[claudeBot.indexOf("--system-prompt") + 1]).toContain(SHAPES)
    // Codex: developer instructions, with or without gizzi's session tools.
    expect(codexDeveloperInstructions(system, undefined)).toContain(SHAPES)
    expect(codexDeveloperInstructions(system, { name: "gizzi", url: "http://x", headers: {} } as any)).toContain(SHAPES)
    // ACP brains (Kimi, Grok, Hermes, Kiro…): it leads every prompt.
    expect(withInstructions("compare A and B", system)).toContain(SHAPES)
  })

  test("CLIs without a system channel get it leading the message; the others don't get it twice", () => {
    // Gemini CLI and the other text CLIs, Antigravity, Qwen, Cursor, OpenCode, Pi…
    for (const [cli, mode] of [
      ["gemini-cli", "one-shot-text"],
      ["antigravity", "one-shot-text"],
      ["qwen-cli", "stream-json"],
      ["cursor-agent", "stream-json"],
      ["opencode", "stream-json"],
      ["pi", "one-shot-json"],
      ["openclaw", "openclaw-json"],
    ]) {
      const message = cliTurnMessage("compare A and B", SHAPES, cli, mode)
      expect(message.startsWith("<session_instructions>\n## Answer shape")).toBe(true)
      expect(message.endsWith("compare A and B")).toBe(true)
    }
    // Claude Code (flags), Codex (developer instructions), ACP (runAcp prefixes it).
    expect(cliTurnMessage("hi", SHAPES, "claude-cli", "stream-json")).toBe("hi")
    expect(cliTurnMessage("hi", SHAPES, "codex-cli", "codex-app-server")).toBe("hi")
    expect(cliTurnMessage("hi", SHAPES, "kimi-cli", "acp")).toBe("hi")
    expect(cliTurnMessage("hi", undefined, "gemini-cli", "one-shot-text")).toBe("hi")
  })
})
