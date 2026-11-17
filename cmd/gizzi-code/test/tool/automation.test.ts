// @ts-nocheck
import { afterAll, beforeEach, describe, expect, test } from "bun:test"

/**
 * automation through the real core (schedules, rrule, heartbeat verdicts),
 * the real tool and the real routing client. Only the HTTP edge (allternit-api
 * bot-routing/automation/*) is stubbed: fetch records every call.
 */
import fs from "fs/promises"
import { mkdtempSync } from "node:fs"
import { tmpdir as osTmpdir } from "node:os"
import path from "path"

process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
process.env.GIZZI_CONFIG_DIR = mkdtempSync(path.join(osTmpdir(), "gizzi-automation-"))
const { tmpdir } = await import("../fixture/fixture")
const { Instance } = await import("../../src/project/instance")
const { Agent } = await import("../../src/runtime/loop/agent")
const { PermissionNext } = await import("../../src/permission/next")
const { ToolRegistry } = await import("../../src/runtime/tools/builtins/registry")
const { THREAD_TOOL_IDS } = await import("../../src/runtime/tools/builtins/thread-tools")
const { WORKER_BOT_TOOL_IDS } = await import("../../src/runtime/tools/builtins/worker-bot-tools")
const { PROJECT_SEND_TOOL_IDS } = await import("../../src/runtime/tools/builtins/project-send")
const { RRule } = await import("../../src/runtime/primitives/rrule")
const { Heartbeat } = await import("../../src/runtime/primitives/heartbeat")
const { Automation } = await import("../../src/runtime/primitives/automation")
const { AutomationTool } = await import("../../src/runtime/tools/builtins/automation")

const CONTRACTS = path.join(import.meta.dir, "../../../../contracts/automation")

type Call = { path: string; body: any }
let calls: Call[] = []
let route: (path: string, body: any) => Response | Promise<Response>
const realFetch = globalThis.fetch

function info(body: any, extra: any = {}) {
  return {
    id: "auto_1",
    title: body.title,
    kind: body.kind,
    target: body.target,
    status: "active",
    schedule: body.schedule,
    timezone: body.timezone,
    nextRunAt: body.schedule.type === "once" ? body.schedule.at : "2026-11-18T15:00:00.000Z",
    origin: "agent",
    initiation: body.initiation,
    sessionId: body.sessionId,
    createdAt: "2026-11-17T20:00:00.000Z",
    ...extra,
  }
}

const ok = (p: string, body: any) => {
  if (p.endsWith("/automation/create")) return Response.json({ automation: info(body) })
  if (p.endsWith("/automation/list")) return Response.json({ automations: [] })
  if (p.endsWith("/automation/delete")) return Response.json({ deleted: true, id: body.id })
  return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 })
}

beforeEach(() => {
  calls = []
  route = ok
  process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const body = JSON.parse(String(init.body))
    calls.push({ path: url.pathname, body })
    return route(url.pathname, body)
  }) as typeof fetch
})

afterAll(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
})

function ctx(opts: { metadata?: Record<string, unknown>; text?: string; asks?: any[] } = {}) {
  return {
    sessionID: "ses_auto",
    messageID: "msg_assistant",
    callID: "call_1",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [
      {
        info: { id: "msg_user", sessionID: "ses_auto", role: "user", time: { created: 0 }, metadata: opts.metadata },
        parts: [{ type: "text", text: opts.text ?? "Remind me later" }],
      },
    ],
    metadata: () => {},
    ask: async (request: any) => {
      opts.asks?.push(request)
    },
  } as any
}

async function run(params: any, c = ctx()) {
  const tool = await AutomationTool.init()
  return tool.execute(params, c)
}

const paths = () => calls.map((c) => c.path.replace(/^.*\/bot-routing/, ""))

function rruleCode(rrule: string, zone = "UTC"): string {
  try {
    RRule.rruleToCron(rrule, zone)
    return "ok"
  } catch (error) {
    return error.code
  }
}

describe("RRule.rruleToCron", () => {
  test("supported rules map to the cron line the schedulers read", () => {
    const cases: Array<[string, string]> = [
      ["FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=9;BYMINUTE=30", "30 9 * * 1,3,5"],
      ["RRULE:FREQ=DAILY;BYHOUR=8", "0 8 * * *"],
      ["FREQ=DAILY;BYHOUR=8,17;BYDAY=MO,TU,WE,TH,FR", "0 8,17 * * 1,2,3,4,5"],
      ["FREQ=HOURLY", "0 * * * *"],
      ["FREQ=HOURLY;INTERVAL=2;BYMINUTE=15", "15 */2 * * *"],
      ["FREQ=HOURLY;BYHOUR=9,10,11;BYDAY=SA", "0 9,10,11 * * 6"],
      ["FREQ=MINUTELY;INTERVAL=30;BYHOUR=9,10;BYDAY=MO", "*/30 9,10 * * 1"],
      ["FREQ=MINUTELY;INTERVAL=15", "*/15 * * * *"],
      ["FREQ=MONTHLY;BYMONTHDAY=1,15;BYHOUR=7", "0 7 1,15 * *"],
      ["FREQ=MONTHLY;INTERVAL=3;BYMONTHDAY=1;BYHOUR=9", "0 9 1 1-12/3 *"],
      ["freq=weekly;byday=su;byhour=20", "0 20 * * 0"],
    ]
    for (const [rrule, cron] of cases) expect(RRule.rruleToCron(rrule).cron).toBe(cron)
  })

  test("COUNT becomes maxRuns and UNTIL an instant; a floating UNTIL reads in the zone", () => {
    expect(RRule.rruleToCron("FREQ=DAILY;BYHOUR=9;COUNT=5")).toEqual({ cron: "0 9 * * *", maxRuns: 5 })
    expect(RRule.rruleToCron("FREQ=DAILY;BYHOUR=9;UNTIL=20261231T235900Z").until).toBe("2026-12-31T23:59:00.000Z")
    // A date alone is the end of that day in the zone (Chicago is UTC-6 in winter).
    expect(RRule.rruleToCron("FREQ=DAILY;BYHOUR=9;UNTIL=20261231", "America/Chicago").until).toBe("2027-01-01T05:59:59.000Z")
  })

  test("rules cron can't express exactly are refused with the supported forms", () => {
    for (const rrule of [
      "FREQ=YEARLY;BYMONTH=1",
      "FREQ=WEEKLY;BYDAY=1MO;BYHOUR=9",
      "FREQ=MONTHLY;BYDAY=-1FR;BYHOUR=9",
      "FREQ=DAILY;INTERVAL=2;BYHOUR=9",
      "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO;BYHOUR=9",
      "FREQ=MONTHLY;BYDAY=MO;BYMONTHDAY=1;BYHOUR=9",
      "FREQ=MINUTELY;INTERVAL=45",
      "FREQ=HOURLY;INTERVAL=5",
      "FREQ=DAILY;BYHOUR=9;WKST=MO",
      "FREQ=MONTHLY;BYMONTHDAY=-1;BYHOUR=9",
    ]) {
      expect([rrule, rruleCode(rrule)]).toEqual([rrule, "unsupported"])
    }
    expect(() => RRule.rruleToCron("FREQ=YEARLY")).toThrow(/Supported rrule forms/)
    for (const rrule of ["", "FREQ=DAILY", "FREQ=WEEKLY;BYHOUR=9", "FREQ=DAILY;BYHOUR=25", "FREQ=DAILY;BYHOUR=9;COUNT=3;UNTIL=20270101", "BYHOUR=9"]) {
      expect([rrule, rruleCode(rrule)]).toEqual([rrule, "invalid"])
    }
  })
})

describe("minimum interval", () => {
  test("anything that could run under 15 minutes apart is refused, across midnight too", () => {
    expect(rruleCode("FREQ=MINUTELY;INTERVAL=5")).toBe("too_frequent")
    expect(rruleCode("FREQ=MINUTELY")).toBe("too_frequent")
    expect(rruleCode("FREQ=HOURLY;BYMINUTE=0,10")).toBe("too_frequent")
    for (const cron of ["* * * * *", "*/10 * * * *", "0,10 9 * * *", "5,55 0,23 * * *"]) {
      expect(() => RRule.validateCron(cron)).toThrow(/at most every 15 minutes/)
    }
    expect(RRule.smallestGapMinutes("5,55 0,23 * * *")).toBe(10)
    expect(RRule.smallestGapMinutes("0 9 * * 1-5")).toBe(1440)
    expect(RRule.validateCron("  */15   9-17 * * 1-5 ")).toBe("*/15 9-17 * * 1-5")
  })

  test("malformed cron lines are refused (cron_lite syntax: numbers only, 5 fields)", () => {
    for (const cron of ["0 9 * *", "0 9 * * MON", "@daily", "60 9 * * *", "0 9 32 * *", "0 9-5 * * *", "0 9 * * 1 2026"]) {
      expect([cron, (() => { try { RRule.validateCron(cron); return "ok" } catch (e) { return e.code } })()]).toEqual([cron, "invalid"])
    }
    expect(RRule.validateCron("0 9 * * 7")).toBe("0 9 * * 7")
  })
})

describe("Automation.resolveSchedule", () => {
  const NOW = new Date("2026-11-17T20:00:00Z")

  test("at + zone is the instant of that wall time in that zone", () => {
    expect(Automation.resolveSchedule({ at: "2026-11-18T09:00" }, "America/Chicago", NOW).schedule).toEqual({
      type: "once",
      at: "2026-11-18T15:00:00.000Z",
    })
    expect(Automation.resolveSchedule({ at: "2026-11-18T09:00" }, "Asia/Kolkata", NOW).schedule.at).toBe("2026-11-18T03:30:00.000Z")
    // A repeated hour (fall back) is the first one: 01:30 CDT, not CST.
    expect(Automation.resolveSchedule({ at: "2027-11-07T01:30" }, "America/Chicago", NOW).schedule.at).toBe("2027-11-07T06:30:00.000Z")
  })

  test("a time the clocks skip is refused and asks for another time", () => {
    const attempt = () => Automation.resolveSchedule({ at: "2027-03-14T02:30" }, "America/Chicago", NOW)
    expect(attempt).toThrow(/doesn't exist in America\/Chicago.*Ask the user for another time/)
    try {
      attempt()
    } catch (error) {
      expect(error.code).toBe("nonexistent_time")
    }
  })

  test("hand-written offsets, impossible dates and out-of-window times are refused", () => {
    const code = (input: any) => {
      try {
        Automation.resolveSchedule(input, "America/Chicago", NOW)
        return "ok"
      } catch (error) {
        return error.code
      }
    }
    expect(code({ at: "2026-11-18T09:00-06:00" })).toBe("invalid")
    expect(code({ at: "2026-11-18T15:00Z" })).toBe("invalid")
    expect(code({ at: "2027-02-30T09:00" })).toBe("invalid")
    expect(code({ at: "2026-11-17T13:00" })).toBe("out_of_window") // 19:00Z, an hour ago
    expect(code({ at: "2026-11-17T14:00" })).toBe("out_of_window") // exactly now
    expect(code({ at: "2027-11-18T09:00" })).toBe("out_of_window") // over 365 days
    expect(code({ delay_minutes: 0 })).toBe("out_of_window")
    expect(code({ delay_minutes: 1.5 })).toBe("out_of_window")
    expect(code({})).toBe("invalid")
    expect(code({ at: "2026-11-18T09:00", cron: "0 9 * * *" })).toBe("invalid")
    expect(code({ rrule: "FREQ=DAILY;BYHOUR=9;UNTIL=20261101" })).toBe("out_of_window")
    expect(Automation.resolveSchedule({ delay_minutes: 20 }, "UTC", NOW).schedule.at).toBe("2026-11-17T20:20:00.000Z")
  })
})

describe("Heartbeat.parseVerdict", () => {
  test("every shared vector (contracts/automation/heartbeat-verdict.json)", async () => {
    const vectors = JSON.parse(await fs.readFile(path.join(CONTRACTS, "heartbeat-verdict.json"), "utf8"))
    expect(vectors.length).toBeGreaterThan(20)
    for (const v of vectors) expect([v.note, Heartbeat.parseVerdict(v.input)]).toEqual([v.note, v.expected])
  })

  test("the instructions match the shared copy the Rust scheduler reads", async () => {
    const shared = await fs.readFile(path.join(CONTRACTS, "heartbeat-instructions.txt"), "utf8")
    expect(Heartbeat.heartbeatInstructions()).toBe(shared.trimEnd())
    expect(Heartbeat.runPrompt("Check the deploy.")).toEndWith(shared.trimEnd())
  })
})

describe("tool.automation", () => {
  test("defaults: list is allowed, every write asks", async () => {
    await using tmp = await tmpdir()
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const build = await Agent.get("build")
        expect(PermissionNext.evaluate("automation", "list", build.permission).action).toBe("allow")
        for (const action of ["create", "update", "delete", "run_now"]) {
          expect([action, PermissionNext.evaluate("automation", action, build.permission).action]).toEqual([action, "ask"])
        }
        expect(PermissionNext.ALWAYS_ASK.has("automation")).toBe(false)
      },
    })
  })

  test("reaches every client, not only bot chats", async () => {
    const botOnly = new Set<string>([...THREAD_TOOL_IDS, ...WORKER_BOT_TOOL_IDS, ...PROJECT_SEND_TOOL_IDS, "message_agent", "suggest_task", "grant_task_permission"])
    const prev = process.env.GIZZI_CLIENT
    await using tmp = await tmpdir()
    try {
      for (const client of ["app", "cli"]) {
        process.env.GIZZI_CLIENT = client
        await Instance.provide({
          directory: tmp.path,
          fn: async () => {
            expect(await ToolRegistry.ids()).toContain("automation")
            expect(botOnly.has("automation")).toBe(false)
          },
        })
      }
    } finally {
      if (prev === undefined) delete process.env.GIZZI_CLIENT
      else process.env.GIZZI_CLIENT = prev
    }
  })

  test("create sends the instant in the user's zone and says how runs are approved", async () => {
    const asks: any[] = []
    const result = await run(
      {
        action: "create",
        kind: "heartbeat",
        title: "Watch the deploy",
        prompt: "Check whether the production deploy of PR #12 finished. Last state: building.",
        schedule: { delay_minutes: 90 },
      },
      ctx({ metadata: { time_zone: "America/Chicago" }, asks }),
    )
    expect(paths()).toEqual(["/automation/create"])
    const body = calls[0].body
    expect(body).toMatchObject({
      sessionId: "ses_auto",
      kind: "heartbeat",
      title: "Watch the deploy",
      target: "this_thread",
      initiation: "user_request",
      timezone: "America/Chicago",
      scheduleInput: { delayMinutes: 90 },
    })
    expect(body.schedule.type).toBe("once")
    expect(Math.abs(new Date(body.schedule.at).getTime() - (Date.now() + 90 * 60_000))).toBeLessThan(60_000)
    expect(asks.map((a) => [a.permission, a.patterns])).toEqual([["automation", ["create"]]])
    expect(result.output).toContain(Automation.APPROVAL_SENTENCE)
    expect(result.output).toContain(`Next run: ${Automation.formatInZone(body.schedule.at, "America/Chicago")}.`)
    expect(result.output).toContain("America/Chicago")
    expect(result.metadata).toMatchObject({ action: "create", status: "ok", automation: { id: "auto_1" } })
  })

  test("list asks with the list pattern and returns what the server has", async () => {
    const asks: any[] = []
    route = (p, body) =>
      Response.json({
        automations: [
          info({ title: "Standup brief", kind: "message", target: "new_session", schedule: { type: "cron", expression: "0 9 * * 1-5" }, timezone: "Europe/Berlin" }),
        ],
      })
    const result = await run({ action: "list" }, ctx({ asks }))
    expect(asks.map((a) => a.patterns)).toEqual([["list"]])
    expect(result.output).toContain('auto_1 · "Standup brief"')
    expect(result.output).toContain('cron "0 9 * * 1-5" (Europe/Berlin)')
  })

  test("a turn started by a scheduled task can't create one, and nobody is asked", async () => {
    for (const c of [
      { metadata: { automation_run: { id: "auto_0" } } },
      { text: "[routine: morning] Check the inbox" },
    ]) {
      const asks: any[] = []
      await expect(
        run({ action: "create", prompt: "Again tomorrow", schedule: { delay_minutes: 60 } }, ctx({ ...c, asks })),
      ).rejects.toThrow(/can't create scheduled tasks/)
      expect(asks).toEqual([])
    }
    expect(calls).toEqual([])
    // Listing and deleting still work from such a run.
    await run({ action: "delete", id: "auto_0" }, ctx({ metadata: { automation_run: { id: "auto_0" } } }))
    expect(paths()).toEqual(["/automation/delete"])
  })

  test("an invalid zone or schedule is refused before anyone is asked", async () => {
    const asks: any[] = []
    await expect(
      run({ action: "create", prompt: "Call Sam", timezone: "Mars/Olympus", schedule: { at: "2030-01-01T09:00" } }, ctx({ asks })),
    ).rejects.toThrow(/not an IANA time zone/)
    await expect(run({ action: "create", prompt: "Ping", schedule: { cron: "* * * * *" } }, ctx({ asks }))).rejects.toThrow(
      /at most every 15 minutes/,
    )
    await expect(run({ action: "delete" }, ctx({ asks }))).rejects.toThrow(/needs the scheduled task's id/)
    expect(asks).toEqual([])
    expect(calls).toEqual([])
  })

  test("no platform and no scheduler running here: says so and gives the start command", async () => {
    delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
    delete process.env.ALLTERNIT_API_TOKEN
    let result = await run({ action: "create", prompt: "Call Sam", schedule: { delay_minutes: 30 } })
    expect(result.metadata).toMatchObject({ status: "no_scheduler", reason: "no_credential", built: { action: "create" } })
    expect(result.output).toMatch(/^Not scheduled: /)
    expect(result.output).toContain("gizzi jobs start")
    expect(calls).toEqual([])

    process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
    route = () => {
      throw new TypeError("fetch failed")
    }
    result = await run({ action: "list" })
    expect(result.metadata).toMatchObject({ status: "no_scheduler", reason: "unreachable" })

    route = () => new Response("Not Found", { status: 404 })
    result = await run({ action: "list" })
    expect(result.metadata).toMatchObject({ status: "no_scheduler", reason: "no_route" })

    // A refusal from the server is an error the model reads, not a fallback.
    route = () => new Response(JSON.stringify({ error: "You already have 25 active scheduled tasks.", code: "cap_reached" }), { status: 429 })
    await expect(run({ action: "create", prompt: "Call Sam", schedule: { delay_minutes: 30 } })).rejects.toThrow(
      /cap_reached: You already have 25 active/,
    )
  })
})

describe("tool.automation on this computer's scheduler (no platform)", () => {
  test("create, list, update and delete go to the CronService the /cron routes use", async () => {
    const { CronService } = await import("../../src/runtime/automation/cron/service")
    const dir = mkdtempSync(path.join(osTmpdir(), "gizzi-automation-cron-"))
    CronService.initialize({ dbPath: path.join(dir, "cron.db"), checkIntervalMs: 3_600_000 })
    CronService.start()
    try {
      delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
      delete process.env.ALLTERNIT_API_TOKEN
      const before = Date.now()
      const created = await run(
        {
          action: "create",
          kind: "heartbeat",
          title: "Watch the deploy",
          prompt: "Check whether the production deploy of PR #12 finished.",
          schedule: { delay_minutes: 90 },
        },
        ctx({ metadata: { time_zone: "America/Chicago" } }),
      )
      expect(calls).toEqual([])
      expect(created.metadata).toMatchObject({ status: "ok", via: "cron", automation: { title: "Watch the deploy", kind: "heartbeat", status: "active" } })
      expect(created.output).toContain("this computer's scheduler")

      const jobs = CronService.list()
      expect(jobs).toHaveLength(1)
      const job = jobs[0]
      expect(job.type).toBe("agent")
      expect(job.config).toMatchObject({
        kind: "heartbeat",
        target: "this_thread",
        targetSessionId: "ses_auto",
        delivery: "ephemeral",
        origin: "agent",
        initiation: "user_request",
        timezone: "America/Chicago",
        automationSchedule: { type: "once" },
      })
      // The scheduler fires at the instant asked for: 90 minutes from now.
      const due = Date.parse(job.nextRunAt) - before
      expect(due).toBeGreaterThan(89 * 60_000)
      expect(due).toBeLessThan(91 * 60_000)

      const listed = await run({ action: "list" })
      expect(listed.metadata).toMatchObject({ status: "ok", via: "cron" })
      expect(listed.metadata.automations.map((a) => a.id)).toContain(job.id)
      expect(listed.output).toContain("Watch the deploy")

      const paused = await run({ action: "update", id: job.id, status: "paused" })
      expect(paused.metadata.automation.status).toBe("paused")
      expect(CronService.get(job.id).status).toBe("paused")

      const deleted = await run({ action: "delete", id: job.id })
      expect(deleted.metadata).toMatchObject({ status: "ok", id: job.id })
      expect(CronService.get(job.id)).toBeNull()
    } finally {
      CronService.close()
    }
  })

  test("a recurring task is armed at its next run in its own zone, not the machine's", async () => {
    // 09:00 in Tokyo on weekdays, from Tue 2026-11-17 12:00 UTC (21:00 Tokyo): next is Wed 09:00 Tokyo = 00:00 UTC.
    const next = Automation.nextCronRun("0 9 * * 1-5", "Asia/Tokyo", new Date("2026-11-17T12:00:00Z"))
    expect(next.toISOString()).toBe("2026-11-18T00:00:00.000Z")
    // Friday evening skips the weekend.
    expect(Automation.nextCronRun("0 9 * * 1-5", "Asia/Tokyo", new Date("2026-11-20T12:00:00Z")).toISOString()).toBe("2026-11-23T00:00:00.000Z")
    // The hour the clocks skip (Chicago, 2027-03-14 02:30) is skipped, not moved.
    expect(Automation.nextCronRun("30 2 * * *", "America/Chicago", new Date("2027-03-13T12:00:00Z")).toISOString()).toBe("2027-03-15T07:30:00.000Z")
    // UNTIL ends it.
    const config = { automationSchedule: { type: "cron", expression: "0 9 * * *" }, timezone: "UTC", until: "2026-11-18T08:00:00.000Z" }
    expect(Automation.nextFire(config, new Date("2026-11-17T10:00:00Z"))).toBeNull()
  })
})

