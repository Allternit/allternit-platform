/**
 * A budget check that throws must not mean unlimited spend: the pre-turn gate
 * (SessionPrompt.holdForLimit → Budget.gate) pauses bot sessions and sessions
 * with a thread budget ("budget-check-failed"); a plain interactive session
 * without a budget warns and continues.
 */
import { afterEach, describe, expect, spyOn, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { Budget } from "../../src/runtime/session/budget"
import { SessionPause } from "../../src/runtime/session/pause"
import { SessionPrompt } from "../../src/runtime/session/prompt"
import { tmpdir } from "../fixture/fixture"

function throwingBudget() {
  return spyOn(Budget, "exceeded").mockImplementation(async () => {
    throw new Error("database is locked")
  })
}

afterEach(() => SessionPause.clearTimers())

describe("budget check failure", () => {
  test("pauses a bot session instead of running unmetered", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const bot = await Session.create({ agentID: "ledger" })
        const spy = throwingBudget()
        try {
          const before = Date.now()
          expect(await SessionPrompt.holdForLimit(bot, undefined)).toBe(true)
          expect(spy).toHaveBeenCalled()
          const paused = (await Session.get(bot.id)).paused
          expect(paused?.reason).toBe("budget-check-failed")
          expect(paused?.limit).toBe("budget check failed")
          expect(paused!.until).toBeGreaterThanOrEqual(before + Budget.CHECK_RETRY_MS)
          // Held turns stay held while paused.
          expect(SessionPause.isPaused(await Session.get(bot.id))).toBe(true)
        } finally {
          spy.mockRestore()
          await Session.remove(bot.id)
        }
      },
    })
  })

  test("pauses a session that carries a thread budget", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const s = await Session.create({})
        Budget.set("session", s.id, 5)
        const spy = throwingBudget()
        try {
          expect(await SessionPrompt.holdForLimit(s, undefined)).toBe(true)
          expect((await Session.get(s.id)).paused?.reason).toBe("budget-check-failed")
        } finally {
          spy.mockRestore()
          Budget.set("session", s.id, null)
          await Session.remove(s.id)
        }
      },
    })
  })

  test("lets a plain interactive session without a budget continue", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const s = await Session.create({})
        const spy = throwingBudget()
        try {
          expect(await SessionPrompt.holdForLimit(s, undefined)).toBe(false)
          expect(spy).toHaveBeenCalled()
          expect((await Session.get(s.id)).paused).toBeUndefined()
        } finally {
          spy.mockRestore()
          await Session.remove(s.id)
        }
      },
    })
  })

  test("an over-budget result still pauses with reason budget", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const bot = await Session.create({ agentID: "scout" })
        const until = Date.now() + 3_600_000
        const spy = spyOn(Budget, "exceeded").mockImplementation(async () => ({ until, limit: "monthly budget ($3.00)" }))
        try {
          expect(await Budget.gate(bot)).toEqual({ until, limit: "monthly budget ($3.00)", reason: "budget" })
        } finally {
          spy.mockRestore()
          await Session.remove(bot.id)
        }
      },
    })
  })
})
