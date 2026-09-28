import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionHandoff } from "../../src/runtime/session/handoff"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { Todo } from "../../src/runtime/session/todo"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

async function userTurn(sessionID: string, text: string) {
  const id = Identifier.ascending("message")
  await Session.updateMessage({
    id,
    sessionID,
    role: "user",
    agent: "build",
    model: { providerID: "test", modelID: "test-model" },
    time: { created: Date.now() },
  })
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text })
}

const tokens = (input: number) => ({ input, output: 0, reasoning: 0, cache: { read: 0, write: 0 } })
const model = (context: number) =>
  ({ id: "m", providerID: "p", limit: { context, output: 8_000 } }) as any

describe("session context handoff", () => {
  test("hands off to a linked, seeded session and redirects to the head", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const first = await Session.create({ title: "Pricing model", surface: "code", agentID: "scout" })
        await userTurn(first.id, "Build the H100 pricing sheet")
        Todo.update({
          sessionID: first.id,
          todos: [{ content: "Annual discount", status: "pending", priority: "high" }],
        })

        const { session: second, baton } = await SessionHandoff.run({
          sessionID: first.id,
          reason: "threshold",
          baton: { summary: "H100 all-in is $1.94/hr", decisions: ["35% margin"] },
        })
        expect(second.continuesFrom).toBe(first.id)
        expect(second).toMatchObject({ title: "Pricing model", surface: "code", agentID: "scout" })
        expect(baton).toMatchObject({ generation: 1, reason: "threshold", written: "caller", todos: ["[pending] Annual discount"] })

        const closed = await Session.get(first.id)
        expect(closed.handoff).toMatchObject({ sessionID: second.id, reason: "threshold" })

        // The fresh window starts from the baton, and keeps the TODO list.
        const seeded = await MessageV2.filterCompacted(MessageV2.stream(second.id))
        const text = seeded[0].parts.find((p): p is MessageV2.TextPart => p.type === "text")!
        expect(text.synthetic).toBe(true)
        expect(text.text).toContain("H100 all-in is $1.94/hr")
        expect(text.text).toContain("- 35% margin")
        expect(text.metadata?.handoff).toMatchObject({ from: first.id, generation: 1 })
        expect(Todo.get(second.id).map((t) => t.content)).toEqual(["Annual discount"])

        // Idempotent: handing off a closed window returns the head.
        const again = await SessionHandoff.run({ sessionID: first.id, reason: "manual" })
        expect(again.session.id).toBe(second.id)

        const { session: third } = await SessionHandoff.run({
          sessionID: second.id,
          reason: "model_switch",
          baton: { summary: "Sheet done" },
        })
        expect(await SessionHandoff.head(first.id)).toBe(third.id)
        expect((await SessionHandoff.lineage(second.id)).map((s) => s.id)).toEqual([first.id, second.id, third.id])

        // Conversation lists show the head only.
        const roots = [...Session.list({ roots: true })].map((s) => s.id)
        expect(roots).toContain(third.id)
        expect(roots).not.toContain(first.id)
        expect(roots).not.toContain(second.id)

        for (const s of [first, second, third]) await Session.remove(s.id)
      },
    })
  })

  test("concurrent handoffs share one run; subagents never hand off", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        await userTurn(parent.id, "hello")
        const [a, b] = await Promise.all([
          SessionHandoff.run({ sessionID: parent.id, reason: "threshold", baton: { summary: "one" } }),
          SessionHandoff.run({ sessionID: parent.id, reason: "threshold", baton: { summary: "two" } }),
        ])
        expect(a.session.id).toBe(b.session.id)

        const child = await Session.create({ parentID: a.session.id })
        await expect(SessionHandoff.run({ sessionID: child.id, reason: "threshold" })).rejects.toThrow("subagent")
        await Session.remove(parent.id)
        await Session.remove(a.session.id)
      },
    })
  })

  test("threshold, window math and baton parsing", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const window = model(200_000)
        const budget = SessionHandoff.usable(window)
        expect(budget).toBe(192_000)
        expect(await SessionHandoff.shouldHandoff({ tokens: tokens(budget * 0.69), model: window })).toBe(false)
        expect(await SessionHandoff.shouldHandoff({ tokens: tokens(budget * 0.7), model: window })).toBe(true)
        expect(await SessionHandoff.shouldHandoff({ tokens: tokens(10), model: model(0) })).toBe(false)
      },
    })

    expect(
      SessionHandoff.parse('<think>x</think>```json\n{"summary":"Done","decisions":["a", 3],"nextSteps":["ship"]}\n```'),
    ).toEqual({ summary: "Done", decisions: ["a"], openItems: [], artifacts: [], nextSteps: ["ship"] })
    expect(SessionHandoff.parse('{"summary":"  "}')).toBeUndefined()
    expect(SessionHandoff.parse("no json")).toBeUndefined()
  })
})
