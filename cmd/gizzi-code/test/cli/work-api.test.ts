// @ts-nocheck
/**
 * Work views (gizzi computers, run --on, /decisions, /pr, /projects,
 * /routines, artifact share). The network is the only thing mocked: each test
 * answers fetch the way the real route does and checks what the caller gets
 * and which route + body went out (the contract with allternit-api, the
 * engine and the cloud relay).
 */
import { afterEach, beforeEach, describe, expect, test } from "bun:test"

const realFetch = globalThis.fetch
let calls: Array<{ url: string; method: string; body?: any; auth?: string }>
let reply: (url: string, init: RequestInit) => Response

beforeEach(() => {
  calls = []
  process.env.ALLTERNIT_API_URL = "http://gw.test"
  process.env.GIZZI_ENGINE_URL = "http://engine.test"
  process.env.ALLTERNIT_CLOUD_URL = "http://cloud.test"
  globalThis.fetch = (async (input: any, init: RequestInit = {}) => {
    const url = String(input)
    const headers = (init.headers ?? {}) as Record<string, string>
    calls.push({
      url,
      method: init.method ?? "GET",
      body: typeof init.body === "string" && init.body ? JSON.parse(init.body) : undefined,
      auth: headers.Authorization,
    })
    return reply(url, init)
  }) as typeof fetch
})

afterEach(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_API_TOKEN
})

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } })

const api = () => import("../../src/cli/work/api")

describe("decisions", () => {
  test("merges engine questions and approvals with the asking session's title", async () => {
    reply = (url) => {
      if (url.endsWith("/v1/question"))
        return json([{ id: "que_1", sessionID: "ses_a", questions: [{ question: "Which db?", options: [{ label: "pg" }] }] }])
      if (url.endsWith("/v1/permission"))
        return json([{ id: "per_1", sessionID: "ses_b", permission: "bash", patterns: ["rm -rf build"], metadata: { command: "rm -rf build" } }])
      if (url.endsWith("/session/ses_a")) return json({ title: "Bot: Ledger" })
      return json({}, 404)
    }
    const { listDecisions, approvalTarget, countDecisions } = await api()
    const list = await listDecisions()
    expect(list.map((d) => [d.kind, d.id, d.session ?? d.sessionID])).toEqual([
      ["question", "que_1", "Bot: Ledger"],
      ["approval", "per_1", "ses_b"],
    ])
    expect(approvalTarget(list[1])).toBe("rm -rf build")
    expect(await countDecisions()).toBe(2)
  })

  test("answers go to the engine reply routes in the engine's shapes", async () => {
    reply = () => json(true)
    const { answerQuestion, answerApproval } = await api()
    await answerQuestion("que_1", [["pg"], ["yes"]])
    await answerApproval("per_1", "always")
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ["POST", "http://engine.test/v1/question/que_1/reply", { answers: [["pg"], ["yes"]] }],
      ["POST", "http://engine.test/v1/permission/per_1/reply", { reply: "always" }],
    ])
  })

  test("count is undefined (badge hidden) when the engine is down", async () => {
    reply = () => {
      throw new TypeError("fetch failed")
    }
    const { countDecisions } = await api()
    expect(await countDecisions()).toBeUndefined()
  })
})

describe("computers", () => {
  test("start and stop post to the computer's lifecycle routes", async () => {
    reply = (url) => json({ id: "cmp_1", status: url.endsWith("/start") ? "running" : "stopped" })
    const { startComputer, stopComputer } = await api()
    expect((await startComputer("cmp_1")).status).toBe("running")
    expect((await stopComputer("cmp_1")).status).toBe("stopped")
    expect(calls.map((c) => `${c.method} ${c.url}`)).toEqual([
      "POST http://gw.test/api/v1/computers/cmp_1/start",
      "POST http://gw.test/api/v1/computers/cmp_1/stop",
    ])
  })

  test("pick matches an id first, then a name in any case", async () => {
    const { pick } = await api()
    const list = [
      { id: "rt_1", name: "Cloud" },
      { id: "Cloud", name: "other" },
    ]
    expect(pick(list, "Cloud")?.id).toBe("Cloud")
    expect(pick(list, "cloud")?.id).toBe("rt_1")
    expect(pick(list, "nope")).toBeUndefined()
  })

  test("run --on without an API key explains what is needed and sends nothing", async () => {
    reply = () => json({})
    const { runOnRuntime } = await api()
    await expect(runOnRuntime("rt_1", "hi")).rejects.toThrow(/ALLTERNIT_API_TOKEN/)
    expect(calls).toHaveLength(0)
  })

  test("run --on creates the session then sends the message through the relay", async () => {
    process.env.ALLTERNIT_API_TOKEN = "allternit_key"
    reply = (_url, init) => {
      const path = JSON.parse(String(init.body)).path
      return path === "/api/v1/agent-sessions" ? json({ id: "ses_r" }) : json({ ok: true })
    }
    const { runOnRuntime } = await api()
    const run = await runOnRuntime("rt_9", "fix the test", { model: { providerID: "claude-cli", modelID: "sonnet" } })
    expect(run.sessionID).toBe("ses_r")
    expect(run.web).toBe("https://ai.allternit.com/?runtime=rt_9&session=ses_r")
    expect(calls.every((c) => c.url === "http://cloud.test/api/v1/runtime-devices/rt_9/proxy")).toBe(true)
    expect(calls.every((c) => c.auth === "Bearer allternit_key")).toBe(true)
    expect(calls.map((c) => c.body.path)).toEqual(["/api/v1/agent-sessions", "/api/v1/agent-sessions/ses_r/messages"])
    expect(JSON.parse(calls[0].body.body).model).toEqual({ providerID: "claude-cli", modelID: "sonnet" })
    expect(JSON.parse(calls[1].body.body)).toEqual({ text: "fix the test" })
  })
})

describe("projects and routines", () => {
  test("project status chips come from metadata.status, which may be a JSON string", async () => {
    const { toProject } = await api()
    const p = toProject({
      id: "p1",
      title: "Launch",
      metadata: JSON.stringify({ kind: "bots", status: { lifecycle: "active", state: "at_risk", next_action: "Ship docs" } }),
    })
    expect(p).toMatchObject({ title: "Launch", kind: "bots", state: "at_risk", nextAction: "Ship docs", lifecycle: "active" })
    expect(toProject({ id: "p2", title: "x", metadata: "not json" }).state).toBeUndefined()
  })

  test("routines sort by next run with paused ones last, and pause is PUT { status }", async () => {
    reply = (_url, init) =>
      init.method === "PUT"
        ? json({ id: "r2", status: "paused" })
        : json([
            { id: "r1", name: "paused one", status: "paused", next_run_at: null },
            { id: "r2", name: "later", status: "active", next_run_at: "2026-11-11T00:00:00Z" },
            { id: "r3", name: "sooner", status: "active", next_run_at: "2026-11-10T00:00:00Z" },
          ])
    const { listRoutines, setRoutinePaused } = await api()
    expect((await listRoutines()).map((r) => r.id)).toEqual(["r3", "r2", "r1"])
    await setRoutinePaused("r2", true)
    expect(calls.at(-1)).toMatchObject({ method: "PUT", url: "http://gw.test/api/v1/automation/routines/r2", body: { status: "paused" } })
  })
})

describe("PR checks", () => {
  test("gh statusCheckRollup maps to pass/fail/pending and the footer chip", async () => {
    const { toBranchPR, prChip } = await api()
    const pr = toBranchPR({
      number: 123,
      title: "t",
      url: "u",
      state: "OPEN",
      isDraft: false,
      headRefName: "feat/x",
      statusCheckRollup: [
        { name: "build", conclusion: "SUCCESS", status: "COMPLETED" },
        { name: "lint", conclusion: "FAILURE", status: "COMPLETED" },
        { name: "e2e", conclusion: "", status: "IN_PROGRESS" },
        { context: "vercel", state: "SUCCESS" },
        { name: "skip", conclusion: "SKIPPED" },
      ],
    })
    expect(pr.checks.map((c) => c.state)).toEqual(["pass", "fail", "pending", "pass", "skip"])
    expect(prChip(pr)).toBe("PR #123 ✓2 ✗1 ●1")
  })
})

describe("artifact share", () => {
  test("share publishes the given version and returns the link", async () => {
    reply = () => json({ published: true, url: "https://a.allternit.app/x", version: 3 })
    const { shareArtifact } = await api()
    expect((await shareArtifact("art_1", 3)).url).toBe("https://a.allternit.app/x")
    expect(calls[0]).toMatchObject({ method: "POST", url: "http://gw.test/api/v1/content-artifacts/art_1/publish", body: { version: 3 } })
  })
})

describe("bots pause", () => {
  test("pause and resume PUT the switch as gizzi; get reads it", async () => {
    reply = (url, init) =>
      init.method === "PUT"
        ? json({ paused: JSON.parse(String(init.body)).paused, since: 1762700000, by: "gizzi" })
        : json({ paused: true, since: 1762700000, by: "gizzi" })
    const { setBotPause, getBotPause, describeBotPause } = await api()
    expect((await setBotPause(true)).paused).toBe(true)
    expect((await setBotPause(false)).paused).toBe(false)
    expect(calls.map((c) => [c.method, c.url, c.body])).toEqual([
      ["PUT", "http://gw.test/api/v1/bots/pause", { paused: true, by: "gizzi" }],
      ["PUT", "http://gw.test/api/v1/bots/pause", { paused: false, by: "gizzi" }],
    ])
    const state = await getBotPause()
    expect(describeBotPause(state)).toContain("All bots are paused since")
    expect(describeBotPause(state)).toContain("by gizzi")
    expect(describeBotPause({ paused: false, since: null, by: null })).toBe("Bots are running.")
  })

  test("an older runtime's 404 becomes the update message", async () => {
    reply = () => json({}, 404)
    const { getBotPause, setBotPause } = await api()
    await expect(getBotPause()).rejects.toThrow("too old for pause. Update it.")
    await expect(setBotPause(true)).rejects.toThrow("too old for pause. Update it.")
  })
})
