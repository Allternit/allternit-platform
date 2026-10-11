import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { BackgroundTask } from "../../src/runtime/session/background-task"
import { SessionTasks } from "../../src/runtime/session/session-tasks"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

describe("session tasks", () => {
  test("parses subagent child titles", () => {
    expect(SessionTasks.parseChildTitle("Inspect files (@explore subagent)")).toEqual({
      description: "Inspect files",
      agent: "explore",
    })
    expect(SessionTasks.parseChildTitle("Plain title")).toEqual({ description: "Plain title", agent: null })
  })

  test("merges background tasks with foreground subagent children", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const foreground = await Session.create({ parentID: parent.id, title: "Review diff (@review subagent)" })
        const background = await Session.create({ parentID: parent.id, title: "Scan repo (@explore subagent)" })
        const subagentTask = Identifier.ascending("task")
        await BackgroundTask.create({
          id: subagentTask,
          parentSessionID: parent.id,
          childSessionID: background.id,
          kind: "subagent",
          description: "Scan repo",
        })
        const shellTask = Identifier.ascending("task")
        await BackgroundTask.create({ id: shellTask, parentSessionID: parent.id, kind: "shell", description: "Dev server" })

        const items = await SessionTasks.list(parent.id)
        expect(items).toHaveLength(3)
        expect(items.find((item) => item.id === subagentTask)).toMatchObject({
          background: true,
          kind: "subagent",
          agent: "explore",
          childSessionID: background.id,
          status: "running",
        })
        expect(items.find((item) => item.id === shellTask)).toMatchObject({ background: true, kind: "shell" })
        expect(items.find((item) => item.id === foreground.id)).toMatchObject({
          background: false,
          kind: "subagent",
          agent: "review",
          description: "Review diff",
          childSessionID: foreground.id,
          status: "completed",
        })
        await Session.remove(parent.id)
      },
    })
  })

  test("progress streams output and cancel runs the cancel hook", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const id = Identifier.ascending("task")
        await BackgroundTask.create({ id, parentSessionID: parent.id, kind: "shell", description: "watch" })
        let killed = false
        BackgroundTask.onCancel(id, () => {
          killed = true
        })

        expect(await BackgroundTask.progress(id, "line 1\n")).toMatchObject({ status: "running", output: "line 1\n" })
        const cancelled = await BackgroundTask.cancel(id)
        expect(killed).toBe(true)
        expect(cancelled).toMatchObject({ status: "cancelled", output: "line 1\n" })
        // Progress after settlement is ignored.
        expect(await BackgroundTask.progress(id, "late")).toBeUndefined()
        await Session.remove(parent.id)
      },
    })
  })

  test("fail keeps the output alongside the error", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const id = Identifier.ascending("task")
        await BackgroundTask.create({ id, parentSessionID: parent.id, kind: "shell", description: "build" })
        expect(await BackgroundTask.fail(id, "Exited with code 2", "$ make\nerror")).toMatchObject({
          status: "failed",
          error: "Exited with code 2",
          output: "$ make\nerror",
        })
        await Session.remove(parent.id)
      },
    })
  })
})
