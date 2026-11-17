import { afterEach, describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { BackgroundTask } from "../../src/runtime/session/background-task"
import { SessionTasks } from "../../src/runtime/session/session-tasks"
import { SessionTable } from "../../src/runtime/session/session.sql"
import { Database, eq } from "../../src/runtime/session/storage/db"
import { AgentControl } from "../../src/runtime/primitives/agent-control"
import { SteerSignal } from "../../src/runtime/primitives/steer-signal"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

// The model edge is the only thing stubbed: turns, steers, stops and the
// parent notification. Rows, sessions, the bus and the steer signal are real.
type Turn = { sessionID: string; text: string; resolve: (text: string) => void; reject: (error: Error) => void }
function fakeRunner() {
  const calls = { turns: [] as Turn[], steers: [] as string[], cancels: [] as string[], notified: [] as any[] }
  const runner: AgentControl.Runner = {
    turn: ({ sessionID, text }) =>
      new Promise<string>((resolve, reject) => calls.turns.push({ sessionID, text, resolve, reject })),
    steer: ({ sessionID }) => void calls.steers.push(sessionID),
    cancel: (sessionID) => void calls.cancels.push(sessionID),
    notifyParent: async ({ payload }) => void calls.notified.push(payload),
  }
  return { calls, restore: AgentControl.useRunner(runner) }
}

let restore: (() => void) | undefined
afterEach(() => {
  restore?.()
  restore = undefined
})

const abort = () => new AbortController().signal

async function child(parentID: string, title: string) {
  return Session.create({ parentID, title: `${title} (@explore subagent)` })
}

async function running(parentID: string, childID: string, description = "work") {
  const id = Identifier.ascending("task")
  await BackgroundTask.create({ id, parentSessionID: parentID, childSessionID: childID, kind: "subagent", description })
  return id
}

function name(sessionID: string, path: string) {
  Database.use((db) => db.update(SessionTable).set({ task_name: path }).where(eq(SessionTable.id, sessionID)).run())
}

function within(fn: () => Promise<void>) {
  return async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({ directory: tmp.path, fn })
  }
}

describe("wait_agents", () => {
  test(
    "returns the first of several targets to settle, with the rest still running",
    within(async () => {
      const parent = await Session.create({})
      const a = await child(parent.id, "A")
      const b = await child(parent.id, "B")
      const rowA = await running(parent.id, a.id)
      await running(parent.id, b.id)

      const waiting = AgentControl.waitAgents({ sessionID: parent.id, abort: abort(), targets: [a.id, b.id], timeout_ms: 5_000 })
      await Bun.sleep(20)
      await BackgroundTask.complete(rowA, 'done\n```json\n{"result": {"summary": "A found 3 files"}}\n```')

      const result = await waiting
      expect(result.woke).toBe("settled")
      expect(result.settled).toEqual([{ id: a.id, name: null, status: "completed", summary: "A found 3 files" }])
      expect(result.running.map((ref) => ref.id)).toEqual([b.id])
    }),
  )

  test(
    "wakes on a steer, but reports a target that settled first as settled",
    within(async () => {
      const parent = await Session.create({})
      const a = await child(parent.id, "A")
      const rowA = await running(parent.id, a.id)

      const steered = AgentControl.waitAgents({ sessionID: parent.id, abort: abort(), timeout_ms: 5_000 })
      while (SteerSignal.waiting(parent.id) === 0) await Bun.sleep(5)
      SteerSignal.notify(parent.id)
      expect(await steered).toEqual({ woke: "steer", settled: [], running: [{ id: a.id, name: null, path: null }] })

      // A background completion also bumps the steer signal; the settled child wins.
      const second = AgentControl.waitAgents({ sessionID: parent.id, abort: abort(), timeout_ms: 5_000 })
      while (SteerSignal.waiting(parent.id) === 0) await Bun.sleep(5)
      await BackgroundTask.complete(rowA, "finished")
      SteerSignal.notify(parent.id)
      const result = await second
      expect(result.woke).toBe("settled")
      expect(result.settled.map((agent) => agent.id)).toEqual([a.id])
    }),
  )

  test(
    "times out with the targets still running",
    within(async () => {
      const parent = await Session.create({})
      const a = await child(parent.id, "A")
      await running(parent.id, a.id)
      const started = Date.now()
      const result = await AgentControl.waitAgents({ sessionID: parent.id, abort: abort(), targets: [a.id], timeout_ms: 60 })
      expect(result.woke).toBe("timeout")
      expect(result.running.map((ref) => ref.id)).toEqual([a.id])
      expect(Date.now() - started).toBeGreaterThanOrEqual(50)
    }),
  )
})

describe("agent_control", () => {
  test(
    "close cascades deepest first and archives every closed agent",
    within(async () => {
      const fake = fakeRunner()
      restore = fake.restore
      const parent = await Session.create({})
      const a = await child(parent.id, "A")
      const a1 = await child(a.id, "A1")
      const a2 = await child(a.id, "A2")
      const a11 = await child(a1.id, "A11")
      const rowA1 = await running(a.id, a1.id)

      const result = await AgentControl.run({ sessionID: parent.id, abort: abort(), action: "close", target: a.id })
      if (result.action !== "close") throw new Error("expected close")
      const order = result.closed.map((ref) => ref.id)
      expect(order[0]).toBe(a11.id)
      expect(order.at(-1)).toBe(a.id)
      expect(order.indexOf(a1.id)).toBeGreaterThan(order.indexOf(a11.id))
      expect(new Set(order)).toEqual(new Set([a.id, a1.id, a2.id, a11.id]))
      expect((await BackgroundTask.get(rowA1))?.status).toBe("cancelled")
      for (const id of order) expect((await Session.get(id)).time.archived).toBeNumber()
      await expect(
        AgentControl.run({ sessionID: parent.id, abort: abort(), action: "resume", target: a.id }),
      ).rejects.toThrow("closed")
    }),
  )

  test(
    "interrupt keeps the child and settles as interrupted; resume opens a new run row on it",
    within(async () => {
      const fake = fakeRunner()
      restore = fake.restore
      const parent = await Session.create({})
      const a = await child(parent.id, "A")
      const first = await running(parent.id, a.id, "Scan repo")

      const interrupted = await AgentControl.run({ sessionID: parent.id, abort: abort(), action: "interrupt", target: a.id })
      expect(interrupted).toMatchObject({ action: "interrupt", outcome: "interrupted" })
      expect(fake.calls.cancels).toEqual([a.id])
      // The stopped run's own failure lands after the interrupt and does not turn it into "failed".
      expect(await BackgroundTask.fail(first, new Error("aborted"))).toMatchObject({ status: "interrupted" })
      expect((await Session.get(a.id)).id).toBe(a.id)

      const resumed = await AgentControl.run({ sessionID: parent.id, abort: abort(), action: "resume", target: a.id })
      if (resumed.action !== "resume" || !resumed.run_id) throw new Error("expected a resumed run")
      expect(resumed.outcome).toBe("resumed")
      expect(resumed.run_id).not.toBe(first)
      expect(await BackgroundTask.get(resumed.run_id)).toMatchObject({ status: "running", childSessionID: a.id, description: "Scan repo" })
      expect(fake.calls.turns.map((turn) => turn.sessionID)).toEqual([a.id])

      expect(await AgentControl.run({ sessionID: parent.id, abort: abort(), action: "resume", target: a.id })).toMatchObject({
        outcome: "already_running",
      })

      fake.calls.turns[0].resolve("resumed and finished")
      await BackgroundTask.wait(resumed.run_id, 2_000)
      expect(await BackgroundTask.get(resumed.run_id)).toMatchObject({ status: "completed", output: "resumed and finished" })
      while (fake.calls.notified.length === 0) await Bun.sleep(5)
      expect(fake.calls.notified[0]).toMatchObject({ background_task_id: resumed.run_id, status: "completed" })
    }),
  )

  test(
    "acts on the caller's own descendants only, by id or name path",
    within(async () => {
      const fake = fakeRunner()
      restore = fake.restore
      const parent = await Session.create({})
      const sibling = await Session.create({})
      const theirs = await child(sibling.id, "Theirs")
      const a = await child(parent.id, "A")
      const tests = await child(a.id, "Tests")
      name(a.id, "/root/build")
      name(tests.id, "/root/build/tests")
      await running(a.id, tests.id)

      for (const target of [theirs.id, sibling.id, parent.id, "/root/nope"]) {
        await expect(
          AgentControl.run({ sessionID: parent.id, abort: abort(), action: "interrupt", target }),
        ).rejects.toThrow("No agent")
      }
      const byPath = await AgentControl.run({
        sessionID: parent.id,
        abort: abort(),
        action: "interrupt",
        target: "/root/build/tests",
      })
      expect(byPath).toMatchObject({
        outcome: "interrupted",
        target: { id: tests.id, name: "tests", path: "/root/build/tests" },
      })

      const listed = await AgentControl.run({ sessionID: parent.id, abort: abort(), action: "list", path_prefix: "/root/build/" })
      if (listed.action !== "list") throw new Error("expected list")
      expect(listed.agents).toEqual([
        expect.objectContaining({ id: tests.id, name: "tests", depth: 2, parent_id: a.id, status: "interrupted" }),
      ])
    }),
  )
})

describe("session tasks", () => {
  test(
    "groups a child's run rows into one item: newest run wins, runs counted",
    within(async () => {
      const parent = await Session.create({})
      const a = await child(parent.id, "A")
      name(a.id, "/root/scan")
      const first = await running(parent.id, a.id, "Scan repo")
      await BackgroundTask.interrupt(first, "Interrupted by the parent agent")
      const second = await running(parent.id, a.id, "Scan repo")

      const items = await SessionTasks.list(parent.id)
      expect(items).toHaveLength(1)
      expect(items[0]).toMatchObject({
        id: second,
        status: "running",
        runs: 2,
        childSessionID: a.id,
        name: "scan",
        path: "/root/scan",
      })
      expect(() => SessionTasks.Item.array().parse(items)).not.toThrow()
    }),
  )
})
