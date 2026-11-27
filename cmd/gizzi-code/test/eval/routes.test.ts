// @ts-nocheck
import { test, expect, afterEach } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionTrace } from "../../src/runtime/session/trace"
import { Identifier } from "../../src/shared/id/id"
import { EvalStore } from "../../src/runtime/eval/store"
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
