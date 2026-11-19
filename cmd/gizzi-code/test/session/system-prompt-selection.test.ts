import { describe, expect, test } from "bun:test"
import fs from "node:fs"
import path from "node:path"
import { SystemPrompt } from "@/runtime/session/system"
import { composeTurnSystem, headerParts } from "@/runtime/session/system-header"
import BASE_CHAT from "@/runtime/session/prompt/base-chat.txt"
import BASE_CODE from "@/runtime/session/prompt/base-code.txt"
import CAPABILITIES from "@/runtime/session/prompt/capabilities.txt"
import SAFETY from "@/runtime/session/prompt/safety.txt"
import CARE from "@/runtime/session/prompt/care.txt"
import BOT from "@/runtime/session/prompt/bot.txt"
import VOICE from "@/runtime/session/prompt/voice.txt"
import PHONE from "@/runtime/session/prompt/phone.txt"
import PLAN_MODE from "@/runtime/session/prompt/plan-mode.txt"
import BUILD_MODE from "@/runtime/session/prompt/build-mode.txt"
import ADDENDUM_GPT from "@/runtime/session/prompt/addendum-gpt.txt"
import ADDENDUM_GEMINI from "@/runtime/session/prompt/addendum-gemini.txt"
import ADDENDUM_OPEN from "@/runtime/session/prompt/addendum-open.txt"
import DUET from "@/runtime/session/prompt/duet.txt"
import DELEGATION from "@/runtime/session/prompt/delegation.txt"
import PROJECT_THREAD from "@/runtime/session/prompt/project-thread.txt"
import COMPUTER_USE from "@/runtime/session/prompt/computer-use.txt"
import UNATTENDED from "@/runtime/session/prompt/unattended.txt"
import MEMORY_USE from "@/runtime/session/prompt/memory-use.txt"
import MODE_AUTO from "@/runtime/session/prompt/mode-auto.txt"
import MODE_ASK from "@/runtime/session/prompt/mode-ask.txt"
import BACKGROUND_CODE from "@/runtime/session/prompt/background-code.txt"
import ADDENDUM_ONDEVICE from "@/runtime/session/prompt/addendum-ondevice.txt"
import VOICE_TAGS from "@/runtime/session/prompt/voice-tags.txt"
import { ALLTERNIT_DEVICES_PROVIDER_ID } from "@/runtime/providers/discovery/allternit-devices"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Automation } from "@/runtime/primitives/automation"

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
      expect(text).toContain(CARE)
      expect(text).not.toContain(BASE_CODE)
    }
  })

  test("code and an unset surface get the Gizzi Code stack", () => {
    for (const ctx of [{ surface: "code" as const }, {}, undefined]) {
      const text = SystemPrompt.provider(claude, undefined, ctx).join("\n")
      expect(text).toContain(BASE_CODE)
      expect(text).toContain(SAFETY)
      expect(text).not.toContain(CARE)
      expect(text).not.toContain(BASE_CHAT)
      expect(text).not.toContain(CAPABILITIES)
    }
  })

  test("order: base, capabilities, safety, care, addendum, mode, voice, phone", () => {
    const parts = SystemPrompt.provider(model("gpt-5.1"), "plan", { surface: "chat", replyMode: "voice", formFactor: "phone" })
    expect(parts).toEqual([BASE_CHAT, CAPABILITIES, SAFETY, CARE, ADDENDUM_GPT, PLAN_MODE, VOICE, PHONE])
  })
})

describe("2026-11-16 audit: no vendor names or Allternit Shell in prompts", () => {
  const VENDOR = /claude|anthropic|chatgpt|openai|gemini|grok|codex|opencode|duet|allternit shell/i
  const dirs = [
    path.join(import.meta.dir, "../../src/runtime/session/prompt"),
    path.join(import.meta.dir, "../../src/runtime/loop/prompts"),
    path.join(import.meta.dir, "../../src/runtime/tools/builtins"),
  ]
  const files = dirs.flatMap((dir) =>
    fs.readdirSync(dir).filter((name) => name.endsWith(".txt")).map((name) => path.join(dir, name)),
  )

  test("finds the prompt files", () => {
    expect(files.length).toBeGreaterThan(10)
    const names = files.map((file) => path.basename(file))
    for (const name of ["duet.txt", "delegation.txt", "project-thread.txt", "computer-use.txt", "care.txt", "bash.txt", "unattended.txt", "memory-use.txt", "mode-auto.txt", "mode-ask.txt", "background-code.txt", "addendum-ondevice.txt", "voice-tags.txt"]) {
      expect(names).toContain(name)
    }
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
      expect(header).toContain(CARE)
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

describe("unattended block from automation_run metadata", () => {
  const run = { [Automation.RUN_METADATA_KEY]: { id: "job-1", runId: "run-1" } }

  test("the key matches the scheduler's", () => {
    expect(SystemPrompt.AUTOMATION_RUN_KEY).toBe(Automation.RUN_METADATA_KEY)
  })

  test("a scheduler's turn gets unattended.txt on bot and non-bot turns, before voice and phone", () => {
    const ctx = SystemPrompt.context("chat", { ...run, reply_mode: "voice", form_factor: "phone" })
    expect(ctx.automationRun).toBe(true)
    const provider = SystemPrompt.provider(claude, undefined, ctx)
    expect(provider.slice(-3)).toEqual([UNATTENDED, VOICE, PHONE])
    expect(SystemPrompt.bot(undefined, ctx).slice(-3)).toEqual([UNATTENDED, VOICE, PHONE])
    expect(SystemPrompt.provider(claude, undefined, SystemPrompt.context("code", run))).toEqual([BASE_CODE, SAFETY, UNATTENDED])
  })

  test("no flag, or an empty one, adds no unattended block", () => {
    for (const metadata of [undefined, {}, { [Automation.RUN_METADATA_KEY]: null }, { reply_mode: "voice" }]) {
      const ctx = SystemPrompt.context("chat", metadata)
      expect(ctx.automationRun).toBeUndefined()
      const text = [...SystemPrompt.provider(claude, undefined, ctx), ...SystemPrompt.bot(undefined, ctx)].join("\n")
      expect(text).not.toContain(UNATTENDED)
    }
  })
})

describe("autonomy blocks from the session permission mode (PR2)", () => {
  const AUTO_MODES = ["auto", "acceptEdits", "dontAsk", "yolo", "bypassPermissions"] as const
  const ASK_MODES = ["default", "manual"] as const

  test("every permission mode is mapped: auto, ask, or none for plan", () => {
    for (const mode of PermissionNext.Mode.options) {
      const expected = (AUTO_MODES as readonly string[]).includes(mode)
        ? "auto"
        : (ASK_MODES as readonly string[]).includes(mode)
          ? "ask"
          : undefined
      expect(SystemPrompt.autonomy(mode)).toBe(expected)
    }
    expect(SystemPrompt.autonomy("plan")).toBeUndefined()
    expect(SystemPrompt.autonomy(undefined)).toBeUndefined()
  })

  test("auto modes load mode-auto after the mode reminder; ask modes load mode-ask", () => {
    for (const permissionMode of AUTO_MODES) {
      const ctx = SystemPrompt.context("code", {}, { permissionMode })
      expect(ctx.permissionMode).toBe(permissionMode)
      expect(SystemPrompt.provider(claude, "build", ctx)).toEqual([BASE_CODE, SAFETY, BUILD_MODE, MODE_AUTO])
    }
    for (const permissionMode of ASK_MODES) {
      const ctx = SystemPrompt.context("chat", { reply_mode: "voice" }, { permissionMode })
      expect(SystemPrompt.provider(claude, undefined, ctx)).toEqual([BASE_CHAT, CAPABILITIES, SAFETY, CARE, MODE_ASK, VOICE])
    }
  })

  test("no mode block in plan, without a mode, or on a run nobody watches", () => {
    const blocks = (parts: string[]) => parts.filter((p) => p === MODE_AUTO || p === MODE_ASK)
    expect(blocks(SystemPrompt.provider(claude, "plan", SystemPrompt.context("code", {}, { permissionMode: "auto" })))).toEqual([])
    expect(blocks(SystemPrompt.provider(claude, undefined, SystemPrompt.context("code", {}, { permissionMode: "plan" })))).toEqual([])
    expect(blocks(SystemPrompt.provider(claude, undefined, SystemPrompt.context("code", {})))).toEqual([])
    for (const metadata of [{ [Automation.RUN_METADATA_KEY]: { id: "job-1" } }, { run_kind: "background_code" }]) {
      expect(blocks(SystemPrompt.provider(claude, undefined, SystemPrompt.context("code", metadata, { permissionMode: "default" })))).toEqual([])
    }
  })

  test("bot turns keep bot.txt's approval rule and get no mode block", () => {
    const parts = SystemPrompt.bot(undefined, SystemPrompt.context("chat", {}, { permissionMode: "auto" }))
    expect(parts).not.toContain(MODE_AUTO)
    expect(parts).not.toContain(MODE_ASK)
  })

  test("the TUI loads the same blocks from its own permission mode", () => {
    const src = fs.readFileSync(path.join(import.meta.dir, "../../src/cli/ui/ink-app/constants/prompts.ts"), "utf8")
    expect(src).toContain("@/runtime/session/prompt/mode-auto.txt")
    expect(src).toContain("@/runtime/session/prompt/mode-ask.txt")
    expect(src).toContain("getPermissionModeSection(permissionMode)")
  })
})

describe("background-code block from the run_kind hint (PR7)", () => {
  test("run_kind background_code loads background-code after unattended, before voice", () => {
    const ctx = SystemPrompt.context("code", { run_kind: SystemPrompt.BACKGROUND_CODE_RUN_KIND })
    expect(ctx.backgroundCode).toBe(true)
    expect(SystemPrompt.provider(claude, undefined, ctx)).toEqual([BASE_CODE, SAFETY, BACKGROUND_CODE])
    const both = SystemPrompt.context("code", { run_kind: "background_code", [Automation.RUN_METADATA_KEY]: { id: "j" }, reply_mode: "voice" })
    expect(SystemPrompt.provider(claude, undefined, both).slice(-3)).toEqual([UNATTENDED, BACKGROUND_CODE, VOICE])
    expect(SystemPrompt.bot(undefined, ctx)).toContain(BACKGROUND_CODE)
  })

  test("other values, or none, add no background-code block", () => {
    for (const metadata of [undefined, {}, { run_kind: "interactive" }, { run_kind: true }]) {
      const ctx = SystemPrompt.context("code", metadata as Record<string, unknown> | undefined)
      expect(ctx.backgroundCode).toBeUndefined()
      expect(SystemPrompt.provider(claude, undefined, ctx)).not.toContain(BACKGROUND_CODE)
    }
  })
})

describe("on-device addendum (PR11f)", () => {
  const device = (id: string) => ({ api: { id }, providerID: ALLTERNIT_DEVICES_PROVIDER_ID })

  test("the provider id matches discovery", () => {
    expect(SystemPrompt.ON_DEVICE_PROVIDER_ID).toBe(ALLTERNIT_DEVICES_PROVIDER_ID)
  })

  test("a device model gets addendum-ondevice instead of addendum-open", () => {
    for (const id of ["qwen3-4b", "llama-3.2-3b", "local-model"]) {
      expect(SystemPrompt.family(device(id))).toBe("ondevice")
      const parts = SystemPrompt.provider(device(id), undefined, { surface: "chat" })
      expect(parts).toContain(ADDENDUM_ONDEVICE)
      expect(parts).not.toContain(ADDENDUM_OPEN)
    }
    expect(SystemPrompt.family({ api: { id: "qwen3-4b" }, providerID: "ollama" })).toBe("open")
  })
})

describe("voice-tags block from the voice_tags hint (PR9)", () => {
  test("voice_tags on loads voice-tags right after the voice block", () => {
    const ctx = SystemPrompt.context("chat", { reply_mode: "voice", voice_tags: "on", form_factor: "phone" })
    expect(ctx.voiceTags).toBe(true)
    expect(SystemPrompt.provider(claude, undefined, ctx).slice(-3)).toEqual([VOICE, VOICE_TAGS, PHONE])
    expect(SystemPrompt.bot(undefined, ctx).slice(-3)).toEqual([VOICE, VOICE_TAGS, PHONE])
  })

  test("no tags without a spoken reply, without the hint, or with another value", () => {
    for (const metadata of [{ voice_tags: "on" }, { reply_mode: "voice" }, { reply_mode: "voice", voice_tags: "off" }]) {
      const ctx = SystemPrompt.context("chat", metadata)
      expect(SystemPrompt.provider(claude, undefined, ctx)).not.toContain(VOICE_TAGS)
    }
    expect(SystemPrompt.VOICE_TAGS_KEY).toBe("voice_tags")
  })
})

describe("memory-use block", () => {
  test("follows the memory tools block when memory is on", () => {
    const section = SystemPrompt.memorySection("# Memory\n\n- [Prefers tea](tea.md)")
    expect(section).toContain(MEMORY_USE.trim())
    expect(section.indexOf(MEMORY_USE.trim())).toBeGreaterThan(section.indexOf("## Memory tools"))
    expect(section.endsWith(MEMORY_USE.trim())).toBe(true)
  })

  test("is left out when memory is off", () => {
    const section = SystemPrompt.memorySection("")
    expect(section).toContain("Persistent memory is turned off")
    expect(section).not.toContain(MEMORY_USE.trim())
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
    ["capabilities", CAPABILITIES, 7_400],
    ["bot", BOT, 3_200],
    ["duet", DUET, 1_800],
    ["delegation", DELEGATION, 1_000],
    ["project-thread", PROJECT_THREAD, 1_300],
    ["computer-use", COMPUTER_USE, 3_400],
    ["care", CARE, 1_500],
    ["safety", SAFETY, 2_900],
    ["base-code", BASE_CODE, 11_200],
    ["addendum-gpt", ADDENDUM_GPT, 700],
    ["addendum-gemini", ADDENDUM_GEMINI, 700],
    ["addendum-open", ADDENDUM_OPEN, 700],
    ["voice", VOICE, 2_000],
    ["phone", PHONE, 700],
    ["unattended", UNATTENDED, 700],
    ["memory-use", MEMORY_USE, 1_300],
    ["mode-auto", MODE_AUTO, 700],
    ["mode-ask", MODE_ASK, 400],
    ["background-code", BACKGROUND_CODE, 700],
    ["addendum-ondevice", ADDENDUM_ONDEVICE, 400],
    ["voice-tags", VOICE_TAGS, 300],
  ]
  for (const [name, text, cap] of caps) {
    test(`${name} ≤ ${cap} bytes`, () => {
      expect(Buffer.byteLength(text, "utf8")).toBeLessThanOrEqual(cap)
    })
  }
})

// Tool texts tracked on main before G1 (they have no cap).
const KNOWN_TOOL_TEXTS = [
  "agent-batch.txt",
  "agent-communicate.txt",
  "agent-control.txt",
  "apply_patch.txt",
  "artifact-check.txt",
  "artifact-claim.txt",
  "artifact-create.txt",
  "artifact-read.txt",
  "artifact-release.txt",
  "artifact-suggest.txt",
  "artifact-update.txt",
  "ask-user-later.txt",
  "automation.txt",
  "bash.txt",
  "batch.txt",
  "browser-tasks.txt",
  "capture-screen-context.txt",
  "codesearch.txt",
  "connector-search.txt",
  "connector-suggest.txt",
  "context-remaining.txt",
  "current-time.txt",
  "edit.txt",
  "folder-access.txt",
  "glob.txt",
  "grep.txt",
  "history-recall.txt",
  "lookup.txt",
  "ls.txt",
  "lsp.txt",
  "media-generate.txt",
  "memory-list.txt",
  "memory-recall.txt",
  "memory-write.txt",
  "multiedit.txt",
  "pane-artifact.txt",
  "pane-browser.txt",
  "plan-enter.txt",
  "plan-exit.txt",
  "project-status.txt",
  "question.txt",
  "read.txt",
  "resource.txt",
  "run_code.txt",
  "send-user-message.txt",
  "settings.txt",
  "sleep.txt",
  "symbol-edit.txt",
  "task.txt",
  "thread-read.txt",
  "todoread.txt",
  "todowrite.txt",
  "tool-search.txt",
  "wait-agents.txt",
  "webfetch.txt",
  "websearch.txt",
  "write.txt",
]

describe("new tool texts (G1 rule)", () => {
  // Tool texts that exist on main are exempt; any other builtin .txt is new and must stay small.
  const EXISTING = new Set<string>(KNOWN_TOOL_TEXTS)
  const dir = path.join(import.meta.dir, "../../src/runtime/tools/builtins")
  for (const name of fs.readdirSync(dir).filter((n) => n.endsWith(".txt") && !EXISTING.has(n))) {
    test(`${name} ≤ 900 bytes`, () => {
      expect(fs.statSync(path.join(dir, name)).size).toBeLessThanOrEqual(900)
    })
  }
})

describe("wave C2 rules", () => {
  test("computer-use names pane_browser refs and the plan step", () => {
    expect(COMPUTER_USE).toContain("refs from `read_page` or `find`")
    expect(COMPUTER_USE).toContain("send `plan` first")
  })
  test("voice captures the screen before asking and never drives the computer", () => {
    expect(VOICE).toContain("capture the screen before asking the user to describe what they see")
    expect(VOICE).toContain("again after it changes")
    expect(VOICE).toContain("Voice never drives the computer.")
  })
})

describe("bot blocks by tool signal (PLAN-bots.md selection)", () => {
  const SIGNAL_BLOCKS = [DUET, DELEGATION, PROJECT_THREAD, COMPUTER_USE]
  const all = { appBot: true, projectSend: true, projectThread: true, computerUse: true }

  test("a bot turn with no signals gets bot.txt and none of the signal blocks", () => {
    for (const ctx of [{}, { surface: "chat" as const }, SystemPrompt.context("chat", {}, { toolIds: ["read", "webfetch"] })]) {
      const parts = SystemPrompt.bot(undefined, ctx)
      expect(parts).toContain(BOT)
      expect(SIGNAL_BLOCKS.filter((block) => parts.includes(block))).toEqual([])
    }
  })

  test("bot order: mode, bot, duet, delegation, project-thread, capabilities, safety, care, computer-use, voice, phone", () => {
    const parts = SystemPrompt.bot("plan", { surface: "chat", replyMode: "voice", formFactor: "phone", ...all })
    expect(parts).toEqual([PLAN_MODE, BOT, DUET, DELEGATION, PROJECT_THREAD, CAPABILITIES, SAFETY, CARE, COMPUTER_USE, VOICE, PHONE])
    // Code surface: the same order with no capabilities.
    expect(SystemPrompt.bot(undefined, { surface: "code", ...all })).toEqual([
      BOT,
      DUET,
      DELEGATION,
      PROJECT_THREAD,
      SAFETY,
      CARE,
      COMPUTER_USE,
    ])
  })

  test("each flag adds exactly its own block on a bot turn", () => {
    const cases: [keyof typeof all, string, string[]][] = [
      ["appBot", DUET, [BOT, DUET, CAPABILITIES, SAFETY, CARE]],
      ["projectSend", DELEGATION, [BOT, DELEGATION, CAPABILITIES, SAFETY, CARE]],
      ["projectThread", PROJECT_THREAD, [BOT, PROJECT_THREAD, CAPABILITIES, SAFETY, CARE]],
      ["computerUse", COMPUTER_USE, [BOT, CAPABILITIES, SAFETY, CARE, COMPUTER_USE]],
    ]
    for (const [flag, block, expected] of cases) {
      const parts = SystemPrompt.bot(undefined, { surface: "chat", [flag]: true } as SystemPrompt.PromptContext)
      expect(parts).toEqual(expected)
      expect(SIGNAL_BLOCKS.filter((b) => parts.includes(b))).toEqual([block])
    }
  })

  test("normal order: base, capabilities, safety, care, addendum, mode, project-thread, computer-use, voice, phone", () => {
    const parts = SystemPrompt.provider(model("gpt-5.1"), "plan", {
      surface: "chat",
      replyMode: "voice",
      formFactor: "phone",
      ...all,
    })
    expect(parts).toEqual([BASE_CHAT, CAPABILITIES, SAFETY, CARE, ADDENDUM_GPT, PLAN_MODE, PROJECT_THREAD, COMPUTER_USE, VOICE, PHONE])
    expect(SystemPrompt.provider(claude, undefined, { projectThread: true })).toEqual([BASE_CODE, SAFETY, PROJECT_THREAD])
    expect(SystemPrompt.provider(claude, undefined, { computerUse: true })).toEqual([BASE_CODE, SAFETY, COMPUTER_USE])
  })

  test("provider() and instructions() never carry duet or delegation, whatever the flags", () => {
    for (const surface of [...CHAT_SURFACES, "code" as const, undefined]) {
      const ctx = { surface, ...all }
      const text = SystemPrompt.provider(claude, "build", ctx).join("\n") + SystemPrompt.instructions(model("gpt-5.1"), undefined, ctx)
      expect(text).not.toContain(DUET)
      expect(text).not.toContain(DELEGATION)
    }
  })

  test("context(): tool ids set appBot, projectSend and computerUse", () => {
    expect(SystemPrompt.context("chat", {}, { toolIds: ["suggest_task"] }).appBot).toBe(true)
    expect(SystemPrompt.context("chat", {}, { toolIds: ["grant_task_permission"] }).appBot).toBe(true)
    expect(SystemPrompt.context("chat", {}, { toolIds: ["project_send"] }).projectSend).toBe(true)
    // project_list alone is not project_send.
    expect(SystemPrompt.context("chat", {}, { toolIds: ["project_list"] }).projectSend).toBeUndefined()
    for (const id of ["pane_browser", "browser", "computer", "computer_v2", "computers", "browser_tasks"]) {
      expect(SystemPrompt.context("code", {}, { toolIds: [id] }).computerUse).toBe(true)
    }
    // Exact ids, not substrings: toolset members are actions, never tool ids.
    for (const id of ["browser_use", "mcp__browser__click", "computer_status", "left_click"]) {
      expect(SystemPrompt.context("code", {}, { toolIds: [id] }).computerUse).toBeUndefined()
    }
    expect(SystemPrompt.context("chat", { reply_mode: "voice" })).toEqual({ surface: "chat", replyMode: "voice" })
  })

  test("context(): project thread from the roster caller (never a main chat) or the server hint", () => {
    const thread = (caller: SystemPrompt.TurnSignals["caller"], metadata: Record<string, unknown> = {}) =>
      SystemPrompt.context("chat", metadata, { toolIds: ["suggest_task"], caller }).projectThread
    expect(thread({ projectId: "p1", mainChat: false })).toBe(true)
    expect(thread({ projectId: "p1" })).toBe(true)
    expect(thread({ projectId: "p1", mainChat: true })).toBeUndefined()
    expect(thread({ projectId: null, mainChat: false })).toBeUndefined()
    expect(thread({ mainChat: false })).toBeUndefined()
    expect(thread(null)).toBeUndefined()
    expect(thread(undefined)).toBeUndefined()
    // The internal project worker has no roster caller; the server stamps the hint.
    expect(thread(undefined, { project_role: "thread" })).toBe(true)
    expect(thread(undefined, { project_role: "lead" })).toBeUndefined()
  })

  test("bot main chat on a chat surface, with computer use, stays within 21,000 bytes", () => {
    // A main chat: suggestions, project sending and pane_browser (on every app session); not a project thread.
    const ctx = SystemPrompt.context("chat", {}, {
      toolIds: ["suggest_task", "grant_task_permission", "thread_update", "project_list", "project_send", "pane_browser"],
      caller: { projectId: "p1", mainChat: true },
    })
    expect(ctx).toMatchObject({ appBot: true, projectSend: true, computerUse: true })
    expect(ctx.projectThread).toBeUndefined()
    const parts = SystemPrompt.bot(undefined, ctx)
    expect(parts).toEqual([BOT, DUET, DELEGATION, CAPABILITIES, SAFETY, CARE, COMPUTER_USE])
    expect(Buffer.byteLength(parts.join("\n\n"), "utf8")).toBeLessThanOrEqual(21_000)
  })
})
