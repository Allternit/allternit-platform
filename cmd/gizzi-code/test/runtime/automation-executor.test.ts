// @ts-nocheck
import { describe, expect, test } from "bun:test"
import fs from "fs/promises"
import path from "path"

/**
 * Scheduled tasks on this computer's scheduler (the automation tool's /cron
 * fallback): the real cron agent executor, the real verdict parser and the
 * shared verdict vectors. Only the session/model edge (AutomationRunDeps) is
 * a fake: no live model runs in a unit test.
 */
const { AgentExecutor, AUTOMATION_FAIL_STREAK } = await import("../../src/runtime/automation/cron/executors/agent-executor")

const vectors = JSON.parse(await fs.readFile(path.join(import.meta.dir, "../../../../contracts/automation/heartbeat-verdict.json"), "utf8"))
const reply = (kind: string) => vectors.find((v) => v.expected.kind === kind)

function fakeDeps(opts: { reply?: string; blocked?: string } = {}) {
  const calls = { isolated: [], delivered: [], notices: [] }
  const deps = {
    async deliverTurn(input) {
      calls.delivered.push(input)
      return { text: "Done.", tokensUsed: 3 }
    },
    async runIsolated(input) {
      calls.isolated.push(input)
      return { text: opts.reply ?? "", tokensUsed: 5, sessionId: "ses_side" }
    },
    async postNotice(input) {
      calls.notices.push(input)
      return { posted: true, push: input.push ? "sent" : "not_requested" }
    },
    async blockedOn() {
      return opts.blocked
    },
  }
  return { deps, calls }
}

function job(config: Record<string, unknown>, extra: Record<string, unknown> = {}) {
  const now = new Date().toISOString()
  return {
    id: "job_1",
    name: "Watch the deploy",
    type: "agent",
    status: "active",
    schedule: { type: "interval", seconds: 900 },
    createdAt: now,
    updatedAt: now,
    nextRunAt: now,
    runCount: 0,
    failCount: 0,
    tags: ["automation"],
    metadata: {},
    config: {
      prompt: "Check whether the deploy of PR #12 finished.",
      kind: "heartbeat",
      target: "this_thread",
      targetSessionId: "ses_thread",
      delivery: "ephemeral",
      origin: "agent",
      title: "Watch the deploy",
      automationSchedule: { type: "cron", expression: "0 * * * *" },
      timezone: "UTC",
      ...config,
    },
    ...extra,
  }
}

function run(triggeredBy = "schedule") {
  return { id: "run_1", jobId: "job_1", status: "running", scheduledAt: new Date().toISOString(), attempt: 1, triggeredBy, metadata: {} }
}

async function execute(j, r, deps) {
  return new AgentExecutor({ defaultCwd: "/cwd", automationDeps: deps }).execute(j, r, new AbortController().signal)
}

describe("heartbeat runs on this computer's scheduler", () => {
  test("the check runs isolated, with the verdict instructions and the run's id on the turn", async () => {
    const { deps, calls } = fakeDeps({ reply: reply("dont_notify").input })
    await execute(job({}), run(), deps)
    expect(calls.isolated).toHaveLength(1)
    const turn = calls.isolated[0]
    expect(turn.prompt).toStartWith("Check whether the deploy of PR #12 finished.")
    expect(turn.prompt).toContain("DONT_NOTIFY")
    expect(turn.metadata).toEqual({ automation_run: { id: "job_1", runId: "run_1" } })
    expect(turn.keep).toBe(false)
    expect(calls.delivered).toHaveLength(0)
  })

  test("DONT_NOTIFY is a quiet run: nothing posted, the task stays on its schedule", async () => {
    const { deps, calls } = fakeDeps({ reply: reply("dont_notify").input })
    const j = job({})
    const r = run()
    await execute(j, r, deps)
    expect(calls.notices).toHaveLength(0)
    expect(r.metadata.automation).toMatchObject({ outcome: "quiet", verdict: "dont_notify", quiet: true })
    expect(j.config.lastRunStatus).toBe("quiet")
    expect(j.status).toBe("active")
    // Re-armed at the next whole hour.
    expect(j.schedule.type).toBe("interval")
    expect(j.schedule.seconds).toBeGreaterThan(0)
    expect(j.schedule.seconds).toBeLessThanOrEqual(3600)
  })

  test("NOTIFY posts the line to the thread once, with a push", async () => {
    const v = reply("notify")
    const { deps, calls } = fakeDeps({ reply: v.input })
    const j = job({})
    const r = run()
    await execute(j, r, deps)
    expect(calls.notices).toEqual([
      { sessionID: "ses_thread", text: v.expected.line, push: true, automation: { id: "job_1", title: "Watch the deploy", kind: "heartbeat" } },
    ])
    expect(r.metadata.automation).toMatchObject({ outcome: "notify", line: v.expected.line, posted: true })
    expect(j.status).toBe("active")
  })

  test("DONE posts, then ends the task", async () => {
    const v = reply("done")
    const { deps, calls } = fakeDeps({ reply: v.input })
    const j = job({})
    await execute(j, run(), deps)
    expect(calls.notices.map((n) => n.text)).toEqual([v.expected.line])
    expect(j.status).toBe("disabled")
    expect(j.config.ended).toBe("completed")
    expect(j.config.lastRunStatus).toBe("done")
  })

  test("no verdict fails the run and posts nothing; three in a row pause the task and say so", async () => {
    const v = reply("none")
    const { deps, calls } = fakeDeps({ reply: v.input })
    const j = job({})
    for (let i = 1; i <= AUTOMATION_FAIL_STREAK; i++) {
      const r = run()
      await expect(execute(j, r, deps)).rejects.toThrow(/didn't end with NOTIFY/)
      expect(r.metadata.automation).toMatchObject({ outcome: "failed", verdict: "none" })
      if (i < AUTOMATION_FAIL_STREAK) {
        expect(j.status).toBe("active")
        expect(calls.notices).toHaveLength(0)
      }
    }
    expect(j.status).toBe("paused")
    expect(calls.notices).toHaveLength(1)
    expect(calls.notices[0].text).toContain("paused after 3 failed runs")
  })

  test("a check that stopped at an approval fails as needs_you and shows the card without a second push", async () => {
    const { deps, calls } = fakeDeps({ reply: "I need to run the deploy script.", blocked: "use bash (./deploy.sh)" })
    const j = job({})
    const r = run()
    await expect(execute(j, r, deps)).rejects.toThrow(/needs the owner's approval/)
    expect(j.config.lastRunStatus).toBe("needs_you")
    expect(calls.notices).toEqual([
      expect.objectContaining({ text: '"Watch the deploy" stopped: it needs your approval to use bash (./deploy.sh).', push: false }),
    ])
  })
})

describe("message runs and the schedule", () => {
  test("this_thread delivers a marked turn into the thread; a one-shot then ends", async () => {
    const { deps, calls } = fakeDeps()
    const j = job({
      kind: "message",
      delivery: "session",
      prompt: "Remind the user to call Sam.",
      title: "Call Sam",
      automationSchedule: { type: "once", at: new Date(Date.now() - 60_000).toISOString() },
    })
    const r = run()
    await execute(j, r, deps)
    expect(calls.delivered).toHaveLength(1)
    expect(calls.delivered[0]).toMatchObject({
      sessionID: "ses_thread",
      text: "[routine: Call Sam] Remind the user to call Sam.",
      metadata: { automation_run: { id: "job_1", runId: "run_1" } },
    })
    expect(r.output).toBe("Done.")
    expect(j.status).toBe("disabled")
    expect(j.config.lastRunStatus).toBe("delivered")
  })

  test("new_session runs in a session of its own and keeps it", async () => {
    const { deps, calls } = fakeDeps({ reply: "Here is the summary." })
    const j = job({ kind: "message", target: "new_session", delivery: "ephemeral", targetSessionId: undefined })
    await execute(j, run(), deps)
    expect(calls.isolated[0]).toMatchObject({ keep: true, title: "Watch the deploy" })
    expect(calls.delivered).toHaveLength(0)
    expect(j.status).toBe("active")
  })

  test("COUNT ends the task on its last run", async () => {
    const { deps } = fakeDeps({ reply: reply("dont_notify").input })
    const j = job({}, { maxRuns: 2, runCount: 1 })
    await execute(j, run(), deps)
    expect(j.status).toBe("disabled")
    expect(j.config.ended).toBe("completed")
  })

  test("a run due more than 12 hours ago is logged as missed and not run", async () => {
    const { deps, calls } = fakeDeps({ reply: reply("notify").input })
    const j = job(
      { kind: "message", delivery: "session", automationSchedule: { type: "once", at: new Date(Date.now() - 13 * 3600_000).toISOString() } },
      { nextRunAt: new Date(Date.now() - 13 * 3600_000).toISOString() },
    )
    await expect(execute(j, run("wake"), deps)).rejects.toThrow(/^Missed: /)
    expect(calls.delivered).toHaveLength(0)
    expect(j.status).toBe("disabled")
    expect(j.config.ended).toBe("missed")
    // A manual run is never "late".
    const k = job({ kind: "message", delivery: "session" }, { nextRunAt: new Date(Date.now() - 13 * 3600_000).toISOString() })
    await execute(k, run("manual"), deps)
    expect(calls.delivered).toHaveLength(1)
  })
})
