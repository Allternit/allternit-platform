import { afterAll, afterEach, beforeAll, describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionPause } from "../../src/runtime/session/pause"
import { CloudContinue } from "../../src/runtime/session/cloud-continue"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

const CATALOG: CloudContinue.CatalogModel[] = [
  { id: "claude-sonnet-4.6", name: "Claude Sonnet 4.6", upstream_id: "anthropic/claude-sonnet-4.6", aliases: ["claude-sonnet"] },
  { id: "kimi-k3", name: "Kimi K3", upstream_id: "moonshotai/Kimi-K3", lane: "standard" },
  { id: "glm-5.3-open", name: "GLM 5.3 (open)", upstream_id: "zai-org/GLM-5.3", lane: "open" },
  { id: "deepseek-v4.1-flash", name: "DeepSeek V4.1 Flash", upstream_id: "deepseek/deepseek-v4.1-flash", lane: "standard" },
]

const prevToken = process.env.ALLTERNIT_API_TOKEN
const prevUrl = process.env.ALLTERNIT_API_URL
// Background reads (the offer pause() makes on its own) never reach the network.
beforeAll(() => {
  process.env.ALLTERNIT_API_URL = "http://127.0.0.1:9"
})
afterAll(() => {
  if (prevUrl === undefined) delete process.env.ALLTERNIT_API_URL
  else process.env.ALLTERNIT_API_URL = prevUrl
})
afterEach(() => {
  SessionPause.clearTimers()
  if (prevToken === undefined) delete process.env.ALLTERNIT_API_TOKEN
  else process.env.ALLTERNIT_API_TOKEN = prevToken
})

describe("choosing the Cloud model", () => {
  test("the same model when Cloud lists it, by id, alias or upstream id", () => {
    expect(CloudContinue.pickModel(CATALOG, "claude-sonnet-4-6")).toEqual({ modelID: "claude-sonnet-4.6", label: "Claude Sonnet 4.6" })
    expect(CloudContinue.pickModel(CATALOG, "claude-sonnet-4-6-20260215")?.modelID).toBe("claude-sonnet-4.6")
    expect(CloudContinue.pickModel(CATALOG, "claude-sonnet")?.modelID).toBe("claude-sonnet-4.6")
    expect(CloudContinue.pickModel(CATALOG, "kimi-k3")?.modelID).toBe("kimi-k3")
  })

  test("else the Cloud default; never an open-lane model", () => {
    expect(CloudContinue.pickModel(CATALOG, "gpt-5.5")?.modelID).toBe("deepseek-v4.1-flash")
    expect(CloudContinue.pickModel(CATALOG, "glm-5.3")?.modelID).toBe("deepseek-v4.1-flash")
    expect(CloudContinue.pickModel(CATALOG)?.modelID).toBe("deepseek-v4.1-flash")
    expect(CloudContinue.pickModel(CATALOG.filter((m) => m.id !== "deepseek-v4.1-flash"), "gpt-5.5")).toBeUndefined()
  })

  // 2026-11-13: the old fixed fallback (DeepSeek V4 Pro) left the list when
  // its upstream ran out of money, so the offer vanished. The catalog's own
  // default (it moves to a funded model) wins; a hidden row is never offered.
  test("the model the catalog flags as default wins over the fixed fallback", () => {
    const flagged = CATALOG.map((m) => (m.id === "kimi-k3" ? { ...m, default: true } : m))
    expect(CloudContinue.pickModel(flagged, "gpt-5.5")?.modelID).toBe("kimi-k3")
    const hiddenDefault = CATALOG.map((m) => (m.id === "kimi-k3" ? { ...m, default: true, hidden: true } : m))
    expect(CloudContinue.pickModel(hiddenDefault, "gpt-5.5")?.modelID).toBe("deepseek-v4.1-flash")
  })

  test("funds: balance, then plan grant, else none; Free includes no Cloud usage", () => {
    expect(CloudContinue.fundsFrom({ balance_usd: 4.2 }, { plan: "plus", weeklyLimit: 20, weeklyUsed: 5 })).toEqual({ kind: "balance", usd: 4.2 })
    expect(CloudContinue.fundsFrom({ balance_usd: 0 }, { plan: "plus", weeklyLimit: 20, weeklyUsed: 5 })).toEqual({ kind: "plan", usd: 15 })
    // 2026-11-15: the $2 free allowance is gone; an old server's free_inference no longer counts.
    expect(CloudContinue.fundsFrom({ balance_usd: 0, free_inference: { remaining_usd: 1.5 } }, { plan: "free", weeklyLimit: 2, weeklyUsed: 0 })).toEqual({ kind: "none" })
    expect(CloudContinue.fundsLabel({ kind: "none" })).toBe("No Cloud credits")
  })

  test("cloud_models says not allowed: nothing to spend, with the server's reason", () => {
    const credits = { balance_usd: 0, cloud_models: { allowed: false, reason: "credits_exhausted", balance_usd: 0, byok_providers: [] } }
    expect(CloudContinue.fundsFrom(credits, { plan: "plus", weeklyLimit: 20, weeklyUsed: 5 })).toEqual({ kind: "none", reason: "credits_exhausted" })
    expect(CloudContinue.fundsLabel({ kind: "none", reason: "credits_exhausted" })).toBe("Cloud credits used up")
  })
})

describe("Cloud entitlement (cloud_models)", () => {
  const locked = { cloud_models: { allowed: false, reason: "credits_required", balance_usd: 0, byok_providers: ["openrouter"] } }

  test("absent means not locked; a malformed field is ignored", () => {
    expect(CloudContinue.entitlementFrom({ balance_usd: 0 })).toBeUndefined()
    expect(CloudContinue.entitlementFrom({ cloud_models: { reason: "credits_required" } })).toBeUndefined()
    expect(CloudContinue.isLocked(undefined)).toBe(false)
    expect(CloudContinue.entitlementFrom(locked)).toEqual({ allowed: false, reason: "credits_required", balanceUsd: 0, byokProviders: ["openrouter"] })
  })

  test("a row served by a provider the account has its own key for is not locked", async () => {
    const ent = CloudContinue.entitlementFrom(locked)!
    expect(CloudContinue.isLocked(ent, { provider: "openrouter" })).toBe(false)
    expect(CloudContinue.isLocked(ent, { provider: "fireworks" })).toBe(true)
    expect(CloudContinue.isLocked(ent)).toBe(true)
    const catalog = async () => Response.json({ data: [{ id: "kimi-k3", provider: "openrouter" }, { id: "glm-5.3", provider: "fireworks" }] })
    const check = await CloudContinue.lockCheck(ent, catalog)
    expect([check("kimi-k3"), check("glm-5.3"), check("unknown")]).toEqual([false, true, true])
  })

  test("no keys: every row locked without reading the catalog; allowed: none locked", async () => {
    const none = CloudContinue.entitlementFrom({ cloud_models: { allowed: false, reason: "credits_required", byok_providers: [] } })
    const noFetch = async () => {
      throw new Error("catalog must not be read")
    }
    expect((await CloudContinue.lockCheck(none, noFetch))("kimi-k3")).toBe(true)
    const ok = CloudContinue.entitlementFrom({ cloud_models: { allowed: true, reason: null, balance_usd: 3, byok_providers: [] } })
    expect((await CloudContinue.lockCheck(ok, noFetch))("kimi-k3")).toBe(false)
  })
})

function fakeCloud(credits: unknown, opts: { status?: number } = {}) {
  return async (url: string) => {
    const path = new URL(url).pathname
    if (opts.status) return new Response("{}", { status: opts.status })
    if (path === "/v1/models") return Response.json({ data: CATALOG })
    if (path === "/api/v1/billing/credits") return Response.json(credits)
    if (path === "/api/v1/me/usage") return Response.json({ plan: "free", weeklyLimit: 0, weeklyUsed: 0 })
    return new Response("{}", { status: 404 })
  }
}

async function turn(sessionID: string, role: "user" | "assistant", text: string) {
  const id = Identifier.ascending("message")
  const base = { id, sessionID, time: { created: Date.now() } }
  await Session.updateMessage(
    role === "user"
      ? { ...base, role, agent: "build", model: { providerID: "codex-cli", modelID: "gpt-5.5" } }
      : ({
          ...base,
          role,
          parentID: id,
          mode: "build",
          agent: "build",
          path: { cwd: "/", root: "/" },
          cost: 0,
          tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
          modelID: "gpt-5.5",
          providerID: "codex-cli",
          finish: "stop",
        } as MessageV2.Assistant),
  )
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text })
}

describe("keep going on Allternit Cloud", () => {
  test("signed out: no offer", async () => {
    delete process.env.ALLTERNIT_API_TOKEN
    delete process.env.ALLTERNIT_API_KEY
    expect(await CloudContinue.offer("gpt-5.5", fakeCloud({ balance_usd: 5 }))).toBeUndefined()
    process.env.ALLTERNIT_API_TOKEN = "tok"
    expect(await CloudContinue.offer("gpt-5.5", fakeCloud({}, { status: 401 }))).toBeUndefined()
  })

  test("no funds: nothing switches; with funds: switches to the Cloud model and continues with a note", async () => {
    process.env.ALLTERNIT_API_TOKEN = "tok"
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const s = await Session.create({ title: "Pricing" })
        await turn(s.id, "user", "Keep going on the sheet")
        await turn(s.id, "assistant", "You've hit your usage limit. Try again in 2 hours.")
        SessionPause.pause(s.id, { until: Date.now() + 7_200_000, limit: "Codex 5-hour limit", reason: "limit_hit", providerID: "codex-cli" })

        const broke = await SessionPause.continueOnCloud(s.id, { fetch: fakeCloud({ balance_usd: 0 }) })
        expect(broke).toMatchObject({ resumed: false, reason: "no_funds", cloud: { modelID: "deepseek-v4.1-flash", funds: { kind: "none" } } })
        expect((await Session.get(s.id)).paused?.cloud?.funds.kind).toBe("none")

        // The Cloud turn itself (SessionPrompt) isn't run here; the switch is what matters.
        const resumed: Array<[string, Parameters<typeof SessionPause.resume>[1]]> = []
        const ok = await SessionPause.continueOnCloud(s.id, {
          fetch: fakeCloud({ balance_usd: 4.2 }),
          resume: async (id, opts) => void resumed.push([id, opts]),
        })
        expect(ok).toMatchObject({ resumed: true, cloud: { modelID: "deepseek-v4.1-flash", label: "DeepSeek V4.1 Flash", funds: { kind: "balance", usd: 4.2 } } })
        expect(resumed).toHaveLength(1)
        const [id, opts] = resumed[0]
        expect(id).toBe(s.id)
        expect(opts?.model).toEqual({ providerID: "allternit", modelID: "deepseek-v4.1-flash" })
        expect(opts?.note?.cloudContinue).toMatchObject({ modelID: "deepseek-v4.1-flash", limit: "Codex 5-hour limit", from: "codex-cli" })

        // What the resumed turn sends: the visible note, then the synthetic carry-on.
        const parts = SessionPause.continueParts({ note: opts!.note, early: true, model: opts!.model! })
        expect(parts[0]).toMatchObject({
          text: "Continued on Allternit Cloud (DeepSeek V4.1 Flash) after the Codex 5-hour limit was reached.",
          metadata: { cloudContinue: { modelID: "deepseek-v4.1-flash" } },
        })
        expect(parts[0]).not.toHaveProperty("synthetic")
        expect(parts[1]).toMatchObject({ synthetic: true, text: "Carry on where you left off; don't repeat finished work." })
        expect(SessionPause.continueParts({ early: false, model: opts!.model! })).toHaveLength(1)
        await Session.remove(s.id)
      },
    })
  })

  test("a pause on Cloud itself offers nothing", async () => {
    process.env.ALLTERNIT_API_TOKEN = "tok"
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const s = await Session.create({ title: "Cloud" })
        await turn(s.id, "user", "Go")
        SessionPause.pause(s.id, { until: Date.now() + 60_000_000, limit: "Allternit Cloud monthly limit", reason: "quota", providerID: "allternit" })
        expect(await SessionPause.continueOnCloud(s.id, { fetch: fakeCloud({ balance_usd: 5 }) })).toEqual({ resumed: false, reason: "not_applicable" })
        await Session.remove(s.id)
      },
    })
  })
})
