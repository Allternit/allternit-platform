// @ts-nocheck
import { test, expect, beforeAll, afterAll, afterEach } from "bun:test"
import fs from "fs/promises"
import path from "path"
import os from "os"
import { estimate, getEstimate, ESTIMATE_TTL_MS } from "../../src/runtime/eval/estimate"
import * as Jobs from "../../src/runtime/eval/jobs"

let root: string
let stub: string

// A stand-in for `gizzi`: prints a session id, and `export` reports a real charge of $0.25 for 15 tokens.
const STUB = `
const a = process.argv.slice(2)
if (a[0] === "export") { console.log(JSON.stringify({ messages: [{ info: { role: "assistant", cost: 0.25, tokens: { input: 10, output: 5 } } }] })); process.exit(0) }
console.log(JSON.stringify({ type: "text", sessionID: "ses_fake", part: { text: "done" } }))
`

const traj = (tokens: number, latency_ms: number) => ({ status: "completed", steps: [], effects: [], cost_usd: 0, tokens, latency_ms })

function caseJson(id: string, recorded?: ReturnType<typeof traj>) {
  return {
    schema: "allternit.gizzi_eval_case.v1",
    id,
    suite: "est",
    title: id,
    label: "good",
    origin: { session_id: "x", through_message_id: null, saved_at: 0, gizzi_version: "0", surface: "cli" },
    turns: [{ role: "user", text: "hi" }],
    workspace: { kind: "none", reason: "no_project" },
    checks: [],
    graders: [{ type: "status", equals: "completed" }],
    redaction: { secrets: 0, paths: 0, entries_dropped: 0 },
    ...(recorded ? { recorded: { trajectory: recorded, grades: [], passed: true } } : {}),
  }
}

async function makeSuite(name: string, cases: [string, ReturnType<typeof traj> | undefined][]) {
  const dir = path.join(root, name)
  for (const [id, rec] of cases) {
    await fs.mkdir(path.join(dir, "cases", id), { recursive: true })
    await fs.writeFile(path.join(dir, "cases", id, "case.json"), JSON.stringify(caseJson(id, rec)))
  }
  await fs.writeFile(
    path.join(dir, "suite.json"),
    JSON.stringify({
      schema: "allternit.eval_suite.v1",
      id: name,
      title: name,
      runner: "gizzi",
      cases: cases.map(([id]) => ({ id, source: { kind: "session", path: `cases/${id}/case.json` } })),
    }),
  )
  return dir
}

beforeAll(async () => {
  root = await fs.mkdtemp(path.join(os.tmpdir(), "estimate-test-"))
  stub = path.join(root, "stub.js")
  await fs.writeFile(stub, STUB)
  Jobs.liveDefaults.gizziArgv = [process.execPath, stub]
  Jobs.liveDefaults.trajectoryFor = async () => traj(0, 5)
})
afterEach(() => {
  Jobs.liveDefaults.timeoutMs = undefined
})
afterAll(async () => {
  for (const k of Object.keys(Jobs.liveDefaults)) delete Jobs.liveDefaults[k]
  await fs.rm(root, { recursive: true, force: true })
})

const price = async () => ({ input: 3, output: 15 })
const noPrice = async () => undefined
const none = async () => undefined

test("a plan login: tokens from recordings, suite median for the rest, equal API price shown", async () => {
  const dir = await makeSuite("plan", [
    ["case_a", traj(10_000, 120_000)],
    ["case_b", traj(30_000, 360_000)],
    ["case_c", undefined],
  ])
  const e = await estimate(
    { suiteDir: dir, k: 2, model: "acme/big" },
    { authType: async () => "oauth", providerSource: none, price, defaultModel: none },
  )
  expect(e).toMatchObject({ cases: 3, k: 2, attempts: 6, billing: "plan", probe_required: false })
  expect(e.tokens_est).toBe(2 * (10_000 + 30_000 + 20_000)) // case_c gets the median, 20k
  expect(e.minutes_est).toBe((2 + 6 + 4) * 2) // case_c has no recording: 4 minutes
  expect(e.usd_est).toBeCloseTo((120_000 * (0.7 * 3 + 0.3 * 15)) / 1_000_000, 4)
  expect(e.estimate_id).toMatch(/^est_/)
})

test("an API key: metered, with a price the dollars are shown and no probe is needed", async () => {
  const dir = await makeSuite("metered", [["case_a", undefined]])
  const e = await estimate({ suiteDir: dir, model: "acme/big" }, { authType: async () => "api", providerSource: none, price, defaultModel: none })
  expect(e.billing).toBe("metered")
  expect(e.tokens_est).toBe(20_000) // nothing recorded anywhere
  expect(e.usd_est).toBeCloseTo(20_000 * 6.6 / 1_000_000, 4)
  expect(e.probe_required).toBe(false)
})

test("metered with no price in the table: no dollars, probe required", async () => {
  const dir = await makeSuite("noprice", [["case_a", undefined]])
  const e = await estimate({ suiteDir: dir, model: "acme/mystery" }, { authType: none, providerSource: async () => "env", price: noPrice, defaultModel: none })
  expect(e.billing).toBe("metered")
  expect(e.usd_est).toBeUndefined()
  expect(e.probe_required).toBe(true)
})

test("no login on record: unknown, and no probe", async () => {
  const dir = await makeSuite("unknown", [["case_a", undefined]])
  const e = await estimate({ suiteDir: dir, model: "acme/big" }, { authType: none, providerSource: none, price, defaultModel: none })
  expect(e.billing).toBe("unknown")
  expect(e.usd_est).toBeUndefined()
  expect(e.probe_required).toBe(false)
})

test("an estimate expires after 10 minutes", async () => {
  const dir = await makeSuite("expiry", [["case_a", undefined]])
  let t = 1_000_000
  const e = await estimate({ suiteDir: dir, model: "acme/big" }, { now: () => t, authType: none, providerSource: none, price, defaultModel: none })
  expect(e.expires_at).toBe(t + ESTIMATE_TTL_MS)
  expect(getEstimate(e.estimate_id, t + ESTIMATE_TTL_MS - 1)).toBeDefined()
  expect(getEstimate(e.estimate_id, t + ESTIMATE_TTL_MS)).toBeUndefined()
  // A live run started with the expired id is refused.
  await expect(Jobs.start({ suiteDir: dir, model: "acme/big", estimateId: e.estimate_id, now: () => t + ESTIMATE_TTL_MS + 1 })).rejects.toMatchObject({ code: "estimate_required" })
})

test("an estimate for other cases or another k is rejected", async () => {
  const dir = await makeSuite("mismatch", [["case_a", undefined], ["case_b", undefined]])
  const deps = { authType: none, providerSource: none, price, defaultModel: none }
  const one = await estimate({ suiteDir: dir, caseIds: ["case_a"], k: 1, model: "acme/big" }, deps)
  await expect(Jobs.start({ suiteDir: dir, k: 1, model: "acme/big", estimateId: one.estimate_id })).rejects.toMatchObject({ code: "estimate_required" })
  await expect(Jobs.start({ suiteDir: dir, caseIds: ["case_a"], k: 3, model: "acme/big", estimateId: one.estimate_id })).rejects.toMatchObject({ code: "estimate_required" })
  await expect(Jobs.start({ suiteDir: dir, caseIds: ["case_a"], k: 1, model: "acme/big" })).rejects.toMatchObject({ code: "estimate_required" })
})

test("a probe that costs far more than estimated stops the run with price_check", async () => {
  // Recorded 5 tokens, but the stub's first attempt really used 15 tokens for $0.25.
  const dir = await makeSuite("probe", [["case_a", traj(5, 60_000)], ["case_b", traj(5, 60_000)]])
  const e = await estimate({ suiteDir: dir, model: "acme/mystery" }, { authType: none, providerSource: async () => "env", price: noPrice, defaultModel: none })
  expect(e.probe_required).toBe(true)
  const { run_id } = await Jobs.start({ suiteDir: dir, model: "acme/mystery", estimateId: e.estimate_id })
  await Jobs.wait(run_id)
  const v = await Jobs.get(run_id)
  expect(v.state).toBe("failed")
  expect(v.error).toBe("price_check")
  expect(v.progress.done).toBe(1) // only the probe ran
})

test("a live job runs every case and writes a live report", async () => {
  const dir = await makeSuite("live", [["case_a", undefined], ["case_b", undefined]])
  const e = await estimate({ suiteDir: dir, k: 2, model: "acme/big" }, { authType: async () => "oauth", providerSource: none, price, defaultModel: none })
  const { run_id } = await Jobs.start({ suiteDir: dir, k: 2, model: "acme/big", estimateId: e.estimate_id })
  await Jobs.wait(run_id)
  const v = await Jobs.get(run_id)
  expect(v.state).toBe("done")
  expect(v.progress).toEqual({ done: 4, total: 4 })
  expect(v.report.extra).toMatchObject({ mode: "live", brain: "acme/big" })
  expect(v.report.extra.per_case).toHaveLength(2)
  expect(v.report.extra.per_case[0]).toMatchObject({ attempts: 2, passes: 2, pass: true, cost: 0.5, tokens: 30 })
})
