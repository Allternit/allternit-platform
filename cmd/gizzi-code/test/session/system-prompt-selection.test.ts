import { describe, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"
import { SystemPrompt } from "@/runtime/session/system"
import { composeTurnSystem, headerParts } from "@/runtime/session/system-header"
import BASE_CHAT from "@/runtime/session/prompt/base-chat.txt"
import BASE_CODE from "@/runtime/session/prompt/base-code.txt"
import CAPABILITIES from "@/runtime/session/prompt/capabilities.txt"
import SAFETY from "@/runtime/session/prompt/safety.txt"
import BOT from "@/runtime/session/prompt/bot.txt"
import VOICE from "@/runtime/session/prompt/voice.txt"
import PHONE from "@/runtime/session/prompt/phone.txt"
import PLAN_MODE from "@/runtime/session/prompt/plan-mode.txt"
import ADDENDUM_GPT from "@/runtime/session/prompt/addendum-gpt.txt"
import ADDENDUM_GEMINI from "@/runtime/session/prompt/addendum-gemini.txt"
import ADDENDUM_OPEN from "@/runtime/session/prompt/addendum-open.txt"

const model = (id: string) => ({ api: { id } })
const claude = model("claude-sonnet-4-5")
const ADDENDA = [ADDENDUM_GPT, ADDENDUM_GEMINI, ADDENDUM_OPEN]
const count = (haystack: string, needle: string) => haystack.split(needle).length - 1
const CHAT_SURFACES = ["chat", "cowork", "browser", "design"] as const
const firstLine = (text: string) => text.split("\n")[0]!

describe("prompt stack by session surface", () => {
  test("chat, cowork, browser and design get the chat stack", () => {
    for (const surface of CHAT_SURFACES) {
      const text = SystemPrompt.provider(claude, undefined, { surface }).join("\n")
      expect(text).toContain(BASE_CHAT)
      expect(text).toContain(CAPABILITIES)
      expect(text).toContain(SAFETY)
      expect(text).not.toContain(BASE_CODE)
    }
  })

  test("code and an unset surface get the Gizzi Code stack", () => {
    for (const ctx of [{ surface: "code" as const }, {}, undefined]) {
      const text = SystemPrompt.provider(claude, undefined, ctx).join("\n")
      expect(text).toContain(BASE_CODE)
      expect(text).toContain(SAFETY)
      expect(text).not.toContain(BASE_CHAT)
      expect(text).not.toContain(CAPABILITIES)
    }
  })

  test("order: base, capabilities, safety, addendum, mode, voice, phone", () => {
    const parts = SystemPrompt.provider(model("gpt-5.1"), "plan", { surface: "chat", replyMode: "voice", formFactor: "phone" })
    expect(parts).toEqual([BASE_CHAT, CAPABILITIES, SAFETY, ADDENDUM_GPT, PLAN_MODE, VOICE, PHONE])
  })
})

describe("2026-11-16 audit: no vendor names or Allternit Shell in prompts", () => {
  const VENDOR = /claude|anthropic|chatgpt|openai|gemini|grok|codex|opencode|allternit shell/i
  const dirs = [
    path.join(import.meta.dir, "../../src/runtime/session/prompt"),
    path.join(import.meta.dir, "../../src/runtime/loop/prompts"),
  ]
  const files = dirs.flatMap((dir) =>
    fs.readdirSync(dir).filter((name) => name.endsWith(".txt")).map((name) => path.join(dir, name)),
  )

  test("finds the prompt files", () => {
    expect(files.length).toBeGreaterThan(10)
  })

  for (const file of files) {
    test(path.relative(path.join(import.meta.dir, "../../src/runtime"), file), () => {
      const hits = fs
        .readFileSync(file, "utf8")
        .split("\n")
        .map((line, i) => ({ line: i + 1, text: line }))
        .filter(({ text }) => VENDOR.test(text))
      expect(hits).toEqual([])
    })
  }
})

describe("one addendum per model family", () => {
  test("family()", () => {
    expect(SystemPrompt.family(model("gpt-5.1"))).toBe("gpt")
    expect(SystemPrompt.family(model("o3-mini"))).toBe("gpt")
    expect(SystemPrompt.family(model("o4-mini"))).toBe("gpt")
    expect(SystemPrompt.family(model("openai/gpt-5"))).toBe("gpt")
    expect(SystemPrompt.family(model("claude-opus-4-1"))).toBeUndefined()
    expect(SystemPrompt.family(model("anthropic/claude-sonnet-4.5"))).toBeUndefined()
    expect(SystemPrompt.family(model("gemini-2.5-pro"))).toBe("gemini")
    for (const id of ["qwen3-coder", "kimi-k2", "glm-4.6", "deepseek-v3", "trinity-mini", "llama-3.3-70b", "mistral-large", "local-model", "something-new"]) {
      expect(SystemPrompt.family(model(id))).toBe("open")
    }
  })

  test("gpt-5.1 gets exactly the gpt addendum; claude gets none", () => {
    const gpt = SystemPrompt.provider(model("gpt-5.1")).join("\n")
    expect(ADDENDA.filter((a) => gpt.includes(a))).toEqual([ADDENDUM_GPT])
    expect(count(gpt, ADDENDUM_GPT)).toBe(1)
    const none = SystemPrompt.provider(claude, "build", { surface: "chat" }).join("\n")
    expect(ADDENDA.filter((a) => none.includes(a))).toEqual([])
  })

  test("gemini and open models get only their own addendum", () => {
    const gemini = SystemPrompt.provider(model("gemini-2.5-flash"), undefined, { surface: "chat" }).join("\n")
    expect(ADDENDA.filter((a) => gemini.includes(a))).toEqual([ADDENDUM_GEMINI])
    for (const id of ["qwen3-coder", "kimi-k2", "unknown-model"]) {
      const text = SystemPrompt.provider(model(id)).join("\n")
      expect(ADDENDA.filter((a) => text.includes(a))).toEqual([ADDENDUM_OPEN])
    }
  })
})

describe("bot turns", () => {
  const PERSONA = "# Bot identity\n\nYou are A://, Eoj's main bot."
  const bot = { id: "agent-1", name: "A://" }

  async function botHeader(ctx: SystemPrompt.PromptContext) {
    const system = await composeTurnSystem({
      userSystem: `+${PERSONA}`,
      bot,
      environment: () => ["<env>Working directory: /w</env>"],
      instructions: () => ["should not load"],
      workspace: () => ["should not load"],
    })
    return headerParts({
      bot,
      provider: () => SystemPrompt.provider(claude, undefined, ctx),
      botGuidance: () => SystemPrompt.bot(undefined, ctx),
      system,
    }).join("\n")
  }

  test("persona first, then bot guidance and safety; no base identity", async () => {
    for (const ctx of [{ surface: "chat" as const }, { surface: "code" as const }, {}]) {
      const header = await botHeader(ctx)
      expect(header.startsWith(PERSONA)).toBe(true)
      expect(header).toContain(BOT)
      expect(header).toContain(SAFETY)
      expect(header.indexOf(BOT)).toBeGreaterThan(header.indexOf(PERSONA))
      expect(header).not.toContain(firstLine(BASE_CHAT))
      expect(header).not.toContain(firstLine(BASE_CODE))
    }
  })

  test("capabilities only on chat-family surfaces", () => {
    for (const surface of CHAT_SURFACES) {
      expect(SystemPrompt.bot(undefined, { surface }).join("\n")).toContain(CAPABILITIES)
    }
    expect(SystemPrompt.bot(undefined, { surface: "code" }).join("\n")).not.toContain(CAPABILITIES)
    expect(SystemPrompt.bot().join("\n")).not.toContain(CAPABILITIES)
  })
})

describe("voice and phone blocks from turn metadata", () => {
  test("reply_mode voice adds the voice block on bot and non-bot turns", () => {
    const ctx = SystemPrompt.context("chat", { reply_mode: "voice" })
    expect(SystemPrompt.provider(claude, undefined, ctx)).toContain(VOICE)
    expect(SystemPrompt.bot(undefined, ctx)).toContain(VOICE)
    expect(SystemPrompt.provider(claude, undefined, ctx)).not.toContain(PHONE)
  })

  test("form_factor phone adds the phone block on bot and non-bot turns", () => {
    const ctx = SystemPrompt.context("chat", { form_factor: "phone" })
    expect(SystemPrompt.provider(claude, undefined, ctx)).toContain(PHONE)
    expect(SystemPrompt.bot(undefined, ctx)).toContain(PHONE)
    expect(SystemPrompt.bot(undefined, ctx)).not.toContain(VOICE)
  })

  test("no metadata or other values add neither", () => {
    for (const metadata of [undefined, {}, { reply_mode: "text", form_factor: "desktop" }]) {
      const ctx = SystemPrompt.context("chat", metadata)
      const text = [...SystemPrompt.provider(claude, undefined, ctx), ...SystemPrompt.bot(undefined, ctx)].join("\n")
      expect(text).not.toContain(VOICE)
      expect(text).not.toContain(PHONE)
    }
  })
})

describe("Codex OAuth instructions", () => {
  test("instructions() is provider() joined, mode included", () => {
    const ctx = { surface: "chat" as const, formFactor: "phone" as const }
    expect(SystemPrompt.instructions(model("gpt-5.1"), "plan", ctx)).toBe(
      SystemPrompt.provider(model("gpt-5.1"), "plan", ctx).join("\n\n"),
    )
    expect(SystemPrompt.instructions(model("gpt-5.1"), "plan", ctx)).toContain(PLAN_MODE)
  })

  test("headerParts with codex leaves the stack to instructions (no duplicate)", () => {
    const system = ["<env>Working directory: /w</env>"]
    const plain = headerParts({
      codex: true,
      provider: () => SystemPrompt.provider(model("gpt-5.1")),
      botGuidance: () => SystemPrompt.bot(),
      system,
    }).join("\n")
    const instructions = SystemPrompt.instructions(model("gpt-5.1"))
    expect(count(plain + instructions, SAFETY)).toBe(1)
    expect(count(plain + instructions, BASE_CODE)).toBe(1)

    const bot = { id: "agent-1" }
    const botSystem = headerParts({
      bot,
      codex: true,
      provider: () => SystemPrompt.provider(model("gpt-5.1")),
      botGuidance: () => SystemPrompt.bot(),
      system,
    }).join("\n")
    const botInstructions = SystemPrompt.bot().join("\n")
    expect(count(botSystem + botInstructions, BOT)).toBe(1)
    expect(count(botSystem + botInstructions, SAFETY)).toBe(1)
  })
})

describe("effectiveSurface", () => {
  type Row = { surface?: SystemPrompt.Surface; parentID?: string }
  const rows: Record<string, Row> = {
    root: { surface: "chat" },
    child: { parentID: "root" },
    grandchild: { parentID: "child" },
    orphan: { parentID: "missing" },
    loopA: { parentID: "loopB" },
    loopB: { parentID: "loopA" },
    own: { surface: "code", parentID: "root" },
  }
  const getParent = async (id: string) => {
    const row = rows[id]
    if (!row) throw new Error(`Session not found: ${id}`)
    return row
  }

  test("a child inherits its root's surface", async () => {
    expect(await SystemPrompt.effectiveSurface(rows.child!, getParent)).toBe("chat")
    expect(await SystemPrompt.effectiveSurface(rows.grandchild!, getParent)).toBe("chat")
  })

  test("a session's own surface wins; a missing parent or a cycle gives none", async () => {
    expect(await SystemPrompt.effectiveSurface(rows.own!, getParent)).toBe("code")
    expect(await SystemPrompt.effectiveSurface(rows.orphan!, getParent)).toBeUndefined()
    expect(await SystemPrompt.effectiveSurface(rows.loopA!, getParent)).toBeUndefined()
    expect(await SystemPrompt.effectiveSurface({}, getParent)).toBeUndefined()
  })
})

describe("byte caps (PLAN.md token budgets)", () => {
  const caps: [string, string, number][] = [
    ["base-chat", BASE_CHAT, 5_600],
    ["capabilities", CAPABILITIES, 8_400],
    ["safety", SAFETY, 1_800],
    ["base-code", BASE_CODE, 10_800],
    ["addendum-gpt", ADDENDUM_GPT, 700],
    ["addendum-gemini", ADDENDUM_GEMINI, 700],
    ["addendum-open", ADDENDUM_OPEN, 700],
    ["voice", VOICE, 1_300],
    ["phone", PHONE, 700],
  ]
  for (const [name, text, cap] of caps) {
    test(`${name} ≤ ${cap} bytes`, () => {
      expect(Buffer.byteLength(text, "utf8")).toBeLessThanOrEqual(cap)
    })
  }
})
