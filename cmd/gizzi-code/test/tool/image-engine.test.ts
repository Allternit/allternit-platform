import { beforeEach, describe, expect, test } from "bun:test"
import {
  buildEnginePrompt,
  claimImageJob,
  resetImageEngineState,
  resolveImageJob,
  type EngineResult,
  type ImageEngine,
} from "../../src/runtime/tools/builtins/image-engine"

const png = "data:image/png;base64,iVBORw0KGgo="
const ok: EngineResult = { status: "ok", image: png, width: 1024, height: 1024 }

function engine(id: ImageEngine["id"], results: EngineResult[], prompts: string[] = []): ImageEngine {
  let i = 0
  return {
    id,
    run: async ({ prompt }) => {
      prompts.push(prompt)
      return results[Math.min(i++, results.length - 1)]!
    },
  }
}
const run = (engines: ImageEngine[], now = () => 0) => resolveImageJob({ engines, prompt: "a red fox", now })

beforeEach(resetImageEngineState)

describe("resolveImageJob", () => {
  test("success returns the first engine's image", async () => {
    const r = await run([engine("codex", [ok]), engine("chatgpt_browser", [ok])])
    expect(r).toEqual({ kind: "image", engine: "codex", image: png, width: 1024, height: 1024 })
  })
  test("not_started falls to the next engine", async () => {
    const r = await run([engine("codex", [{ status: "not_started" }]), engine("chatgpt_browser", [ok])])
    expect(r).toMatchObject({ kind: "image", engine: "chatgpt_browser" })
  })
  test("every engine not_started gives no_engine", async () => {
    expect(await run([engine("codex", [{ status: "not_started" }]), engine("chatgpt_browser", [{ status: "not_started" }])])).toEqual({ kind: "no_engine" })
  })
  test("definite stops and never reaches the next engine", async () => {
    const second: string[] = []
    const r = await run([engine("codex", [{ status: "definite", error: "Bad image", code: "invalid_image" }]), engine("chatgpt_browser", [ok], second)])
    expect(r).toEqual({ kind: "error", engine: "codex", code: "invalid_image", message: "Bad image" })
    expect(second).toEqual([])
  })
  test("ambiguous stops with the check message and does not fall through", async () => {
    const second: string[] = []
    const r = await run([engine("codex", [{ status: "ambiguous" }]), engine("chatgpt_browser", [ok], second)])
    expect(r).toMatchObject({ kind: "ambiguous", engine: "codex", code: "ambiguous" })
    expect((r as { message: string }).message).toContain("Check before trying again")
    expect(second).toEqual([])
  })
  test("an engine that throws is ambiguous", async () => {
    const boom: ImageEngine = { id: "codex", run: async () => { throw new Error("pipe closed") } }
    expect((await run([boom, engine("chatgpt_browser", [ok])])).kind).toBe("ambiguous")
  })
  test("a bad image from an engine is a definite invalid_image", async () => {
    const r = await run([engine("codex", [{ status: "ok", image: "http://x", width: 1, height: 1 }])])
    expect(r).toMatchObject({ kind: "error", code: "invalid_image" })
  })
  test("usage_limit cooldown skips the engine until resetsAt, then expires", async () => {
    const limited: EngineResult = { status: "definite", error: "Limit reached", code: "usage_limit", resetsAt: 1000 }
    const codexPrompts: string[] = []
    const codex = engine("codex", [limited, ok], codexPrompts)
    const browser = engine("chatgpt_browser", [ok])
    expect(await run([codex, browser], () => 0)).toMatchObject({ kind: "error", code: "usage_limit" })
    expect(await run([codex, browser], () => 500)).toMatchObject({ kind: "image", engine: "chatgpt_browser" })
    expect(codexPrompts.length).toBe(1)
    expect(await run([codex, browser], () => 1000)).toMatchObject({ kind: "image", engine: "codex" })
    expect(codexPrompts.length).toBe(2)
  })
  test("usage_limit without resetsAt sets no cooldown", async () => {
    const codex = engine("codex", [{ status: "definite", error: "Limit", code: "usage_limit" }, ok])
    await run([codex])
    expect(await run([codex])).toMatchObject({ kind: "image", engine: "codex" })
  })
  test("aborted signal returns cancelled before any engine runs", async () => {
    const c = new AbortController()
    c.abort()
    const prompts: string[] = []
    const r = await resolveImageJob({ engines: [engine("codex", [ok], prompts)], prompt: "x", signal: c.signal, now: () => 0 })
    expect(r).toMatchObject({ kind: "error", code: "cancelled" })
    expect(prompts).toEqual([])
  })
})

describe("claimImageJob", () => {
  test("only the first claim per user message wins", () => {
    expect(claimImageJob("msg_1")).toBe(true)
    expect(claimImageJob("msg_1")).toBe(false)
    expect(claimImageJob("msg_2")).toBe(true)
  })
})

describe("buildEnginePrompt", () => {
  test("brief alone is verbatim", () => {
    expect(buildEnginePrompt("  a fox\n")).toBe("  a fox\n")
  })
  test("earlier briefs follow as labelled context, newest last", () => {
    const p = buildEnginePrompt("make it blue", ["a fox", "a fox in snow"])
    expect(p.startsWith("make it blue")).toBe(true)
    expect(p).toContain("Earlier picture requests")
    expect(p.indexOf("- a fox\n")).toBeLessThan(p.indexOf("- a fox in snow"))
    expect(p.endsWith("- a fox in snow")).toBe(true)
  })
  test("total stays within 16000, dropping oldest first", () => {
    const old = "o".repeat(6000)
    const mid = "m".repeat(6000)
    const newest = "n".repeat(6000)
    const p = buildEnginePrompt("brief", [old, mid, newest])
    expect(p.length).toBeLessThanOrEqual(16_000)
    expect(p).not.toContain(old)
    expect(p).toContain(mid)
    expect(p).toContain(newest)
    expect(p.endsWith(newest)).toBe(true)
  })
  test("an oversize brief is cut to the cap with no context", () => {
    const p = buildEnginePrompt("b".repeat(20_000), ["x"])
    expect(p.length).toBe(16_000)
  })
})
