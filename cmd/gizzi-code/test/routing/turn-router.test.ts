import { afterEach, describe, expect, test } from "bun:test"
import * as TR from "../../src/runtime/routing/turn-router"
import { buildModelPool, type ProviderModelView } from "../../src/runtime/model-pool/pool"
import {
  callTypeClass,
  defaultMaxOutputTokens,
  genClassOf,
  GEN_DEEP,
  GEN_SMALL,
  GEN_STANDARD,
  SMALL_COST_MAX,
} from "../../src/runtime/model-pool/classes"

const view = (modelID: string, costIn: number, costOut: number, reasoning = false): ProviderModelView => ({
  providerID: "p", modelID, url: "https://api.example.test/v1", status: "active", reasoning, toolcall: true,
  imageIn: false, textOut: true, context: 100000, costIn, costOut,
})
const POOL = buildModelPool([view("cheap", 0.5, 1.5), view("mid", 3, 9), view("deep", 10, 40, true)])
const by = (m: string) => POOL.find((e) => e.extensions["x-model_ref"] === `p/${m}`)!

type Call = { url: string; body: any }
function fake(handlers: { kernel?: (b: any) => Response | Promise<Response>; s1?: (b: any) => Response }) {
  const calls: Call[] = []
  const f = async (url: string, init?: RequestInit) => {
    const body = init?.body ? JSON.parse(String(init.body)) : undefined
    calls.push({ url, body })
    if (url.includes("/kernel/turn-route")) {
      if (!handlers.kernel) throw new Error("ECONNREFUSED")
      return handlers.kernel(body)
    }
    if (url.endsWith("/v1/decision/outcome")) return new Response("{}")
    if (url.endsWith("/v1/decision")) return (handlers.s1 ?? (() => new Response("{}", { status: 503 })))(body)
    throw new Error("unexpected " + url)
  }
  return { calls, f }
}
const env = { ALLTERNIT_API_URL: "http://api.test", ALLTERNIT_API_TOKEN: "t", ALLTERNIT_S1_URL: "http://s1.test" }
const s1Answer = (b: any) => {
  const bank = b.request.decision_bank_id
  const answer = bank === TR.ROUTE_BANK ? "coding" : "gen.small"
  return Response.json({ answer, confidence: 0.6, extensions: { "x-decision_id": `${bank}#1` } })
}
const kernelAnswer = (b: any) =>
  Response.json({ gen_class: "gen.standard", backend_id: by("mid").backend_id, max_output_tokens: null, estimated_cost: by("mid").cost, call_type: b.call_type })
const settle = () => new Promise((r) => setTimeout(r, 20))
const turn = (text = "fix the bug", requested = { providerID: "auto", modelID: "auto" }) => ({
  sessionID: "s1", userMessageID: "u1", text, requested, incumbent: { providerID: "p", modelID: "cheap" },
})

afterEach(() => TR.setDeps(undefined))

describe("capability classes (O1/O5) mirror the kernel table", () => {
  test("pool entries carry gen classes", () => {
    expect(by("cheap").extensions["x-gen_class"]).toBe(GEN_SMALL)
    expect(by("mid").extensions["x-gen_class"]).toBe(GEN_STANDARD)
    expect(by("deep").extensions["x-gen_class"]).toBe(GEN_DEEP)
    expect(SMALL_COST_MAX).toBe(0.0015) // commrails/src/kernel/classes.rs
    expect(genClassOf({ ...by("mid"), extensions: { "x-gen_class": "gen.deep" } })).toBe(GEN_DEEP)
  })
  test("call types and caps", () => {
    for (const ct of ["title", "summary", "compaction", "extraction", "memory_extraction", "lessons"]) expect(callTypeClass(ct)).toBe(GEN_SMALL)
    expect(callTypeClass("answer")).toBe(GEN_STANDARD)
    expect(callTypeClass("plan")).toBe(GEN_DEEP)
    expect(defaultMaxOutputTokens("S2", "title")).toBe(64)
    expect(defaultMaxOutputTokens("S2", "summary")).toBe(1024)
    expect(defaultMaxOutputTokens("S2", "patch")).toBe(8192)
    expect(defaultMaxOutputTokens("S2", "answer")).toBeUndefined()
    expect(defaultMaxOutputTokens("S1")).toBe(0)
    expect(defaultMaxOutputTokens("S0")).toBeUndefined()
  })
})

describe("turn routing shadow (O2/O14)", () => {
  test("asks the kernel and both S1 banks; the incumbent still decides", async () => {
    const { calls, f } = fake({ kernel: kernelAnswer, s1: s1Answer })
    TR.setDeps({ fetch: f as any, pool: async () => POOL, env })
    const r = await TR.startTurn(turn())
    expect(r.model).toEqual({ providerID: "p", modelID: "cheap" })
    await settle()
    const kernel = calls.find((c) => c.url === "http://api.test/api/v1/kernel/turn-route")!
    expect(kernel.body.call_type).toBe("answer")
    expect(kernel.body.entries.length).toBe(3)
    const banks = calls.filter((c) => c.url === "http://s1.test/v1/decision").map((c) => c.body)
    expect(banks.map((b) => b.request.decision_bank_id).sort()).toEqual([TR.ROUTE_BANK, TR.ROUTE_MODEL_BANK])
    for (const b of banks) {
      expect(b.backend).toBe("auto")
      expect(b.request.candidates.at(-1)).toEqual({ candidate_id: "unknown", label: "unknown", is_unknown: true })
    }
    expect(banks.find((b) => b.request.decision_bank_id === TR.ROUTE_BANK).request.candidates.length).toBe(9)
    const rec = TR.current("s1")!
    expect(rec.kernel?.gen_class).toBe("gen.standard")
    expect(rec.kernel?.model_ref).toBe("p/mid")
    expect(rec.incumbentClass).toBe("gen.small")
    expect(rec.incumbentCost).toBe(by("cheap").cost)
    expect(rec.routeDecisionId).toBe(`${TR.ROUTE_BANK}#1`)
    expect(rec.routeModelChoice).toBe("gen.small")
  })

  test("authority=kernel uses the kernel's pick for auto turns", async () => {
    const { f } = fake({ kernel: kernelAnswer, s1: s1Answer })
    TR.setDeps({ fetch: f as any, pool: async () => POOL, env: { ...env, ALLTERNIT_TURN_ROUTE_AUTHORITY: "kernel" } })
    expect((await TR.startTurn(turn())).model).toEqual({ providerID: "p", modelID: "mid" })
    // An explicit model is never overridden.
    expect((await TR.startTurn(turn("x", { providerID: "p", modelID: "cheap" }))).model).toEqual({ providerID: "p", modelID: "cheap" })
  })

  test("kernel unreachable or failing → incumbent stands, reason recorded", async () => {
    for (const kernel of [undefined, () => new Response("no", { status: 500 })]) {
      const { f } = fake({ kernel, s1: s1Answer })
      TR.setDeps({ fetch: f as any, pool: async () => POOL, env: { ...env, ALLTERNIT_TURN_ROUTE_AUTHORITY: "kernel" } })
      const r = await TR.startTurn(turn())
      expect(r.model).toEqual({ providerID: "p", modelID: "cheap" })
      expect(TR.current("s1")!.kernelError).toMatch(/kernel router (unreachable|HTTP 500)/)
    }
    // Not configured at all: same fallback, no call made.
    const { calls, f } = fake({ s1: s1Answer })
    TR.setDeps({ fetch: f as any, pool: async () => POOL, env: { ALLTERNIT_S1_URL: "http://s1.test" } })
    expect((await TR.startTurn(turn())).model.modelID).toBe("cheap")
    expect(calls.some((c) => c.url.includes("turn-route"))).toBe(false)
  })

  test("shadow off → no calls", async () => {
    const { calls, f } = fake({ kernel: kernelAnswer, s1: s1Answer })
    TR.setDeps({ fetch: f as any, pool: async () => POOL, env: { ...env, ALLTERNIT_TURN_ROUTE_SHADOW: "0" } })
    expect((await TR.startTurn(turn())).model.modelID).toBe("cheap")
    expect(calls.length).toBe(0)
  })
})

describe("outcome labels", () => {
  test("ROUTE label from the turn's tools", () => {
    expect(TR.routeLabel([])).toBe("answer_from_memory")
    expect(TR.routeLabel(["read", "grep"])).toBe("retrieval")
    expect(TR.routeLabel(["bash"])).toBe("single_tool")
    expect(TR.routeLabel(["read", "edit"])).toBe("coding")
    expect(TR.routeLabel(["task"])).toBe("agent_run")
    expect(TR.routeLabel(["question"])).toBe("clarify")
    expect(TR.routeLabel(["computer_use"])).toBe("computer_use")
  })

  test("finishTurn posts the ROUTE outcome; the next turn labels ROUTE_MODEL with costs", async () => {
    const { calls, f } = fake({ kernel: kernelAnswer, s1: s1Answer })
    TR.setDeps({ fetch: f as any, pool: async () => POOL, env })
    await TR.startTurn(turn())
    await settle()
    await TR.finishTurn("s1", { tools: ["read", "edit"], errored: false })
    const outcomes = () => calls.filter((c) => c.url === "http://s1.test/v1/decision/outcome").map((c) => c.body)
    expect(outcomes()).toEqual([{ decision_id: `${TR.ROUTE_BANK}#1`, truth: "coding", source: "turn_tools", "x-tools": 2 }])
    // Same text again = a retry → one class up from the incumbent's gen.small.
    await TR.startTurn(turn())
    const rm = outcomes().find((o) => o.decision_id === `${TR.ROUTE_MODEL_BANK}#1`)!
    expect(rm.truth).toBe("gen.standard")
    expect(rm.source).toBe("user_retry")
    expect(rm["x-incumbent_cost"]).toBe(by("cheap").cost)
    expect(rm["x-kernel_cost"]).toBe(by("mid").cost)
  })

  test("a model switch labels the switched-to class; an accepted turn keeps the incumbent's", async () => {
    const { calls, f } = fake({ kernel: kernelAnswer, s1: s1Answer })
    TR.setDeps({ fetch: f as any, pool: async () => POOL, env })
    const outcomes = () => calls.filter((c) => c.url.endsWith("/outcome")).map((c) => c.body)
    await TR.startTurn(turn("a"))
    await settle()
    await TR.startTurn(turn("b", { providerID: "p", modelID: "deep" }))
    expect(outcomes().at(-1)).toMatchObject({ truth: "gen.deep", source: "user_model_switch" })
    await settle()
    await TR.startTurn(turn("c", { providerID: "p", modelID: "deep" }))
    expect(outcomes().at(-1)).toMatchObject({ truth: "gen.small", source: "turn_accepted" })
  })
})
