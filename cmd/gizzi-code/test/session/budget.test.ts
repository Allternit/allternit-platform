import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { Budget } from "../../src/runtime/session/budget"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

async function spend(sessionID: string, cost: number) {
  await Session.updateMessage({
    id: Identifier.ascending("message"),
    sessionID,
    role: "assistant",
    parentID: Identifier.ascending("message"),
    mode: "build",
    agent: "build",
    path: { cwd: "/", root: "/" },
    cost,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    modelID: "m",
    providerID: "p",
    time: { created: Date.now() },
  } as MessageV2.Assistant)
}

describe("spend limits", () => {
  test("a bot's monthly budget and a thread budget hold the next turn", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const a = await Session.create({ agentID: "ledger" })
        const b = await Session.create({ agentID: "ledger" })
        const other = await Session.create({ agentID: "scout" })
        await spend(a.id, 1.5)
        await spend(b.id, 2)
        await spend(other.id, 9)
        expect(Budget.agentSpend("ledger", Budget.monthStart())).toBeCloseTo(3.5)

        expect(await Budget.exceeded(a)).toBeUndefined()
        Budget.set("agent", "ledger", 3)
        const over = await Budget.exceeded(a)
        expect(over?.limit).toBe("monthly budget ($3.00)")
        expect(over?.until).toBe(Budget.nextMonthStart())
        Budget.set("agent", "ledger", 10)
        expect(await Budget.exceeded(a)).toBeUndefined()

        Budget.set("session", b.id, 1)
        expect((await Budget.exceeded(b))?.limit).toBe("thread budget ($1.00)")
        Budget.set("session", b.id, null)
        expect(await Budget.exceeded(b)).toBeUndefined()
        for (const s of [a, b, other]) await Session.remove(s.id)
      },
    })
  })
})
