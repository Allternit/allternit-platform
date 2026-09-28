import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import { join } from "node:path"
import { tmpdir } from "../../fixture/fixture"

// Direct store reads used by the /bots pane to follow a context handoff
// (spec P3.16) without the runtime bootstrap.
let tmp: Awaited<ReturnType<typeof tmpdir>>
const run = `ses-handoff-head-${Date.now()}`

beforeAll(async () => {
  tmp = await tmpdir()
  process.env.XDG_DATA_HOME = join(tmp.path, "xdg-data")
  process.env.XDG_CACHE_HOME = join(tmp.path, "xdg-cache")
  process.env.XDG_CONFIG_HOME = join(tmp.path, "xdg-config")
  process.env.XDG_STATE_HOME = join(tmp.path, "xdg-state")
})

afterAll(async () => {
  try {
    const { Database, sql } = await import("../../../src/runtime/session/storage/db")
    Database.use((db) => {
      db.run(sql`DELETE FROM part WHERE session_id LIKE ${run + "-%"}`)
      db.run(sql`DELETE FROM message WHERE session_id LIKE ${run + "-%"}`)
      db.run(sql`DELETE FROM session WHERE id LIKE ${run + "-%"}`)
      db.run(sql`DELETE FROM project WHERE id LIKE ${"proj-" + run + "%"}`)
    })
  } catch {
    // store never opened
  }
})

let seeded: ReturnType<typeof seedStore> | undefined
// Both tests read the same rows; seed them once.
const seed = () => (seeded ??= seedStore())

async function seedStore() {
  // Creates the sandboxed data dir, as the real store reads do.
  await (await import("../../../src/runtime/bots/session-db")).sessionExistsInStore("warmup")
  const { Database } = await import("../../../src/runtime/session/storage/db")
  const { SessionTable, MessageTable, PartTable } = await import("../../../src/runtime/session/session.sql")
  const { ProjectTable } = await import("../../../src/runtime/context/project/project.sql")
  const [a, b, c] = [`${run}-a`, `${run}-b`, `${run}-c`]
  Database.use((db) => {
    db.insert(ProjectTable).values({ id: `proj-${run}`, worktree: "/tmp/x", sandboxes: [] }).onConflictDoNothing().run()
    const session = (id: string, handoff?: string) =>
      db
        .insert(SessionTable)
        .values({
          id,
          project_id: `proj-${run}`,
          slug: "s",
          directory: "/tmp/x",
          title: "bot",
          version: "1",
          ...(handoff ? { handoff: { sessionID: handoff, reason: "threshold", at: 1 } } : {}),
        })
        .run()
    session(a, b)
    session(b, c)
    session(c)
    db.insert(MessageTable).values({ id: `${c}-m1`, session_id: c, data: { role: "user" } as never }).run()
    // The earlier window b: a question, an answer with a tool call, an empty reply.
    const msg = (id: string, role: string, at: number, modelID?: string) =>
      db.insert(MessageTable).values({ id: `${b}-${id}`, session_id: b, time_created: at, data: { role, modelID } as never }).run()
    const part = (id: string, message: string, data: object) =>
      db.insert(PartTable).values({ id: `${b}-${id}`, message_id: `${b}-${message}`, session_id: b, data: data as never }).run()
    msg("m1", "user", 1000)
    msg("m2", "assistant", 2000, "glm-4.7-flash")
    msg("m3", "assistant", 3000)
    part("p1", "m1", { type: "text", text: "When do we ship?" })
    part("p2", "m2", { type: "text", text: "Friday." })
    part("p3", "m2", { type: "tool", tool: "read" })
    part("p4", "m3", { type: "reasoning", text: "thinking only" })
    db.insert(PartTable)
      .values({ id: `${c}-p0`, message_id: `${c}-m1`, session_id: c, data: { type: "text", text: "just text" } as never })
      .run()
    db.insert(PartTable)
      .values({
        id: `${c}-p1`,
        message_id: `${c}-m1`,
        session_id: c,
        data: {
          type: "text",
          text: "[checkpoint: window 2] Decided: ship Friday.",
          metadata: { handoff: { from: b, generation: 2, reason: "model_switch" } },
          time: { start: 1790000000000 },
        } as never,
      })
      .run()
  })
  return { a, b, c }
}

describe("session handoff head", () => {
  test("follows the lineage to the newest window and reads its seed", async () => {
    const { sessionHandoffHead, sessionHandoffSeed } = await import("../../../src/runtime/bots/session-db")
    const { a, b, c } = await seed()
    expect(await sessionHandoffHead(a)).toBe(c)
    expect(await sessionHandoffHead(b)).toBe(c)
    expect(await sessionHandoffHead(c)).toBe(c)
    expect(await sessionHandoffSeed(c)).toEqual({
      text: "[checkpoint: window 2] Decided: ship Friday.",
      from: b,
      generation: 2,
      reason: "model_switch",
      at: 1790000000000,
    })
    expect(await sessionHandoffSeed(a)).toBeNull()

    const { resolveHandoffHead } = await import("../../../src/cli/ui/ink-app/screens/bots-pane/handoff-head")
    expect(await resolveHandoffHead(c)).toBeNull()
    expect(await resolveHandoffHead(a)).toMatchObject({ sessionId: c, from: b, generation: 3, reason: "model_switch" })
    // First store open runs migrations; slow on a busy machine.
  }, 30_000)

  test("reads the earlier window as text, newest capped", async () => {
    const { sessionTextMessages } = await import("../../../src/runtime/bots/session-db")
    const { b, c } = await seed()
    expect(await sessionTextMessages(b)).toEqual({
      total: 2,
      messages: [
        { id: `${b}-m1`, role: "user", content: "When do we ship?", at: 1000 },
        { id: `${b}-m2`, role: "assistant", content: "Friday.\n[Tool read]", at: 2000, model: "glm-4.7-flash" },
      ],
    })
    expect((await sessionTextMessages(b, 1)).messages.map((m) => m.id)).toEqual([`${b}-m2`])
    // A window's seed carries its handoff.
    const [first] = (await sessionTextMessages(c)).messages
    expect(first?.handoff).toEqual({ from: b, generation: 2, reason: "model_switch" })

    const { loadEarlierWindow } = await import("../../../src/cli/ui/ink-app/screens/bots-pane/handoff-head")
    expect((await loadEarlierWindow({ sessionId: c, from: b, checkpoint: "" }))?.total).toBe(2)
    expect(await loadEarlierWindow({ sessionId: c, checkpoint: "" })).toBeNull()
  })
})
