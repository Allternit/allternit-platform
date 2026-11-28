// @ts-nocheck
import { test, expect, afterEach } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionTrace } from "../../src/runtime/session/trace"
import { Identifier } from "../../src/shared/id/id"
import fs from "fs/promises"
import path from "path"
import { EvalStore } from "../../src/runtime/eval/store"
import * as Jobs from "../../src/runtime/eval/jobs"
import { EvalRoutes } from "../../src/runtime/server/routes/eval"
import { tmpdir } from "../fixture/fixture"

const app = EvalRoutes()
const created: string[] = []

afterEach(async () => {
  for (const id of created.splice(0)) await EvalStore.deleteCase(id).catch(() => {})
})

async function makeSession(prompt = "Fix the failing test") {
  const session = await Session.create({ title: "eval routes test" })
  const t0 = Date.now()
  const messageID = Identifier.ascending("message")
  await Session.updateMessage({
    id: messageID,
    sessionID: session.id,
    role: "user",
    time: { created: t0 },
    agent: "build",
    model: { providerID: "test", modelID: "test" },
  })
  await Session.updatePart({ id: Identifier.ascending("part"), sessionID: session.id, messageID, type: "text", text: prompt })
  const row = (kind: string, data: unknown, time: number) => SessionTrace.append({ sessionID: session.id, kind, data, time })
  row("request.started", { agent: "build" }, t0)
  row("request.completed", { cost: 0.01, tokens: { input: 100, output: 50 } }, t0 + 2000)
  return session
}

const post = (body: unknown, headers: Record<string, string> = {}) =>
  app.request("/cases", { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body) })

async function inProject<T>(fn: () => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return Instance.provide({ directory: tmp.path, fn })
}

test("save, list, get and delete a case", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const res = await post({ session_id: s.id, label: "good", note: "works" }, { "x-allternit-device-platform": "ios" })
    expect(res.status).toBe(201)
    const body = await res.json()
    created.push(body.case.id)
    expect(body.case).toMatchObject({
      suite: "my-cases",
      title: "Fix the failing test",
      label: "good",
      session_id: s.id,
      needs_check: false,
      last_result: null,
    })
    expect(typeof body.case.saved_at).toBe("number")
    expect(typeof body.case.bytes).toBe("number")
    expect(["archive", "none"]).toContain(body.case.workspace_kind)
    expect(body.self_test).toEqual({ passed: true, agrees_with_label: true })
    expect(body.needs_check).toBe(false)

    const list = await (await app.request(`/cases?session_id=${s.id}`)).json()
    expect(list.cases.map((c) => c.id)).toEqual([body.case.id])
    const other = await (await app.request("/cases?session_id=ses_none")).json()
    expect(other.cases).toEqual([])

    const one = await (await app.request(`/cases/${body.case.id}`)).json()
    expect(one.case.id).toBe(body.case.id)
    expect(one.case.origin.surface).toBe("ios")

    const suites = await (await app.request("/suites")).json()
    expect(suites.suites.find((x) => x.id === "my-cases").cases).toBeGreaterThanOrEqual(1)

    const del = await app.request(`/cases/${body.case.id}`, { method: "DELETE" })
    expect(del.status).toBe(204)
    const gone = await app.request(`/cases/${body.case.id}`)
    expect(gone.status).toBe(404)
    expect((await gone.json()).error).toBe("case_not_found")
  })
})

test("unknown ids give 404 case_not_found with a message", async () => {
  await inProject(async () => {
    const res = await app.request("/cases/case_NOPE")
    expect(res.status).toBe(404)
    const body = await res.json()
    expect(body.error).toBe("case_not_found")
    expect(typeof body.message).toBe("string")
    expect((await app.request("/cases/case_NOPE", { method: "DELETE" })).status).toBe(404)
  })
})

test("an unknown session is 404 session_not_found", async () => {
  await inProject(async () => {
    const res = await post({ session_id: "ses_doesnotexist", label: "good" })
    expect(res.status).toBe(404)
    expect((await res.json()).error).toBe("session_not_found")
  })
})

test("an invalid label is 400 with the error body", async () => {
  const res = await post({ session_id: "ses_x", label: "great" })
  expect(res.status).toBe(400)
  const body = await res.json()
  expect(body.error).toBe("invalid_request")
  expect(typeof body.message).toBe("string")
  expect((await post("not an object")).status).toBe(400)
})

// ── run routes ──────────────────────────────────────────────────────────────

const json = (path: string, body?: unknown, method = "POST") =>
  app.request(path, { method, headers: { "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) })

async function makeSuiteInStore(name: string) {
  const dir = path.join(EvalStore.root(), "suites", name)
  const id = "case_routetest"
  await fs.mkdir(path.join(dir, "cases", id), { recursive: true })
  await fs.writeFile(
    path.join(dir, "cases", id, "case.json"),
    JSON.stringify({
      schema: "allternit.gizzi_eval_case.v1",
      id,
      suite: name,
      title: "route test",
      label: "good",
      origin: { session_id: "x", through_message_id: null, saved_at: 0, gizzi_version: "0", surface: "cli" },
      turns: [{ role: "user", text: "hang" }],
      workspace: { kind: "none", reason: "no_project" },
      checks: [],
      graders: [{ type: "status", equals: "completed" }],
      redaction: { secrets: 0, paths: 0, entries_dropped: 0 },
    }),
  )
  await fs.writeFile(
    path.join(dir, "suite.json"),
    JSON.stringify({ schema: "allternit.eval_suite.v1", id: name, title: name, runner: "gizzi", cases: [{ id, source: { kind: "session", path: `cases/${id}/case.json` } }] }),
  )
  return dir
}

test("a live run without a price is refused with 409", async () => {
  const res = await json("/runs", { suite: "my-cases", mode: "live" })
  expect(res.status).toBe(409)
  expect(await res.json()).toEqual({ error: "estimate_required", message: "Get a price first." })
  const bad = await json("/runs", { suite: "my-cases", mode: "live", estimate_id: "est_nope" })
  expect(bad.status).toBe(409)
})

test("a replay run answers with a report, and it shows in the run list and suite last_run", async () => {
  const name = `replay-${Date.now().toString(36)}`
  const dir = await makeSuiteInStore(name)
  try {
    const res = await json("/runs", { suite: name, mode: "replay" })
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.run_id).toMatch(/^run_/)
    expect(body.report.schema).toBe("allternit.eval_report.v1")
    expect(body.report.extra.mode).toBe("replay")
    const list = await (await app.request(`/runs?suite=${name}`)).json()
    expect(list.runs.map((r: any) => r.run_id)).toContain(body.run_id)
    const got = await (await app.request(`/runs/${body.run_id}`)).json()
    expect(got).toMatchObject({ mode: "replay", state: "done" })
    const suites = await (await app.request("/suites")).json()
    expect(suites.suites.find((s: any) => s.id === name).last_run.run_id).toBe(body.run_id)
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
})

test("an unknown run is 404 run_not_found", async () => {
  const res = await app.request("/runs/run_doesnotexist")
  expect(res.status).toBe(404)
  expect((await res.json()).error).toBe("run_not_found")
  expect((await json("/runs/run_doesnotexist/cancel")).status).toBe(404)
})

test("a live run can be priced, started and cancelled", async () => {
  const name = `live-${Date.now().toString(36)}`
  const dir = await makeSuiteInStore(name)
  // A stand-in for `gizzi` that never calls a model: the prompt "hang" keeps it alive until it is killed.
  const stub = path.join(dir, "stub.js")
  await fs.writeFile(stub, `if (process.argv[2] === "export") process.exit(0); setTimeout(() => {}, 60000)`)
  Jobs.liveDefaults.gizziArgv = [process.execPath, stub]
  try {
    const est = await (await json("/runs/estimate", { suite: name, k: 1, model: "acme/big" })).json()
    expect(est).toMatchObject({ cases: 1, k: 1, attempts: 1 })
    const started = await json("/runs", { suite: name, mode: "live", k: 1, model: "acme/big", estimate_id: est.estimate_id })
    expect(started.status).toBe(202)
    const { run_id } = await started.json()
    const cancelled = await json(`/runs/${run_id}/cancel`)
    expect(cancelled.status).toBe(202)
    await Jobs.wait(run_id)
    const got = await (await app.request(`/runs/${run_id}`)).json()
    expect(got.state).toBe("cancelled")
    expect(got.report).toBeUndefined()
  } finally {
    delete Jobs.liveDefaults.gizziArgv
    await fs.rm(dir, { recursive: true, force: true })
  }
})
