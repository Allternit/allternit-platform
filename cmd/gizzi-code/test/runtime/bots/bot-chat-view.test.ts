import { describe, expect, test } from "bun:test"
import { BotChatTracker, itemsFromMessages, parseSSE, type SyncEvent } from "../../../src/runtime/bots/bot-chat-view"

const text = (id: string, body: string, done = true) => ({
  id,
  sessionID: "s1",
  messageID: "m1",
  type: "text",
  text: body,
  time: done ? { start: 1, end: 2 } : { start: 1 },
})

describe("itemsFromMessages", () => {
  test("history becomes user, assistant, tool and rip items", () => {
    const items = itemsFromMessages([
      { id: "seed", role: "user", content: "checkpoint", metadata: { handoff: { from: "s0", generation: 2, reason: "budget" } } },
      { id: "u1", role: "user", content: "hi" },
      {
        id: "a1",
        role: "assistant",
        content: "",
        metadata: {
          parts: [
            text("p1", "hello"),
            { id: "p2", type: "tool", tool: "bash", state: { status: "completed", input: { command: "ls -la\nmore" } } },
            { id: "p3", type: "tool", tool: "read", state: { status: "running" } },
            { id: "p4", type: "text", text: "hidden", synthetic: true },
          ],
        } as any,
      },
    ])
    expect(items).toEqual([
      { kind: "rip", key: "msg:seed", generation: 2, reason: "budget", from: "s0" },
      { kind: "user", key: "msg:u1", text: "hi" },
      { kind: "assistant", key: "p1", text: "hello" },
      { kind: "tool", key: "p2", tool: "bash", title: "ls -la" },
    ])
  })
})

describe("BotChatTracker", () => {
  test("streams deltas as a preview and commits the finished part once", () => {
    const t = new BotChatTracker("s1")
    expect(t.handle({ type: "part_updated", session_id: "s1", part: text("p1", "", false) }).streamingText).toBeNull()
    expect(t.handle({ type: "part_delta", session_id: "s1", part_id: "p1", field: "text", delta: "Hel" }).streamingText).toBe("Hel")
    expect(t.handle({ type: "part_delta", session_id: "s1", part_id: "p1", field: "text", delta: "lo" }).streamingText).toBe("Hello")
    const done = t.handle({ type: "part_updated", session_id: "s1", part: text("p1", "Hello") })
    expect(done.commit).toEqual([{ kind: "assistant", key: "p1", text: "Hello" }])
    expect(done.streamingText).toBeNull()
    // message.updated fires again on every change: no second commit.
    expect(t.handle({ type: "part_updated", session_id: "s1", part: text("p1", "Hello") }).commit).toEqual([])
  })

  test("ignores other sessions and parts already shown", () => {
    const t = new BotChatTracker("s1")
    t.markShown([{ kind: "assistant", key: "p1", text: "x" }])
    expect(t.handle({ type: "part_updated", session_id: "s2", part: text("p9", "no") }).commit).toEqual([])
    expect(t.handle({ type: "part_updated", session_id: "s1", part: text("p1", "x") }).commit).toEqual([])
  })

  test("a user turn typed elsewhere commits; this terminal's own echo does not", () => {
    const t = new BotChatTracker("s1")
    t.expectEcho("mine")
    expect(t.handle({ type: "message_added", session_id: "s1", id: "u1", role: "user", content: "mine" }).commit).toEqual([])
    expect(t.handle({ type: "message_added", session_id: "s1", id: "u2", role: "user", content: "from desktop" }).commit).toEqual([
      { kind: "user", key: "msg:u2", text: "from desktop" },
    ])
    expect(t.handle({ type: "message_added", session_id: "s1", id: "a1", role: "assistant", content: "x" }).commit).toEqual([])
  })

  test("a handoff follows the new window and draws one rip", () => {
    const t = new BotChatTracker("s1")
    t.handle({ type: "part_updated", session_id: "s1", part: text("p1", "partial", false) })
    const update = t.handle({ type: "handed_off", session_id: "s1", to: "s2", generation: 2, reason: "budget" })
    expect(update.handedOffTo).toEqual({ sessionId: "s2", generation: 2, reason: "budget" })
    expect(update.streamingText).toBeNull()
    expect(update.commit).toEqual([{ kind: "rip", key: "handoff:s2", generation: 2, reason: "budget", from: "s1" }])
    expect(t.sessionId).toBe("s2")
    // The new window's seed would draw a second rip; it is skipped.
    const seed = { type: "message_added", session_id: "s2", id: "seed", role: "user", content: "ckpt", metadata: { handoff: { from: "s1" } } }
    expect(t.handle(seed).commit).toEqual([])
    // The new window streams normally.
    expect(t.handle({ type: "part_updated", session_id: "s2", part: { ...text("q1", "next"), sessionID: "s2" } }).commit).toHaveLength(1)
  })

  test("prompts pass through and resolve on any surface", () => {
    const t = new BotChatTracker("s1")
    const ask: SyncEvent = { type: "permission_asked", session_id: "s1", request_id: "per_1", permission: "bash" }
    expect(t.handle(ask).permission).toBe(ask)
    expect(t.handle({ type: "question_asked", session_id: "s1", request_id: "que_1" }).question?.request_id).toBe("que_1")
    expect(t.handle({ type: "permission_replied", session_id: "s1", request_id: "per_1" }).resolved).toBe("per_1")
    expect(t.handle({ type: "question_rejected", session_id: "s1", request_id: "que_1" }).resolved).toBe("que_1")
  })

  test("commitItems skips what the feed already committed", () => {
    const t = new BotChatTracker("s1")
    t.handle({ type: "part_updated", session_id: "s1", part: text("p1", "done") })
    expect(
      t.commitItems([
        { kind: "assistant", key: "p1", text: "done" },
        { kind: "assistant", key: "p2", text: "missed" },
      ]),
    ).toEqual([{ kind: "assistant", key: "p2", text: "missed" }])
  })
})

describe("parseSSE", () => {
  test("reads ids and JSON data across chunk boundaries, skipping comments", async () => {
    const enc = new TextEncoder()
    const chunks = [': connected\n\nid: 7\ndata: {"type":"a"', '}\n\ndata: not json\n\nid: 8\r\ndata: {"type":"b"}\r\n\r\n']
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        for (const chunk of chunks) c.enqueue(enc.encode(chunk))
        c.close()
      },
    })
    const out: Array<{ id?: string; type: string }> = []
    for await (const { id, data } of parseSSE(stream)) out.push({ id, type: data.type })
    expect(out).toEqual([
      { id: "7", type: "a" },
      { id: "8", type: "b" },
    ])
  })
})
