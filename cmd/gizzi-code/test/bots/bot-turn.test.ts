import { afterEach, describe, expect, test } from "bun:test"
import path from "node:path"
import * as BotTurn from "@/runtime/bots/bot-turn"
import { composeTurnSystem, headerParts } from "@/runtime/session/system-header"
import { SystemPrompt } from "@/runtime/session/system"
import { cliAdapterArgv } from "@/runtime/drivers/local-cli-driver"

const PERSONA = "# Bot identity\n\nYou are A://, Eoj's main bot."
const claudeModel = { api: { id: "claude-sonnet-4-5" }, providerID: "anthropic" } as any

async function turnPrompt(opts: { bot?: BotTurn.BotTurnInfo; userSystem?: string }) {
  const loaded: string[] = []
  const system = await composeTurnSystem({
    userSystem: opts.userSystem,
    bot: opts.bot,
    environment: () => ["<env>Working directory: /w</env>"],
    instructions: () => {
      loaded.push("instructions")
      return ["Instructions from: /Users/x/.claude/CLAUDE.md\n- my name is Eoj"]
    },
    workspace: () => {
      loaded.push("workspace")
      return ["# Identity\nfrom ~/.gizzi"]
    },
    mcpInstructions: () => {
      loaded.push("mcpInstructions")
      return ["# MCP Server Instructions\n\n## github\nUse search_code first."]
    },
    scratchpad: "scratchpad note",
  })
  const header = headerParts({
    bot: opts.bot,
    provider: () => SystemPrompt.provider(claudeModel),
    botGuidance: () => SystemPrompt.bot(),
    system,
  }).join("\n")
  return { header, loaded }
}

const bot: BotTurn.BotTurnInfo = { id: "agent-1", name: "A://" }

afterEach(() => BotTurn.reset())

describe("bot turn system prompt", () => {
  test("starts with the bot persona and has no Allternit Shell identity", async () => {
    const { header, loaded } = await turnPrompt({ bot, userSystem: `+${PERSONA}` })
    expect(header.startsWith(PERSONA)).toBe(true)
    expect(header).not.toContain("Allternit Shell")
    expect(header).not.toContain("allternit-shell")
    expect(header).toContain("# Operating guidance")
    // no personal instruction files or ~/.gizzi identity are even read
    expect(loaded).toEqual([])
    expect(header).not.toContain(".claude/CLAUDE.md")
    expect(header).not.toContain("from ~/.gizzi")
  })

  test("the bot header never carries a coding-agent identity, in any mode", () => {
    for (const mode of [undefined, "plan", "build"] as const) {
      const text = SystemPrompt.bot(mode).join("\n")
      expect(text).not.toMatch(/Allternit Shell|allternit-shell|coding agent/i)
    }
  })

  test("non-bot sessions are unchanged: provider header first, instructions loaded, + text last", async () => {
    const { header, loaded } = await turnPrompt({ userSystem: "+standing instructions" })
    expect(header.startsWith("You are Allternit Shell (allternit-shell), an AI coding agent.")).toBe(true)
    expect(loaded).toEqual(["instructions", "workspace", "mcpInstructions"])
    expect(header).toContain("my name is Eoj")
    expect(header.trimEnd().endsWith("standing instructions")).toBe(true)
    expect(header).not.toContain("# Operating guidance")
  })

  test("a full (non +) system override still replaces everything for both", async () => {
    const plain = await turnPrompt({ userSystem: "ONLY THIS" })
    expect(plain.header.endsWith("ONLY THIS")).toBe(true)
    const botOverride = await turnPrompt({ bot, userSystem: "ONLY THIS" })
    expect(botOverride.header.startsWith("ONLY THIS")).toBe(true)
    expect(botOverride.header).not.toContain("Allternit Shell")
  })
})

describe("bot marker resolution", () => {
  test("newest user message marker wins and sticks for the session", () => {
    const msgs = [
      { info: { role: "user", bot } },
      { info: { role: "assistant" } },
      { info: { role: "user" } }, // synthetic follow-up without a marker
    ]
    expect(BotTurn.resolve("ses_1", msgs)?.id).toBe("agent-1")
    expect(BotTurn.resolve("ses_1", [{ info: { role: "user" } }])?.id).toBe("agent-1")
    expect(BotTurn.resolve("ses_2", [{ info: { role: "user" } }])).toBeUndefined()
  })
})

describe("bot skills", () => {
  const home = "/Users/x"
  test("personal and project skills are hidden; builtin and app-managed stay", () => {
    expect(BotTurn.skillVisibleToBot({ location: path.join(home, ".claude/skills/weekly-review/SKILL.md"), source: "user" }, home)).toBe(false)
    expect(BotTurn.skillVisibleToBot({ location: path.join(home, ".agents/skills/x/SKILL.md"), source: "user" }, home)).toBe(false)
    expect(BotTurn.skillVisibleToBot({ location: "/repo/.claude/skills/x/SKILL.md", source: "project" }, home)).toBe(false)
    expect(BotTurn.skillVisibleToBot({ location: "builtin://pdf", builtin: true }, home)).toBe(true)
    expect(BotTurn.skillVisibleToBot({ location: path.join(home, ".allternit/skills/x/SKILL.md"), source: "user" }, home)).toBe(true)
  })
})

describe("claude-cli bot spawn args", () => {
  const mcp = { name: "allternit", url: "http://127.0.0.1:1/mcp", headers: {} }
  const argv = (ctx: Record<string, unknown>) =>
    cliAdapterArgv("claude-cli", ["claude"], "hi", { taskId: "t1", systemPrompt: `${PERSONA}\n\nmore`, mcp, ...ctx })
  const after = (args: string[], flag: string) => args[args.indexOf(flag) + 1]

  test("normal sessions keep today's flags", () => {
    const args = argv({})
    expect(after(args, "--permission-mode")).toBe("bypassPermissions")
    expect(args).toContain("--append-system-prompt")
    expect(args).not.toContain("--setting-sources")
    expect(args).not.toContain("--system-prompt")
  })

  test("a gated bot is isolated, gets its allowlist and no bypassPermissions", () => {
    const gatedBot: BotTurn.BotTurnInfo = {
      id: "agent-1",
      allowedTools: ["web_search", "file_read", "code_execution"],
      askTools: ["code_execution"],
      gated: true,
    }
    const args = argv({ bot: gatedBot })
    expect(args).not.toContain("bypassPermissions")
    expect(after(args, "--setting-sources")).toBe("")
    expect(args).toContain("--disable-slash-commands")
    expect(args).toContain("--strict-mcp-config")
    expect(after(args, "--permission-mode")).toBe("default")
    expect(after(args, "--permission-prompt-tool")).toBe("stdio")
    const allowed = after(args, "--allowedTools").split(",")
    expect(allowed).toEqual(expect.arrayContaining(["WebSearch", "Read", "Glob", "Grep", "mcp__allternit"]))
    // ask-each-time tools are not pre-approved
    expect(allowed).not.toContain("Bash")
    // persona replaces Claude Code's own prompt instead of being appended
    expect(after(args, "--system-prompt").startsWith(PERSONA)).toBe(true)
    expect(args).not.toContain("--append-system-prompt")
  })

  test("a bot with no allowlist or gates keeps running its tools, still isolated", () => {
    const args = argv({ bot: { id: "agent-2" } })
    expect(after(args, "--permission-mode")).toBe("bypassPermissions")
    expect(after(args, "--setting-sources")).toBe("")
    expect(args).toContain("--disable-slash-commands")
    expect(args).not.toContain("--allowedTools")
  })
})

describe("claude-cli bot tool decisions", () => {
  test("allowlisted → allow, ask-each-time → ask, unlisted → deny (or ask when gated)", () => {
    const b: BotTurn.BotTurnInfo = { id: "a", allowedTools: ["web_search"], askTools: ["code_execution"] }
    expect(BotTurn.claudeToolDecision(b, "WebSearch", "allternit")).toBe("allow")
    expect(BotTurn.claudeToolDecision(b, "mcp__allternit__pane_doc", "allternit")).toBe("allow")
    expect(BotTurn.claudeToolDecision(b, "Bash", "allternit")).toBe("ask")
    expect(BotTurn.claudeToolDecision(b, "Write", "allternit")).toBe("deny")
    expect(BotTurn.claudeToolDecision({ ...b, gated: true }, "Write", "allternit")).toBe("ask")
    expect(BotTurn.deniedMessage({ id: "a", name: "A://" }, "Write")).toContain("A://'s allowed tools")
  })
})

describe("MCP server instructions in the turn system", () => {
  test("a normal turn carries them after the workspace identity, before the scratchpad", async () => {
    const { header, loaded } = await turnPrompt({})
    expect(loaded).toContain("mcpInstructions")
    const at = header.indexOf("# MCP Server Instructions")
    expect(at).toBeGreaterThan(header.indexOf("# Identity"))
    expect(at).toBeLessThan(header.indexOf("scratchpad note"))
  })

  test("a bot turn does not load them", async () => {
    const { header, loaded } = await turnPrompt({ bot, userSystem: `+${PERSONA}` })
    expect(loaded).not.toContain("mcpInstructions")
    expect(header).not.toContain("# MCP Server Instructions")
  })
})
