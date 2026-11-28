// @ts-nocheck
import { test, expect, afterEach } from "bun:test"
import fs from "fs/promises"
import path from "path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionTrace } from "../../src/runtime/session/trace"
import { Identifier } from "../../src/shared/id/id"
import { EvalStore } from "../../src/runtime/eval/store"
import { Case } from "../../src/runtime/eval/schema"
import { tmpdir } from "../fixture/fixture"

const SECRET = "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd"
const created: string[] = []

afterEach(async () => {
  for (const id of created.splice(0)) await EvalStore.deleteCase(id).catch(() => {})
})

async function makeSession(opts: { prompt?: string | null; toolOutput?: string } = {}) {
  const session = await Session.create({ title: "eval store test" })
  const t0 = Date.now()
  if (opts.prompt !== null) {
    const messageID = Identifier.ascending("message")
    await Session.updateMessage({
      id: messageID,
      sessionID: session.id,
      role: "user",
      time: { created: t0 },
      agent: "build",
      model: { providerID: "test", modelID: "test" },
    })
    await Session.updatePart({
      id: Identifier.ascending("part"),
      sessionID: session.id,
      messageID,
      type: "text",
      text: opts.prompt ?? "Fix the failing test",
    })
  }
  const row = (kind: string, data: unknown, time: number) =>
    SessionTrace.append({ sessionID: session.id, kind, data, time })
  row("request.started", { agent: "build" }, t0)
  row(
    "part.updated",
    { type: "tool", callID: "c1", tool: "edit", state: { status: "completed", output: opts.toolOutput ?? "ok" } },
    t0 + 1000,
  )
  row("request.completed", { cost: 0.01, tokens: { input: 100, output: 50 } }, t0 + 2000)
  return session
}

const save = (sessionID: string, label: "good" | "bad", extra = {}) =>
  EvalStore.saveCase({ sessionID, label, surface: "cli", ...extra })

async function inProject<T>(fn: () => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return Instance.provide({ directory: tmp.path, fn })
}

test("a good completed session saves clean and case.json validates", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const r = await save(s.id, "good", { note: "works" })
    created.push(r.case.id)
    expect(r.needs_check).toBe(false)
    expect(r.self_test).toEqual({ passed: true, agrees_with_label: true })
    expect(r.case.title).toBe("Fix the failing test")
    const dir = path.join(EvalStore.root(), "suites", "my-cases", "cases", r.case.id)
    const parsed = Case.parse(JSON.parse(await fs.readFile(path.join(dir, "case.json"), "utf8")))
    expect(parsed.label).toBe("good")
    expect(parsed.recorded.passed).toBe(true)
    expect(parsed.graders.map((g) => g.type)).toEqual(["status", "cost_ceiling", "latency_ceiling"])
    expect((await EvalStore.getCase(r.case.id)).id).toBe(r.case.id)
    const suite = JSON.parse(await fs.readFile(path.join(EvalStore.root(), "suites", "my-cases", "suite.json"), "utf8"))
    expect(suite.runner).toBe("gizzi")
    expect(suite.cases.some((c) => c.id === r.case.id)).toBe(true)
  })
})

test("a bad label on a passing session is marked needs_check", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const r = await save(s.id, "bad", { quickCheck: ["wrong_answer"] })
    created.push(r.case.id)
    expect(r.needs_check).toBe(true)
    expect(r.self_test).toEqual({ passed: true, agrees_with_label: false })
    expect((await EvalStore.getCase(r.case.id)).needs_check).toBe(true)
  })
})

test("a session with no user text throws empty_session", async () => {
  await inProject(async () => {
    const s = await makeSession({ prompt: null })
    await expect(save(s.id, "good")).rejects.toMatchObject({ code: "empty_session" })
  })
})

test("a missing session throws session_not_found", async () => {
  await expect(save("ses_doesnotexist0000000000000", "good")).rejects.toMatchObject({ code: "session_not_found" })
})

test("the 101st case in a suite throws case_limit", async () => {
  await inProject(async () => {
    const suite = "limit-" + Math.random().toString(36).slice(2, 8)
    const cases = path.join(EvalStore.root(), "suites", suite, "cases")
    for (let i = 0; i < 100; i++) await fs.mkdir(path.join(cases, `case_STUB${i}`), { recursive: true })
    const s = await makeSession()
    await expect(save(s.id, "good", { suite })).rejects.toMatchObject({ code: "case_limit" })
    await fs.rm(path.join(EvalStore.root(), "suites", suite), { recursive: true, force: true })
  })
})

test("deleteCase removes the folder and the suite entry", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const r = await save(s.id, "good")
    const dir = path.join(EvalStore.root(), "suites", "my-cases", "cases", r.case.id)
    expect(await fs.stat(dir).then(() => true, () => false)).toBe(true)
    await EvalStore.deleteCase(r.case.id)
    expect(await fs.stat(dir).then(() => true, () => false)).toBe(false)
    const suite = JSON.parse(await fs.readFile(path.join(EvalStore.root(), "suites", "my-cases", "suite.json"), "utf8"))
    expect(suite.cases.some((c) => c.id === r.case.id)).toBe(false)
    await expect(EvalStore.getCase(r.case.id)).rejects.toMatchObject({ code: "case_not_found" })
  })
})

test("a planted secret in tool output is not in trace.json", async () => {
  await inProject(async () => {
    const s = await makeSession({ toolOutput: `token ${SECRET}` })
    const r = await save(s.id, "good")
    created.push(r.case.id)
    const trace = await fs.readFile(
      path.join(EvalStore.root(), "suites", "my-cases", "cases", r.case.id, "trace.json"),
      "utf8",
    )
    expect(trace).not.toContain(SECRET)
    expect(trace).toContain("part.updated")
  })
})
