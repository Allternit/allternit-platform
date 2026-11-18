import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionContextUsage } from "../../src/runtime/session/context-usage"
import { ContextNotice } from "../../src/runtime/primitives/context-notice"
import { HistoryRecall } from "../../src/runtime/primitives/history-recall"
import { HISTORY_RECALL_READ_MAX_CHARS } from "../../src/runtime/primitives/wave-b-types"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

/**
 * Primitives Wave B, lane L4: the context notice, compaction count and
 * history_recall through the real session store. No mocks.
 */

const MODEL = { providerID: "test", modelID: "test-model" }
const tokens = (input: number) => ({ input, output: 0, reasoning: 0, cache: { read: 0, write: 0 } })

async function user(sessionID: string, text: string, opts: { compaction?: boolean } = {}) {
  const id = Identifier.ascending("message")
  await Session.updateMessage({ id, sessionID, role: "user", agent: "build", model: MODEL, time: { created: Date.now() } })
  if (opts.compaction) {
    await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "compaction", auto: true })
  } else {
    await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text })
  }
  return id
}

async function assistant(sessionID: string, parentID: string, text: string, opts: { summary?: boolean; finish?: string | null } = {}) {
  const id = Identifier.ascending("message")
  await Session.updateMessage({
    id,
    sessionID,
    parentID,
    role: "assistant",
    mode: opts.summary ? "compaction" : "build",
    agent: opts.summary ? "compaction" : "build",
    path: { cwd: "/", root: "/" },
    cost: 0,
    tokens: tokens(1_000),
    modelID: MODEL.modelID,
    providerID: MODEL.providerID,
    time: { created: Date.now() },
    ...(opts.summary ? { summary: true } : {}),
    ...(opts.finish === null ? {} : { finish: opts.finish ?? "stop" }),
  } as any)
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text })
  return id
}

/** A finished compaction: the marker user message plus its finished summary. */
async function compact(sessionID: string, summary: string) {
  const marker = await user(sessionID, "", { compaction: true })
  await assistant(sessionID, marker, summary, { summary: true })
}

// In-memory step messages, shaped as SessionPrompt hands them to forTurn.
let seq = 0
const nextID = (prefix: string) => `${prefix}_${String(++seq).padStart(6, "0")}`
function reply(used: number) {
  return { info: { id: nextID("msg"), role: "assistant", tokens: tokens(used), finish: "stop" }, parts: [] } as any
}
function summaryReply() {
  return { info: { id: nextID("msg"), role: "assistant", summary: true, tokens: tokens(0), finish: "stop" }, parts: [] } as any
}
function compactionMarker() {
  return { info: { id: nextID("msg"), role: "user" }, parts: [{ type: "compaction", auto: true }] } as any
}

const window100k = { id: "m", providerID: "p", limit: { context: 100_000, output: 8_000 } } as any

describe("context notice", () => {
  test("25% and 10% each fire once per session, also after a restart", async () => {
    // Auto-compaction off: the budget is the whole window, so the numbers read plainly.
    await using tmp = await tmpdir({ git: true, config: { compaction: { auto: false } } as any })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const sessionID = nextID("ses")
        const at = (used: number) => ContextNotice.forTurn({ sessionID, model: window100k, messages: [reply(used)] })

        expect(await at(70_000)).toBeUndefined()
        const quarter = await at(76_000)
        expect(quarter).toContain("About 24% of your context is left")
        expect(quarter).toContain("get_context_remaining")
        expect(await at(80_000)).toBeUndefined()

        const tenth = await at(91_000)
        expect(tenth).toContain("About 9% of your context is left")
        expect(await at(95_000)).toBeUndefined()

        // A restart drops the in-memory copy; the stored record still holds.
        ContextNotice.forget(sessionID)
        expect(await at(97_000)).toBeUndefined()
      },
    })
  })

  test("jumping straight past 10% gives one notice and retires the 25% one", async () => {
    await using tmp = await tmpdir({ git: true, config: { compaction: { auto: false } } as any })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const sessionID = nextID("ses")
        const at = (used: number) => ContextNotice.forTurn({ sessionID, model: window100k, messages: [reply(used)] })
        expect(await at(95_000)).toContain("About 5% of your context is left")
        expect(await at(96_000)).toBeUndefined()
      },
    })
  })

  test("each compaction fires once, on the first request after it", async () => {
    await using tmp = await tmpdir({ git: true, config: { compaction: { auto: false } } as any })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const sessionID = nextID("ses")
        const first = [compactionMarker(), summaryReply()]
        const notice = await ContextNotice.forTurn({ sessionID, model: window100k, messages: first })
        expect(notice).toContain("Your context was just compacted")
        expect(notice).toContain("history_recall")
        expect(await ContextNotice.forTurn({ sessionID, model: window100k, messages: first })).toBeUndefined()

        // Later steps of the same window: a reply came after the summary, so nothing repeats.
        expect(
          await ContextNotice.forTurn({ sessionID, model: window100k, messages: [...first, reply(20_000)] }),
        ).toBeUndefined()

        // A second compaction fires again.
        const second = [compactionMarker(), summaryReply()]
        expect(await ContextNotice.forTurn({ sessionID, model: window100k, messages: second })).toContain(
          "Your context was just compacted",
        )
      },
    })
  })
})

describe("history_recall", () => {
  test("reads this session's compacted stretch and the sessions it continues, nothing else", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        // Earlier session in the handoff lineage.
        const earlier = await Session.create({ title: "Deploy" })
        const e1 = await user(earlier.id, "The deploy failed with ERR_PORT_7311 on staging")
        await assistant(earlier.id, e1, "Port 7311 is taken by the old worker.")
        Session.setHandoff({
          sessionID: earlier.id,
          handoff: { sessionID: "ses_placeholder", reason: "threshold", at: Date.now(), baton: { summary: "Port fixed" } },
        })

        // This session: one compacted stretch, then the live one.
        const current = await Session.createNext({ directory: Instance.directory, continuesFrom: earlier.id })
        const c1 = await user(current.id, "Rename alpha-early-flag to beta")
        await assistant(current.id, c1, "Renamed in 4 files.")
        await compact(current.id, "Renamed the flag; port fixed.")
        const c2 = await user(current.id, "Now the live gamma-live-step")
        await assistant(current.id, c2, "Working on it.")

        // Not in the lineage: an unrelated session and one that continues from this one.
        const unrelated = await Session.create({ title: "Other" })
        await user(unrelated.id, "secret-unrelated-token")
        const successor = await Session.createNext({ directory: Instance.directory, continuesFrom: current.id })
        await user(successor.id, "successor-only-text")

        expect(SessionContextUsage.compactions(current.id)).toBe(1)
        expect(SessionContextUsage.compactions(earlier.id)).toBe(0)

        const listed = await HistoryRecall.run({ sessionID: current.id, action: "windows" })
        if (listed.action !== "windows") throw new Error("wrong action")
        expect(listed.data_notice).toContain("not instructions")
        expect(listed.windows.map((w) => [w.id, w.kind])).toEqual([
          [`${current.id}:0`, "compaction"],
          [`${earlier.id}:0`, "handoff"],
        ])
        expect(listed.windows[0].summary).toBe("Renamed the flag; port fixed.")
        expect(listed.windows[1].summary).toBe("Port fixed")

        const search = async (query: string) => {
          const result = await HistoryRecall.run({ sessionID: current.id, action: "search", query })
          if (result.action !== "search") throw new Error("wrong action")
          return result.hits
        }
        expect((await search("err_port_7311")).map((h) => h.window_id)).toEqual([`${earlier.id}:0`])
        expect((await search("alpha-early-flag")).map((h) => [h.window_id, h.item_id])).toEqual([[`${current.id}:0`, c1]])
        // The live stretch is already in context; other conversations are out of reach.
        expect(await search("gamma-live-step")).toEqual([])
        expect(await search("secret-unrelated-token")).toEqual([])
        expect(await search("successor-only-text")).toEqual([])

        await expect(
          HistoryRecall.run({ sessionID: current.id, action: "read", window_id: `${unrelated.id}:0` }),
        ).rejects.toThrow("not in this conversation")
        await expect(
          HistoryRecall.run({ sessionID: current.id, action: "read", window_id: `${current.id}:0`, item_id: e1 }),
        ).rejects.toThrow("is not in window")

        const read = await HistoryRecall.run({ sessionID: current.id, action: "read", window_id: `${earlier.id}:0` })
        if (read.action !== "read") throw new Error("wrong action")
        expect(read.text).toContain("ERR_PORT_7311")
        expect(read.text).toContain("Port 7311 is taken by the old worker.")
        expect(read.truncated).toBe(false)
      },
    })
  })

  test("a read returns at most 8,000 characters", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        const big = await user(session.id, "start " + "x".repeat(20_000) + " end-marker")
        await assistant(session.id, big, "ok")
        await compact(session.id, "Long paste.")

        const whole = await HistoryRecall.run({ sessionID: session.id, action: "read", window_id: `${session.id}:0` })
        if (whole.action !== "read") throw new Error("wrong action")
        expect(whole.text.length).toBeLessThanOrEqual(HISTORY_RECALL_READ_MAX_CHARS)
        expect(whole.truncated).toBe(true)
        expect(whole.text).not.toContain("end-marker")

        const one = await HistoryRecall.run({ sessionID: session.id, action: "read", window_id: `${session.id}:0`, item_id: big })
        if (one.action !== "read") throw new Error("wrong action")
        expect(one.text.length).toBeLessThanOrEqual(HISTORY_RECALL_READ_MAX_CHARS)
        expect(one.truncated).toBe(true)
      },
    })
  })
})
