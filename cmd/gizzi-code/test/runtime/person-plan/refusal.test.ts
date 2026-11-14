import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { isPersonOnlyRef, refuseIfPersonOnly } from "../../../src/runtime/providers/person-plan/rules"
import { PersonPlanLanguageModel } from "../../../src/runtime/providers/person-plan/language-model"
import { isComposerTurn, queryPersonPlanWithStreaming } from "../../../src/cli/ui/ink-app/services/api/personPlan"
import { BotStoreError, createBot } from "../../../src/runtime/bots/bot-store"

let home: string
const saved = { config: process.env.GIZZI_CONFIG_DIR }

beforeAll(() => {
  home = mkdtempSync(join(tmpdir(), "gizzi-person-plan-"))
  // An empty config dir: no person session, so cloud plans have no token.
  process.env.GIZZI_CONFIG_DIR = home
})
afterAll(() => {
  if (saved.config === undefined) delete process.env.GIZZI_CONFIG_DIR
  else process.env.GIZZI_CONFIG_DIR = saved.config
  rmSync(home, { recursive: true, force: true })
})

async function streamText(model: PersonPlanLanguageModel, headers: Record<string, string>) {
  const { stream } = await model.doStream({ prompt: [{ role: "user", content: [{ type: "text", text: "hi" }] }], headers })
  const parts: any[] = []
  const reader = stream.getReader()
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    parts.push(value)
  }
  return parts
}

describe("which models are person-only", () => {
  test("plans and ChatGPT direct are; the Copilot CLI brain and Cloud are not", () => {
    expect(isPersonOnlyRef("plan/claude/sonnet")).toBe(true)
    expect(isPersonOnlyRef("chatgpt/gpt-6.1-sol")).toBe(true)
    expect(isPersonOnlyRef("copilot-plan/gpt-5")).toBe(true)
    // `copilot/…` is the GitHub Copilot CLI on this computer: the person's own CLI, fine headless.
    expect(isPersonOnlyRef("copilot/gpt-5")).toBe(false)
    expect(isPersonOnlyRef("allternit/fast")).toBe(false)
    expect(isPersonOnlyRef("claude-cli/sonnet")).toBe(false)
  })

  test("headless and mission runs get a plain refusal naming the model", () => {
    expect(refuseIfPersonOnly("plan/claude/sonnet", "headless")).toBe(
      "plan/claude/sonnet is your own plan. It answers only messages you send yourself in the chat, so `gizzi run` cannot use it. " +
        "Pick another model (for example an Allternit Cloud model or a CLI brain).",
    )
    expect(refuseIfPersonOnly("chatgpt/gpt-6.1-sol", "mission")).toContain("so missions cannot use it")
    expect(refuseIfPersonOnly("allternit/fast", "headless")).toBeUndefined()
    expect(refuseIfPersonOnly(undefined, "headless")).toBeUndefined()
  })
})

describe("engine turns (gizzi serve, Desktop, gizzi run)", () => {
  test("a turn without the person header never reaches the plan", async () => {
    const parts = await streamText(new PersonPlanLanguageModel("plan", "claude/sonnet"), {})
    const err = parts.find((p) => p.type === "error")
    expect(String(err.error.message)).toContain("plan/claude/sonnet is your own plan")
    expect(parts.find((p) => p.type === "finish").finishReason).toBe("error")
    expect(parts.some((p) => p.type === "text-delta")).toBe(false)
  })

  test("a person turn with only a device token or key is refused with the sign-in message", async () => {
    const parts = await streamText(new PersonPlanLanguageModel("plan", "claude/sonnet"), { "x-allternit-turn": "person" })
    expect(String(parts.find((p) => p.type === "error").error.message)).toContain("Plans answer only messages you send while signed in as yourself")
  })
})

describe("TUI turns", () => {
  const opts = (over: Record<string, unknown>) => ({ model: "plan/claude/sonnet", querySource: "repl_main_thread", isNonInteractiveSession: false, ...over }) as any

  test("only the interactive main thread is the composer", () => {
    expect(isComposerTurn(opts({}))).toBe(true)
    expect(isComposerTurn(opts({ querySource: "repl_main_thread:outputStyle:Explanatory" }))).toBe(true)
    expect(isComposerTurn(opts({ agentId: "a1" }))).toBe(false)
    expect(isComposerTurn(opts({ isNonInteractiveSession: true }))).toBe(false)
    expect(isComposerTurn(opts({ querySource: "compact" }))).toBe(false)
    expect(isComposerTurn(opts({ querySource: "session_memory" }))).toBe(false)
  })

  test("a sub-agent turn gets the refusal as an error message", async () => {
    const out: any[] = []
    for await (const m of queryPersonPlanWithStreaming({ messages: [], signal: new AbortController().signal, options: opts({ agentId: "a1" }) })) out.push(m)
    expect(out).toHaveLength(1)
    expect(out[0].isApiErrorMessage).toBe(true)
    expect(out[0].message.content[0].text).toContain("so sub-agents cannot use it")
  })
})

describe("bots", () => {
  test("a bot cannot be pinned to a plan model", async () => {
    await expect(createBot({ name: "plan-bot", title: "Plan bot", model: "chatgpt/gpt-6.1-sol" })).rejects.toThrow(BotStoreError)
    await expect(createBot({ name: "plan-bot", title: "Plan bot", model: "chatgpt/gpt-6.1-sol" })).rejects.toThrow(/bots cannot use it/)
  })
})
