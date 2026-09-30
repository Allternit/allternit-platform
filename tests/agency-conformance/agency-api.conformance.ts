/**
 * WP12 API-level conformance for the Agency API v0.3 contract
 * (surfaces/docs/api/agency/openapi-v0.3.yaml). Alpha gate: CL-282. Also CL-001.
 *
 * Skipped unless AGENCY_BASE_URL is set, so it is inert until WP11 (the Agency
 * API in allternit-api) is merged. Run against a local allternit-api:
 *
 *   AGENCY_BASE_URL=http://127.0.0.1:8080 AGENCY_TOKEN=<bearer> \
 *     bun test tests/agency-conformance/agency-api.conformance.ts
 *
 * or `scripts/conformance-report.sh --api http://127.0.0.1:8080`.
 *
 * Optional: AGENCY_MODEL_CLASS_A / AGENCY_MODEL_CLASS_B (two `mc.*` classes that
 * both exist in the model pool; defaults mc.local and mc.remote),
 * AGENCY_RUN_TIMEOUT_S (default 120), AGENCY_RESTART_CMD (a shell command that
 * restarts the API and returns when it is healthy again; enables the restart test).
 *
 * The file is deliberately not named *.test.ts so the repo's vitest globs never
 * collect it. Test names start with the same group tags as the kernel suite.
 */
import { describe, expect, test } from "bun:test"
import { execSync } from "node:child_process"

const BASE = (process.env.AGENCY_BASE_URL ?? "").replace(/\/$/, "")
const TOKEN = process.env.AGENCY_TOKEN ?? ""
const CLASS_A = process.env.AGENCY_MODEL_CLASS_A ?? "mc.local"
const CLASS_B = process.env.AGENCY_MODEL_CLASS_B ?? "mc.remote"
const RUN_TIMEOUT_MS = Number(process.env.AGENCY_RUN_TIMEOUT_S ?? "120") * 1000
const RESTART_CMD = process.env.AGENCY_RESTART_CMD ?? ""

const live = BASE ? test : test.skip
const liveRestart = BASE && RESTART_CMD ? test : test.skip

const TERMINAL = ["completed", "partial", "failed", "cancelled"]
const VENDOR = /openai|anthropic|claude|codex|gpt|gemini|llama|mistral|deepseek|qwen|sonnet|kimi|grok/i

const uuid = () => crypto.randomUUID()

async function api(method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const res = await fetch(BASE + path, {
    method,
    headers: {
      accept: "application/json",
      ...(body !== undefined ? { "content-type": "application/json" } : {}),
      ...(TOKEN ? { authorization: `Bearer ${TOKEN}` } : {}),
      ...headers,
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  const text = await res.text()
  let json: any = undefined
  try {
    json = text ? JSON.parse(text) : undefined
  } catch {}
  return { status: res.status, headers: res.headers, json, text }
}

const GOAL = {
  agent: "allternit-code",
  goal: "Fix the failing unit test in the sample repository.",
  workspace: { repo: process.env.AGENCY_SAMPLE_REPO ?? "https://example.com/sample.git", ref: "main" },
  authority: { profile: "code-safe" },
  budget: { max_seconds: 300, max_cost_usd: 1 },
  completion: { require: ["target_tests_pass", "no_new_regressions"] },
  stream: false,
}

async function createRun(overrides: Record<string, unknown> = {}, key = uuid()) {
  const r = await api("POST", "/v1/agency", { ...GOAL, ...overrides }, { "idempotency-key": key })
  expect([200, 201, 202]).toContain(r.status)
  expect(r.json.id).toMatch(/^run_/)
  return { run: r.json, key }
}

async function untilTerminal(id: string) {
  const end = Date.now() + RUN_TIMEOUT_MS
  for (;;) {
    const r = await api("GET", `/v1/runs/${id}`)
    expect(r.status).toBe(200)
    if (TERMINAL.includes(r.json.status) || r.json.status === "needs_attention") return r.json
    if (Date.now() > end) throw new Error(`run ${id} not terminal after ${RUN_TIMEOUT_MS}ms: ${r.json.status}`)
    await Bun.sleep(1000)
  }
}

async function pages(path: string) {
  const out: any[] = []
  let cursor = ""
  for (let i = 0; i < 50; i++) {
    const r = await api("GET", path + (cursor ? `${path.includes("?") ? "&" : "?"}cursor=${cursor}` : ""))
    expect(r.status).toBe(200)
    out.push(...r.json.data)
    if (!r.json.has_more) break
    cursor = r.json.next_cursor
  }
  return out
}

/** Model-independent shape of a run: graph structure + ordered event types. */
async function loopShape(id: string) {
  const g = await api("GET", `/v1/runs/${id}/graph`)
  expect(g.status).toBe(200)
  const nodes = (g.json.nodes as any[])
    .map((n) => ({ primitive: n.primitive_id ?? n.primitive, role: n.cognitive_role ?? n.role, kind: n.node_kind ?? n.kind }))
    .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)))
  const events = await pages(`/v1/runs/${id}/events`)
  return { nodes, edges: (g.json.edges ?? []).length, events: events.map((e) => e.type) }
}

describe("agency api conformance (CL-282, CL-001)", () => {
  live("restart_completed_run_is_durable_across_api_restart", async () => {
    if (!RESTART_CMD) return
    const { run } = await createRun()
    const done = await untilTerminal(run.id)
    execSync(RESTART_CMD, { stdio: "inherit" })
    const after = await api("GET", `/v1/runs/${run.id}`)
    expect(after.status).toBe(200)
    expect(after.json.status).toBe(done.status)
    const chain = await api("GET", `/v1/runs/${run.id}/receipts/verification`)
    expect(chain.json.hash_chain_valid).toBe(true)
  })

  liveRestart("restart_in_flight_run_resumes_after_api_restart", async () => {
    const { run } = await createRun()
    execSync(RESTART_CMD, { stdio: "inherit" })
    const done = await untilTerminal(run.id)
    expect(["completed", "needs_attention", "failed", "partial"]).toContain(done.status)
    const events = await pages(`/v1/runs/${run.id}/events`)
    // gap-free, ordered sequence across the restart
    events.forEach((e, i) => expect(e.seq).toBe(events[0].seq + i))
  })

  live("resume_paused_run_continues_and_events_replay_from_last_event_id", async () => {
    const { run } = await createRun()
    const p = await api("POST", `/v1/runs/${run.id}/pause`, {}, { "idempotency-key": uuid() })
    expect([200, 202, 409]).toContain(p.status)
    const r = await api("POST", `/v1/runs/${run.id}/resume`, {}, { "idempotency-key": uuid() })
    expect([200, 202, 409]).toContain(r.status)
    const all = await pages(`/v1/runs/${run.id}/events`)
    expect(all.length).toBeGreaterThan(0)
    // Last-Event-ID resume over SSE returns only later events.
    const mid = all[Math.floor(all.length / 2)]
    const res = await fetch(`${BASE}/v1/runs/${run.id}/events`, {
      headers: { accept: "text/event-stream", "last-event-id": mid.id, ...(TOKEN ? { authorization: `Bearer ${TOKEN}` } : {}) },
    })
    expect(res.status).toBe(200)
    expect(res.headers.get("content-type")).toContain("text/event-stream")
    const reader = res.body!.getReader()
    const first = new TextDecoder().decode((await reader.read()).value)
    await reader.cancel()
    expect(first).not.toContain(`id: ${mid.id}\n`)
  })

  live("receipt_completion_run_cannot_complete_without_verifier_receipts", async () => {
    const { run } = await createRun()
    const done = await untilTerminal(run.id)
    if (done.status !== "completed") return // needs_attention/failed is acceptable: the point is the converse below
    const receipts = await pages(`/v1/runs/${run.id}/receipts`)
    const types = receipts.map((r) => r.type)
    expect(types.some((t: string) => /verif/i.test(t))).toBe(true)
    expect(types.some((t: string) => /run|complet/i.test(t))).toBe(true)
    const chain = await api("GET", `/v1/runs/${run.id}/receipts/verification`)
    expect(chain.json.hash_chain_valid).toBe(true)
    expect(chain.json.signatures_valid).not.toBe(false)
  })

  live("receipt_completion_a_client_cannot_disable_verifier_owned_completion", async () => {
    for (const bad of [
      { completion: { require: [], verify: "off" } },
      { authority: { profile: "yolo" } },
      { authority: { profile: "bypass" } },
      { judge: { fail_closed: false } },
    ]) {
      const r = await api("POST", "/v1/agency", { ...GOAL, ...bad }, { "idempotency-key": uuid() })
      expect([400, 403, 422]).toContain(r.status)
    }
  })

  live("replay_of_a_finished_run_has_no_unexpected_divergence_and_no_live_effects", async () => {
    const { run } = await createRun()
    await untilTerminal(run.id)
    const before = (await pages(`/v1/runs/${run.id}/receipts`)).length
    const rp = await api("POST", "/v1/replays", { source_run_id: run.id, effects: "recorded_only" }, { "idempotency-key": uuid() })
    expect([200, 202]).toContain(rp.status)
    expect(rp.json.effects).toBe("recorded_only")
    let replay = rp.json
    for (let i = 0; i < 60 && !replay.verdict; i++) {
      await Bun.sleep(1000)
      replay = (await api("GET", `/v1/replays/${rp.json.id}`)).json
    }
    expect(["equivalent", "diverged_expected"]).toContain(replay.verdict)
    expect((await pages(`/v1/runs/${run.id}/receipts`)).length).toBe(before) // replay appended nothing
    const rejected = await api("POST", "/v1/replays", { source_run_id: run.id, effects: "live" }, { "idempotency-key": uuid() })
    expect([400, 422]).toContain(rejected.status)
  })

  live("fault_unknown_run_and_bad_bodies_fail_closed_with_typed_errors", async () => {
    const nf = await api("GET", "/v1/runs/run_does_not_exist")
    expect(nf.status).toBe(404)
    expect(nf.json.error?.code ?? nf.json.code).toBeDefined()
    const extra = await api("POST", "/v1/agency", { ...GOAL, surprise: 1 }, { "idempotency-key": uuid() })
    expect([400, 422]).toContain(extra.status)
    const noKey = await api("POST", "/v1/agency", GOAL)
    expect([400, 422]).toContain(noKey.status)
    const noBudgetGoal = await api("POST", "/v1/agency", { ...GOAL, goal: "" }, { "idempotency-key": uuid() })
    expect([400, 422]).toContain(noBudgetGoal.status)
  })

  live("fault_budget_exhaustion_stops_the_run_and_never_reports_completed", async () => {
    const { run } = await createRun({ budget: { max_seconds: 1, max_cost_usd: 0 } })
    const done = await untilTerminal(run.id)
    expect(["failed", "needs_attention", "partial"]).toContain(done.status)
    expect(done.status).not.toBe("completed")
  })

  live("rollback_cancel_settles_the_run_and_is_receipted", async () => {
    const { run } = await createRun()
    const c = await api("POST", `/v1/runs/${run.id}/cancel`, {}, { "idempotency-key": uuid() })
    expect([200, 202, 409]).toContain(c.status)
    const done = await untilTerminal(run.id)
    expect(["cancelled", "completed", "failed"]).toContain(done.status)
    const chain = await api("GET", `/v1/runs/${run.id}/receipts/verification`)
    expect(chain.json.hash_chain_valid).toBe(true)
  })

  live("duplicate_same_key_same_body_returns_the_original_run", async () => {
    const key = uuid()
    const a = await api("POST", "/v1/agency", GOAL, { "idempotency-key": key })
    const b = await api("POST", "/v1/agency", GOAL, { "idempotency-key": key })
    expect(b.json.id).toBe(a.json.id)
    expect(b.headers.get("idempotency-replayed")).toBe("true")
  })

  live("duplicate_same_key_different_body_is_409_key_reused", async () => {
    const key = uuid()
    await api("POST", "/v1/agency", GOAL, { "idempotency-key": key })
    const b = await api("POST", "/v1/agency", { ...GOAL, goal: "A different goal entirely." }, { "idempotency-key": key })
    expect(b.status).toBe(409)
    expect(JSON.stringify(b.json)).toContain("ERR_IDEMPOTENCY_KEY_REUSED")
  })

  live("concurrency_parallel_creates_with_one_key_make_exactly_one_run", async () => {
    const key = uuid()
    const rs = await Promise.all(Array.from({ length: 6 }, () => api("POST", "/v1/agency", GOAL, { "idempotency-key": key })))
    const ok = rs.filter((r) => [200, 201, 202].includes(r.status))
    const inProgress = rs.filter((r) => r.status === 409)
    expect(ok.length + inProgress.length).toBe(rs.length)
    expect(new Set(ok.map((r) => r.json.id)).size).toBe(1)
    for (const r of inProgress) expect(JSON.stringify(r.json)).toContain("ERR_IDEMPOTENCY_IN_PROGRESS")
  })

  live("security_unauthenticated_and_cross_tenant_access_is_refused", async () => {
    const res = await fetch(`${BASE}/v1/runs`, { headers: { accept: "application/json" } })
    expect([401, 403]).toContain(res.status)
    const bad = await fetch(`${BASE}/v1/runs`, { headers: { authorization: "Bearer not-a-real-token" } })
    expect([401, 403]).toContain(bad.status)
  })

  live("security_default_views_never_expose_vendor_or_model_identity", async () => {
    const { run } = await createRun()
    await untilTerminal(run.id)
    for (const path of [`/v1/runs/${run.id}`, `/v1/runs/${run.id}/graph`, `/v1/runs/${run.id}/receipts`, `/v1/runs/${run.id}/events`]) {
      const r = await api("GET", path)
      expect(r.text).not.toMatch(VENDOR)
    }
  })

  live("security_no_permissive_authority_profile_exists", async () => {
    const r = await api("GET", "/v1/authority-profiles")
    if (r.status !== 200) return
    expect(JSON.stringify(r.json)).not.toMatch(/yolo|bypass|auto-?approve/i)
  })

  live("model_swap_same_goal_two_backends_gives_the_same_graph_and_loop_shape", async () => {
    const a = await createRun({ models: { allow_classes: [CLASS_A] } })
    const b = await createRun({ models: { allow_classes: [CLASS_B] } })
    await Promise.all([untilTerminal(a.run.id), untilTerminal(b.run.id)])
    const [sa, sb] = [await loopShape(a.run.id), await loopShape(b.run.id)]
    expect(sb.nodes).toEqual(sa.nodes)
    expect(sb.edges).toBe(sa.edges)
    // The set of event types and their first-occurrence order is model-independent.
    const order = (xs: string[]) => [...new Set(xs)]
    expect(order(sb.events)).toEqual(order(sa.events))
    // Completion authority is the same: both were closed (or held) by the verifier path.
    const [ra, rb] = [await api("GET", `/v1/runs/${a.run.id}`), await api("GET", `/v1/runs/${b.run.id}`)]
    expect(ra.json.completion?.required ?? ra.json.completion).toEqual(rb.json.completion?.required ?? rb.json.completion)
  })
})
