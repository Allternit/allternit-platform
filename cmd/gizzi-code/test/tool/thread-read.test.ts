// @ts-nocheck
import { afterAll, beforeEach, describe, expect, test } from "bun:test"

/**
 * Past chats (Wave D lanes T): thread_read over the real session store, and
 * thread_recent / thread_wait through the real routing client plus this
 * runtime's own plain chats (real store, real status bus). Only the HTTP
 * edge (allternit-api /bot-routing/thread/*) is stubbed. The server-side
 * wait logic (first of N, cursors, timeout 0) has its own Rust tests in
 * allternit-api thread_read_tools.rs.
 */
import { mkdtempSync } from "node:fs"
import { tmpdir as osTmpdir } from "node:os"
import path from "path"

process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
process.env.GIZZI_CONFIG_DIR = mkdtempSync(path.join(osTmpdir(), "gizzi-thread-read-"))
const { tmpdir } = await import("../fixture/fixture")
const { Instance } = await import("../../src/runtime/context/project/instance")
const { Session } = await import("../../src/runtime/session")
const { SessionStatus } = await import("../../src/runtime/session/status")
const { Identifier } = await import("../../src/shared/id/id")
const { PastChats } = await import("../../src/runtime/primitives/past-chats")
const { ThreadReadTool } = await import("../../src/runtime/tools/builtins/thread-read")

type Call = { path: string; body: any }
let calls: Call[] = []
let route: (path: string, body: any) => Response
const realFetch = globalThis.fetch

beforeEach(() => {
  calls = []
  route = () => new Response(JSON.stringify({ error: "unexpected" }), { status: 500 })
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const body = JSON.parse(String(init.body))
    calls.push({ path: url.pathname.replace(/^.*\/bot-routing/, ""), body })
    return route(url.pathname.replace(/^.*\/bot-routing/, ""), body)
  }) as typeof fetch
})

afterAll(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
})

async function say(sessionID: string, role: "user" | "assistant", text: string) {
  const id = Identifier.ascending("message")
  const base = { id, sessionID, time: { created: Date.now() } }
  await Session.updateMessage(
    role === "user"
      ? { ...base, role, agent: "build", model: { providerID: "claude-cli", modelID: "sonnet" } }
      : {
          ...base,
          role,
          parentID: id,
          mode: "build",
          agent: "build",
          path: { cwd: "/", root: "/" },
          cost: 0,
          tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
          modelID: "sonnet",
          providerID: "claude-cli",
          finish: "stop",
        },
  )
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text })
}

/** A chat of `n` turns: "turn 0" (user), "turn 1" (assistant), ... */
async function chat(title: string, n: number, text = (i: number) => `turn ${i}`) {
  const s = await Session.create({ title })
  for (let i = 0; i < n; i++) await say(s.id, i % 2 === 0 ? "user" : "assistant", text(i))
  return s.id
}

async function inProject<T>(fn: () => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return Instance.provide({ directory: tmp.path, fn })
}

const texts = (r: { turns: Array<{ text: string }> }) => r.turns.map((t) => t.text)

const notFound = () => new Response(JSON.stringify({ error: "No chat with that id.", code: "not_found" }), { status: 404 })

/** The server's view of a plain chat (not a bot thread): it resolves to its own session. */
const plainChat = (id: string) => Response.json({ thread: { id, title: "Chat", status: "unknown", sessionId: id, local: true } })

describe("thread_read", () => {
  test("cursor paging covers every turn once, backwards and forwards", async () => {
    await inProject(async () => {
      const sid = await chat("Paging", 11)
      const all = Array.from({ length: 11 }, (_, i) => `turn ${i}`)

      // Latest page, then walk back.
      const back: string[][] = []
      let r = await PastChats.read(sid, { thread: "current", turns: 3 })
      expect(r.status).toBe("local")
      expect(r.next_cursor).toBeNull()
      back.push(texts(r))
      while (r.prev_cursor) {
        r = await PastChats.read(sid, { thread: "current", turns: 3, cursor: r.prev_cursor })
        back.push(texts(r))
      }
      expect(back.reverse().flat()).toEqual(all)

      // From the first page forward.
      const fwd: string[] = [...texts(r)]
      while (r.next_cursor) {
        r = await PastChats.read(sid, { thread: "current", turns: 3, cursor: r.next_cursor })
        fwd.push(...texts(r))
      }
      expect(fwd).toEqual(all)

      await expect(PastChats.read(sid, { thread: "current", cursor: "nonsense" })).rejects.toThrow("cursor")
    })
  })

  test("around opens at the first hit and the window is clamped to the thread's ends", async () => {
    await inProject(async () => {
      const words = (i: number) => (i === 0 ? "the venue deposit" : i === 6 ? "venue contract signed" : i === 9 ? "invoice venue" : `turn ${i}`)
      const sid = await chat("Around", 10, words)

      const mid = await PastChats.read(sid, { thread: "current", around: "contract", turns: 5 })
      expect(mid.around_found).toBe(true)
      expect(texts(mid)).toEqual(["turn 4", "turn 5", "venue contract signed", "turn 7", "turn 8"])
      expect(mid.prev_cursor).not.toBeNull()
      expect(mid.next_cursor).not.toBeNull()

      // First hit is turn 0: no lead-in before the start.
      const start = await PastChats.read(sid, { thread: "current", around: "VENUE", turns: 4 })
      expect(texts(start)[0]).toBe("the venue deposit")
      expect(start.prev_cursor).toBeNull()

      // Hit on the last turn: the window ends there and still holds `turns` turns.
      const end = await PastChats.read(sid, { thread: "current", around: "invoice", turns: 4 })
      expect(texts(end)).toEqual(["venue contract signed", "turn 7", "turn 8", "invoice venue"])
      expect(end.next_cursor).toBeNull()

      const miss = await PastChats.read(sid, { thread: "current", around: "zebra", turns: 2 })
      expect(miss.around_found).toBe(false)
      expect(texts(miss)).toEqual(["turn 8", "invoice venue"])
    })
  })

  test("an incognito chat is never read, even when its transcript is on this computer", async () => {
    await inProject(async () => {
      const caller = await chat("Caller", 2)
      const secret = await chat("Secret", 2, () => "the surprise party plan")
      route = (p) =>
        p === "/thread/resolve"
          ? new Response(JSON.stringify({ error: "No chat with that id.", code: "not_found" }), { status: 404 })
          : new Response("{}", { status: 500 })
      const r = await PastChats.read(caller, { thread: secret })
      expect(r.status).toBe("not_found")
      expect(r.turns).toEqual([])
      expect(calls.map((c) => c.path)).toEqual(["/thread/resolve"])

      const tool = await ThreadReadTool.init()
      const out = await tool.execute({ thread: secret }, { sessionID: caller, abort: AbortSignal.any([]) } as any)
      expect(out.title).toBe("Chat not found")
      expect(out.output).not.toContain("surprise")
    })
  })

  test("a transcript on another computer comes back as its summary and where it is, labelled as records", async () => {
    await inProject(async () => {
      const caller = await chat("Caller", 2)
      route = (p) =>
        p === "/thread/resolve"
          ? Response.json({
              thread: { id: "thr_1", title: "Venue hunt", status: "idle", sessionId: "ses_elsewhere", local: false, computer: "Studio Mac", summary: "Booked the hall." },
            })
          : new Response("{}", { status: 500 })
      const tool = await ThreadReadTool.init()
      const out = await tool.execute({ thread: "thr_1" }, { sessionID: caller, abort: AbortSignal.any([]) } as any)
      expect(out.metadata.status).toBe("remote")
      expect(out.output.startsWith(PastChats.RECORD_NOTICE)).toBe(true)
      const r = JSON.parse(out.output.slice(PastChats.RECORD_NOTICE.length))
      expect(r).toMatchObject({ status: "remote", summary: "Booked the hall.", turns: [] })
      expect(r.where).toContain("Studio Mac")
    })
  })
})

describe("thread_wait", () => {
  test("timeout 0 is a snapshot and cursors go back to the server; the first woken target is returned", async () => {
    route = (p, body) =>
      p === "/thread/wait"
        ? Response.json({
            woke: body.timeoutS === 0 ? null : { thread: "t2", title: "B", status: "needs_you", statusLine: "Which venue?", cursor: "9", reason: "needs_user" },
            timedOut: body.timeoutS === 0,
            statuses: [
              { thread: "t1", title: "A", status: "working", statusLine: null, cursor: "4" },
              { thread: "t2", title: "B", status: "needs_you", statusLine: "Which venue?", cursor: "9" },
            ],
          })
        : new Response("{}", { status: 500 })

    const snap = await PastChats.wait("ses_caller", { targets: [{ thread: "t1" }, { thread: "t2", after_cursor: "3" }], timeout_s: 0 })
    expect(snap).toMatchObject({ woke: null, timed_out: true })
    expect(snap.statuses.map((s) => s.cursor)).toEqual(["4", "9"])
    expect(calls[0].body).toMatchObject({ sessionId: "ses_caller", timeoutS: 0, targets: [{ thread: "t1" }, { thread: "t2", afterCursor: "3" }] })

    const woke = await PastChats.wait("ses_caller", { targets: [{ thread: "t1", after_cursor: "4" }, { thread: "t2", after_cursor: "3" }], timeout_s: 30 })
    expect(woke.woke).toMatchObject({ thread: "t2", reason: "needs_user", status_line: "Which venue?", cursor: "9" })
    expect(woke.timed_out).toBe(false)
  })
})

describe("thread_recent", () => {
  test("lists this computer's plain chats and CLI sessions with the bot threads, never an incognito one", async () => {
    await inProject(async () => {
      const caller = await chat("Caller", 2)
      const plain = await chat("Seating chart", 2)
      const secret = await chat("Secret", 2)
      route = (p, body) => {
        if (p === "/thread/recent")
          return Response.json({
            threads: [
              { id: "thr_bot", botId: "b1", botName: "Scout", title: "Venue hunt", status: "idle", closed: false, archived: false, lastActivityAt: new Date(Date.now() + 60_000).toISOString(), kind: "task" },
            ],
          })
        if (p === "/thread/resolve") return body.thread === secret ? notFound() : plainChat(body.thread)
        return new Response("{}", { status: 500 })
      }
      const r = await PastChats.recent(caller, { limit: 20 })
      const ids = r.threads.map((t) => t.id)
      expect(ids[0]).toBe("thr_bot")
      expect(ids).toContain(plain)
      expect(ids).not.toContain(secret)
      expect(r.threads.find((t) => t.id === plain)).toMatchObject({ title: "Seating chart", status: "idle", botName: null })
      const times = r.threads.map((t) => Date.parse(t.lastActivityAt))
      expect(times).toEqual([...times].sort((a, b) => b - a))
    })
  })
})

describe("thread_wait on a plain chat", () => {
  test("wakes when the chat finishes a turn, and its cursor stops the same turn waking it again", async () => {
    await inProject(async () => {
      const caller = await chat("Caller", 2)
      const worker = await chat("Worker", 2)
      route = (p, body) => (p === "/thread/resolve" ? plainChat(body.thread) : new Response("{}", { status: 500 }))

      const snap = await PastChats.wait(caller, { targets: [{ thread: worker }], timeout_s: 0 })
      expect(snap).toMatchObject({ woke: null, timed_out: true, statuses: [{ thread: worker, title: "Worker", status: "idle" }] })

      const waiting = PastChats.wait(caller, { targets: [{ thread: worker, after_cursor: snap.statuses[0].cursor }], timeout_s: 30 })
      SessionStatus.set(worker, { type: "busy" })
      await say(worker, "user", "book the hall")
      await say(worker, "assistant", "booked")
      SessionStatus.set(worker, { type: "idle" })
      const r = await waiting
      expect(r.woke).toMatchObject({ thread: worker, reason: "turn_finished", status: "idle" })
      expect(r.timed_out).toBe(false)

      const again = await PastChats.wait(caller, { targets: [{ thread: worker, after_cursor: r.woke!.cursor }], timeout_s: 0 })
      expect(again.woke).toBeNull()
    })
  })

  test("an incognito chat can't be waited on", async () => {
    await inProject(async () => {
      const caller = await chat("Caller", 2)
      const secret = await chat("Secret", 2)
      route = (p) => (p === "/thread/resolve" ? notFound() : new Response("{}", { status: 500 }))
      await expect(PastChats.wait(caller, { targets: [{ thread: secret }], timeout_s: 0 })).rejects.toThrow("No chat")
    })
  })
})
