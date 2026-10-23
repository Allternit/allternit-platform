import { describe, expect, test } from "bun:test"
import { tmpdir } from "../fixture/fixture"
import { Instance } from "../../src/runtime/context/project/instance"
import { ReviewStore } from "../../src/runtime/review/store"
import { ReviewLoop } from "../../src/runtime/review/loop"
import { GoalRoundChecks } from "../../src/runtime/review/goal-checks"
import type { ReviewVerdict } from "../../src/runtime/review/verdict"

function record(over: Partial<ReviewVerdict.Stored> = {}): ReviewVerdict.Stored {
  return {
    id: `rev_${Math.random().toString(36).slice(2)}`,
    sessionID: "ses_test",
    time: { started: 1, finished: 2 },
    source: "git",
    files: [{ path: "a.ts", status: "modified" }],
    verdict: "PASS",
    verified: ["ok"],
    findings: [],
    unverified: [],
    summary: "ok",
    ...over,
  }
}

const fail = (problem = "broken") =>
  record({ verdict: "FAIL", findings: [{ file: "a.ts", line: 3, problem }], verified: [] })

describe("review verdict storage", () => {
  test("keeps the last 8 per session, newest first", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const sessionID = `ses_${Math.random().toString(36).slice(2)}`
        const ids: string[] = []
        for (let i = 0; i < 10; i++) {
          const r = record({ sessionID, id: `rev_${i}` })
          ids.push(r.id)
          await ReviewStore.add(r)
        }
        const list = await ReviewStore.list(sessionID)
        expect(list.length).toBe(ReviewStore.KEEP)
        expect(list.map((r) => r.id)).toEqual(ids.slice(2).reverse())
        expect(await ReviewStore.list("ses_other_" + sessionID)).toEqual([])
      },
    })
  })
})

describe("reviewUntilPass", () => {
  const publish = (() => {}) as any

  test("stops on PASS and passes the previous verdict as a claim", async () => {
    const verdicts = [fail(), record()]
    const seen: unknown[] = []
    const fixes: string[] = []
    const result = await ReviewLoop.reviewUntilPass(
      { sessionID: "ses_a", cwd: "/tmp", maxRounds: 5 },
      {
        review: async (input) => {
          seen.push(input.previous)
          return verdicts.shift()!
        },
        sendFix: async (_id, text) => {
          fixes.push(text)
          return "done"
        },
        publish,
      },
    )
    expect(result.stopped).toBe("pass")
    expect(result.rounds).toBe(2)
    expect(fixes).toHaveLength(1)
    expect(fixes[0]).toContain("Fix these review findings:")
    expect(fixes[0]).toContain("a.ts:3")
    expect(seen[0]).toBeUndefined()
    expect((seen[1] as ReviewVerdict.Stored).verdict).toBe("FAIL")
  })

  test("stops at the round limit and caps rounds at 12", async () => {
    let reviews = 0
    const result = await ReviewLoop.reviewUntilPass(
      { sessionID: "ses_b", cwd: "/tmp", maxRounds: 3 },
      { review: async () => (reviews++, fail()), sendFix: async () => "done", publish },
    )
    expect(result).toMatchObject({ stopped: "limit", rounds: 3 })
    expect(reviews).toBe(3)
    expect(ReviewLoop.clampRounds(50)).toBe(12)
    expect(ReviewLoop.clampRounds(undefined)).toBe(5)
  })

  test("stops on abort, error and when there is nothing to review", async () => {
    const aborted = await ReviewLoop.reviewUntilPass(
      { sessionID: "ses_c", cwd: "/tmp" },
      { review: async () => fail(), sendFix: async () => "aborted", publish },
    )
    expect(aborted.stopped).toBe("abort")

    const errored = await ReviewLoop.reviewUntilPass(
      { sessionID: "ses_d", cwd: "/tmp" },
      {
        review: async () => fail(),
        sendFix: async () => {
          throw new Error("boom")
        },
        publish,
      },
    )
    expect(errored).toMatchObject({ stopped: "error", error: "boom" })

    const nothing = await ReviewLoop.reviewUntilPass(
      { sessionID: "ses_e", cwd: "/tmp" },
      { review: async () => null, sendFix: async () => "done", publish },
    )
    expect(nothing).toMatchObject({ stopped: "nothing", verdict: null })
  })
})

describe("goal round checks", () => {
  test("feeds failures back and escalates after two rounds with the same failures", async () => {
    const sessionID = `ses_goal_${Math.random().toString(36).slice(2)}`
    const deps = {
      review: async () => fail("same problem"),
      runCheck: async (command: string) => ({ command, ok: false, output: "error TS2322" }),
      goalConfig: async () => ({ checks: ["bun run typecheck"] }),
      escalationModel: async () => ({ providerID: "anthropic", modelID: "big" }),
    }
    const first = await GoalRoundChecks.run({ sessionID, cwd: "/tmp" }, deps)
    expect(first.feedback).toContain("bun run typecheck")
    expect(first.feedback).toContain("same problem")
    expect(first.escalate).toBeUndefined()
    const second = await GoalRoundChecks.run({ sessionID, cwd: "/tmp" }, deps)
    expect(second.escalate).toEqual({ providerID: "anthropic", modelID: "big" })
    const third = await GoalRoundChecks.run({ sessionID, cwd: "/tmp" }, deps)
    expect(third.escalate).toBeUndefined()
    GoalRoundChecks.reset(sessionID)
  })

  test("no feedback when checks and review pass; review can be turned off", async () => {
    let reviewed = 0
    const result = await GoalRoundChecks.run(
      { sessionID: "ses_goal_ok", cwd: "/tmp" },
      {
        review: async () => (reviewed++, record()),
        runCheck: async (command: string) => ({ command, ok: true, output: "" }),
        goalConfig: async () => ({ checks: ["true"], review: false }),
        escalationModel: async () => undefined,
      },
    )
    expect(result.feedback).toBeUndefined()
    expect(reviewed).toBe(0)
  })
})
