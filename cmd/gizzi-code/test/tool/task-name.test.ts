import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { Identifier } from "../../src/shared/id/id"
import { TaskEffort, TaskFork, TaskName } from "../../src/runtime/primitives/task-name"
import { tmpdir } from "../fixture/fixture"

const model = { providerID: "anthropic", modelID: "claude-sonnet" }

async function userTurn(sessionID: string, text: string) {
  const id = Identifier.ascending("message")
  await Session.updateMessage({
    id,
    sessionID,
    role: "user",
    agent: "build",
    model,
    time: { created: Date.now() },
  } as MessageV2.User)
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text } as MessageV2.TextPart)
  return id
}

async function reply(sessionID: string, parentID: string, text: string, done = true) {
  const id = Identifier.ascending("message")
  await Session.updateMessage({
    id,
    sessionID,
    role: "assistant",
    parentID,
    mode: "build",
    agent: "build",
    path: { cwd: "/", root: "/" },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    ...model,
    time: done ? { created: Date.now(), completed: Date.now() } : { created: Date.now() },
  } as MessageV2.Assistant)
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text } as MessageV2.TextPart)
  return id
}

const texts = (msgs: MessageV2.WithParts[]) =>
  msgs.map((m) =>
    m.parts
      .filter((p) => p.type === "text")
      .map((p) => (p as MessageV2.TextPart).text)
      .join(""),
  )

function inInstance(fn: () => Promise<void>) {
  return async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({ directory: tmp.path, fn })
  }
}

describe("task names", () => {
  test(
    "a name becomes a unique path under the parent; a taken name gets -2",
    inInstance(async () => {
      const root = await Session.create({})
      const a = await Session.create({ parentID: root.id })
      const b = await Session.create({ parentID: root.id })
      expect(await TaskName.assign({ parentSessionID: root.id, childSessionID: a.id, name: "tests" })).toBe("/root/tests")
      expect(await TaskName.assign({ parentSessionID: root.id, childSessionID: b.id, name: "tests" })).toBe("/root/tests-2")
      expect(await TaskName.pathOf(b.id)).toBe("/root/tests-2")

      const build = await Session.create({ parentID: root.id })
      await TaskName.assign({ parentSessionID: root.id, childSessionID: build.id, name: "build" })
      const grand = await Session.create({ parentID: build.id })
      expect(await TaskName.assign({ parentSessionID: build.id, childSessionID: grand.id, name: "tests" })).toBe(
        "/root/build/tests",
      )
    }),
  )

  test(
    "names go at most 3 levels deep and must match the pattern",
    inInstance(async () => {
      let parent = await Session.create({})
      for (const name of ["a", "b", "c"]) {
        const child = await Session.create({ parentID: parent.id })
        await TaskName.assign({ parentSessionID: parent.id, childSessionID: child.id, name })
        parent = child
      }
      expect(await TaskName.pathOf(parent.id)).toBe("/root/a/b/c")
      const tooDeep = await Session.create({ parentID: parent.id })
      await expect(TaskName.assign({ parentSessionID: parent.id, childSessionID: tooDeep.id, name: "d" })).rejects.toThrow(
        /at most 3 levels/,
      )
      expect(() => TaskName.validate("Bad/Name")).toThrow(/Invalid task name/)
      expect(() => TaskName.validate("x".repeat(33))).toThrow(/Invalid task name/)
    }),
  )

  test(
    "resolve finds a descendant by absolute path, relative path or id, and nothing outside the subtree",
    inInstance(async () => {
      const root = await Session.create({})
      const build = await Session.create({ parentID: root.id })
      await TaskName.assign({ parentSessionID: root.id, childSessionID: build.id, name: "build" })
      const tests = await Session.create({ parentID: build.id })
      await TaskName.assign({ parentSessionID: build.id, childSessionID: tests.id, name: "tests" })
      const other = await Session.create({ parentID: root.id })
      await TaskName.assign({ parentSessionID: root.id, childSessionID: other.id, name: "docs" })

      expect(await TaskName.resolve({ sessionID: root.id, target: "/root/build/tests" })).toBe(tests.id)
      expect(await TaskName.resolve({ sessionID: root.id, target: "build/tests" })).toBe(tests.id)
      expect(await TaskName.resolve({ sessionID: build.id, target: "tests" })).toBe(tests.id)
      expect(await TaskName.resolve({ sessionID: root.id, target: tests.id })).toBe(tests.id)
      // build's subtree does not hold docs, by path or by id.
      expect(await TaskName.resolve({ sessionID: build.id, target: "/root/docs" })).toBeUndefined()
      expect(await TaskName.resolve({ sessionID: build.id, target: other.id })).toBeUndefined()
      expect(await TaskName.resolve({ sessionID: root.id, target: "/root/missing" })).toBeUndefined()
    }),
  )
})

describe("task fork", () => {
  test(
    "copyHistory stops before the unfinished assistant message that is calling the tool",
    inInstance(async () => {
      const parent = await Session.create({})
      const u1 = await userTurn(parent.id, "first question")
      await reply(parent.id, u1, "first answer")
      const u2 = await userTurn(parent.id, "now run the tests")
      await reply(parent.id, u2, "calling task…", false)
      const child = await Session.create({ parentID: parent.id })

      expect(await TaskFork.copyHistory({ parentSessionID: parent.id, childSessionID: child.id })).toBe(3)
      const copied = await Session.messages({ sessionID: child.id })
      expect(texts(copied)).toEqual(["first question", "first answer", "now run the tests"])
      expect(copied.every((m) => m.info.sessionID === child.id)).toBe(true)
      const assistant = copied[1].info as MessageV2.Assistant
      expect(assistant.parentID).toBe(copied[0].info.id)
    }),
  )

  test("fork all refuses a model or effort override and allows none", () => {
    expect(() => TaskFork.check({ fork: "all", model: "deep" })).toThrow(/cannot take model: deep/)
    expect(() => TaskFork.check({ fork: "all", effort: "high" })).toThrow(/effort: high/)
    expect(() => TaskFork.check({ fork: "all", model: "inherit" })).not.toThrow()
    expect(() => TaskFork.check({ fork: "all" })).not.toThrow()
    expect(() => TaskFork.check({ fork: "none", model: "deep", effort: "high" })).not.toThrow()
  })
})

describe("task effort", () => {
  test("maps to the nearest level the model has; ties go lower; no levels means no variant", () => {
    expect(TaskEffort.nearest("high", ["low", "medium", "high", "xhigh", "max"])).toBe("high")
    expect(TaskEffort.nearest("medium", ["low", "high"])).toBe("low")
    expect(TaskEffort.nearest("low", ["high", "max"])).toBe("high")
    expect(TaskEffort.nearest("medium", ["minimal", "low", "medium", "high"])).toBe("medium")
    expect(TaskEffort.nearest("high", [])).toBeUndefined()
    expect(TaskEffort.nearest("high", ["thinking-8k"])).toBeUndefined()
  })
})
