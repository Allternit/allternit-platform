import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { BackgroundTask } from "../../src/runtime/session/background-task"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { SessionTasks } from "../../src/runtime/session/session-tasks"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

async function reply(
  sessionID: string,
  input: { cost?: number; input?: number; output?: number; reasoning?: number; cacheRead?: number; model?: string; error?: MessageV2.Assistant["error"] },
) {
  const [providerID, modelID] = (input.model ?? "anthropic/claude-sonnet").split("/")
  await Session.updateMessage({
    id: Identifier.ascending("message"),
    sessionID,
    role: "assistant",
    parentID: Identifier.ascending("message"),
    mode: "build",
    agent: "build",
    path: { cwd: "/", root: "/" },
    cost: input.cost ?? 0,
    tokens: {
      input: input.input ?? 0,
      output: input.output ?? 0,
      reasoning: input.reasoning ?? 0,
      cache: { read: input.cacheRead ?? 0, write: 0 },
    },
    modelID,
    providerID,
    error: input.error,
    time: { created: Date.now() },
  } as MessageV2.Assistant)
}

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

  test("sums model, cost and tokens from a child session", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const child = await Session.create({ parentID: parent.id, title: "Read docs (@explore subagent)" })
        await reply(child.id, { cost: 0.25, input: 100, output: 40, reasoning: 10, cacheRead: 500, model: "openai/gpt-x" })
        await reply(child.id, { cost: 0.5, input: 200, output: 60, model: "anthropic/claude-sonnet" })
        const shell = Identifier.ascending("task")
        await BackgroundTask.create({ id: shell, parentSessionID: parent.id, kind: "shell", description: "tail" })

        const items = await SessionTasks.list(parent.id)
        const sub = items.find((item) => item.id === child.id)!
        expect(sub.model).toBe("anthropic/claude-sonnet")
        expect(sub.cost).toBeCloseTo(0.75)
        expect(sub.tokens).toEqual({ input: 300, output: 110 })
        expect(sub.status).toBe("completed")
        expect(sub.children).toEqual([])
        expect(items.find((item) => item.id === shell)).toMatchObject({ model: null, cost: null, tokens: null, children: [] })
        expect(() => SessionTasks.Item.array().parse(items)).not.toThrow()
        await Session.remove(parent.id)
      },
    })
  })

  test("nests child tasks with a depth limit", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const root = await Session.create({})
        const one = await Session.create({ parentID: root.id, title: "Plan (@plan subagent)" })
        const two = await Session.create({ parentID: one.id, title: "Search (@explore subagent)" })
        const three = await Session.create({ parentID: two.id, title: "Grep (@explore subagent)" })
        await Session.create({ parentID: three.id, title: "Too deep (@explore subagent)" })
        // A background subagent row also carries its child's nested tasks.
        const bgChild = await Session.create({ parentID: root.id, title: "Scan (@explore subagent)" })
        const bgGrandchild = await Session.create({ parentID: bgChild.id, title: "Look (@explore subagent)" })
        const bg = Identifier.ascending("task")
        await BackgroundTask.create({ id: bg, parentSessionID: root.id, childSessionID: bgChild.id, kind: "subagent", description: "Scan" })

        const items = await SessionTasks.list(root.id)
        const l1 = items.find((item) => item.id === one.id)!
        expect(l1.children.map((item) => item.id)).toEqual([two.id])
        const l2 = l1.children[0]
        expect(l2.children.map((item) => item.id)).toEqual([three.id])
        expect(l2.children[0].children).toEqual([])
        const bgItem = items.find((item) => item.id === bg)!
        expect(bgItem.status).toBe("running")
        expect(bgItem.children.map((item) => item.id)).toEqual([bgGrandchild.id])
        await Session.remove(root.id)
      },
    })
  })

  test("a foreground child whose last reply errored reads as failed", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const failed = await Session.create({ parentID: parent.id, title: "Fix build (@build subagent)" })
        await reply(failed.id, { cost: 0.1 })
        await reply(failed.id, {
          cost: 0.2,
          error: { name: "APIError", message: "APIError", data: { message: "Rate limit reached", isRetryable: false } },
        })
        const recovered = await Session.create({ parentID: parent.id, title: "Retry (@build subagent)" })
        await reply(recovered.id, { error: { name: "APIError", message: "boom", data: { message: "boom" } } })
        await reply(recovered.id, {})

        const items = await SessionTasks.list(parent.id)
        expect(items.find((item) => item.id === failed.id)).toMatchObject({
          status: "failed",
          error: "Rate limit reached",
        })
        expect(items.find((item) => item.id === recovered.id)).toMatchObject({ status: "completed", error: null })
        await Session.remove(parent.id)
      },
    })
  })
})
