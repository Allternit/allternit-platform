import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { ContextNotice } from "../../src/runtime/primitives/context-notice"
import { BackgroundTask } from "../../src/runtime/session/background-task"
import { BackgroundTaskTable } from "../../src/runtime/session/session.sql"
import { Database, eq } from "../../src/runtime/session/storage/db"
import { Todo } from "../../src/runtime/session/todo"
import { Session } from "../../src/runtime/session"
import { tmpdir } from "../fixture/fixture"

/** PR8c quiet check-in and PR8d todo nudge, through the real stores. */

let seq = 0
const nextID = (prefix: string) => `${prefix}_q${String(++seq).padStart(6, "0")}`
const noModel = { id: "m", providerID: "p", limit: { context: 0, output: 0 } } as any
const tokens = { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } }
const turn = (todowrite = false) =>
  ({
    info: { id: nextID("msg"), role: "assistant", tokens, finish: "stop" },
    parts: todowrite ? [{ type: "tool", tool: "todowrite" }] : [],
  }) as any
const turns = (n: number) => Array.from({ length: n }, () => turn())

describe("context notice: quiet background command", () => {
  test("fires after 10 quiet minutes, once per quiet period", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const sessionID = (await Session.create({ title: "context notice" })).id
        const id = nextID("task")
        await BackgroundTask.create({ id, parentSessionID: sessionID, kind: "shell", description: "pnpm build" })
        const at = () => ContextNotice.forTurn({ sessionID, model: noModel, messages: [] })
        const backdate = (ms: number) =>
          Database.use((db) =>
            db.update(BackgroundTaskTable).set({ time_updated: Date.now() - ms }).where(eq(BackgroundTaskTable.id, id)).run(),
          )

        expect(await at()).toBeUndefined()
        backdate(11 * 60_000)
        const notice = await at()
        expect(notice).toContain('Background command "pnpm build"')
        expect(notice).toContain("task_output")
        expect(await at()).toBeUndefined()

        // A restart keeps the stored record, so it stays quiet.
        ContextNotice.forget(sessionID)
        expect(await at()).toBeUndefined()

        // New output, then another quiet period: one more check-in.
        await BackgroundTask.progress(id, "tick")
        expect(await at()).toBeUndefined()
        backdate(12 * 60_000)
        expect(await at()).toContain('Background command "pnpm build"')
        expect(await at()).toBeUndefined()
      },
    })
  })
})

describe("context notice: todo nudge", () => {
  test("fires after 10 turns with open todos and no todowrite, once per window", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const sessionID = (await Session.create({ title: "context notice" })).id
        const at = (messages: any[]) => ContextNotice.forTurn({ sessionID, model: noModel, messages })

        // No open items: never nudges.
        expect(await at(turns(12))).toBeUndefined()

        await Todo.update({
          sessionID,
          todos: [{ id: "t1", content: "ship it", status: "in_progress", priority: "high" }],
        })
        const history = [...turns(9)]
        expect(await at(history)).toBeUndefined()
        history.push(turn())
        const notice = await at(history)
        expect(notice).toContain("todo list")
        expect(notice).toContain("mark done what is finished")
        expect(await at(history)).toBeUndefined()

        // Nine more turns: still inside the window. The tenth starts the next nudge.
        history.push(...turns(9))
        expect(await at(history)).toBeUndefined()
        history.push(turn())
        expect(await at(history)).toContain("todo list")

        // A todowrite resets the count.
        history.push(...turns(5), turn(true), ...turns(9))
        expect(await at(history)).toBeUndefined()
      },
    })
  })
})
