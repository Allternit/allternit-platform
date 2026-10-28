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
  { id: "deepseek-v4-pro", name: "DeepSeek V4 Pro", upstream_id: "deepseek-ai/DeepSeek-V4-Pro-0813", lane: "standard" },
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
    expect(CloudContinue.pickModel(CATALOG, "gpt-5.5")?.modelID).toBe("deepseek-v4-pro")
    expect(CloudContinue.pickModel(CATALOG, "glm-5.3")?.modelID).toBe("deepseek-v4-pro")
    expect(CloudContinue.pickModel(CATALOG)?.modelID).toBe("deepseek-v4-pro")
    expect(CloudContinue.pickModel(CATALOG.filter((m) => m.id !== "deepseek-v4-pro"), "gpt-5.5")).toBeUndefined()
  })

  test("funds: balance, then plan grant, then free allowance, else none", () => {
    expect(CloudContinue.fundsFrom({ balance_usd: 4.2 }, { plan: "plus", weeklyLimit: 20, weeklyUsed: 5 })).toEqual({ kind: "balance", usd: 4.2 })
    expect(CloudContinue.fundsFrom({ balance_usd: 0 }, { plan: "plus", weeklyLimit: 20, weeklyUsed: 5 })).toEqual({ kind: "plan", usd: 15 })
    expect(CloudContinue.fundsFrom({ balance_usd: 0, free_inference: { remaining_usd: 1.5 } }, { plan: "free" })).toEqual({ kind: "free", usd: 1.5 })
    expect(CloudContinue.fundsFrom({ balance_usd: 0, free_inference: { remaining_usd: 0 } }, { plan: "free", weeklyLimit: 2, weeklyUsed: 2 })).toEqual({ kind: "none" })
    expect(CloudContinue.fundsLabel({ kind: "free", usd: 1.5 })).toBe("$1.50 free left this month")
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

        const broke = await SessionPause.continueOnCloud(s.id, { fetch: fakeCloud({ balance_usd: 0, free_inference: { remaining_usd: 0 } }) })
        expect(broke).toMatchObject({ resumed: false, reason: "no_funds", cloud: { modelID: "deepseek-v4-pro", funds: { kind: "none" } } })
        expect((await Session.get(s.id)).paused?.cloud?.funds.kind).toBe("none")

        // The Cloud turn itself (SessionPrompt) isn't run here; the switch is what matters.
        const resumed: Array<[string, Parameters<typeof SessionPause.resume>[1]]> = []
        const ok = await SessionPause.continueOnCloud(s.id, {
          fetch: fakeCloud({ balance_usd: 4.2 }),
          resume: async (id, opts) => void resumed.push([id, opts]),
        })
        expect(ok).toMatchObject({ resumed: true, cloud: { modelID: "deepseek-v4-pro", label: "DeepSeek V4 Pro", funds: { kind: "balance", usd: 4.2 } } })
        expect(resumed).toHaveLength(1)
        const [id, opts] = resumed[0]
        expect(id).toBe(s.id)
        expect(opts?.model).toEqual({ providerID: "allternit", modelID: "deepseek-v4-pro" })
        expect(opts?.note?.cloudContinue).toMatchObject({ modelID: "deepseek-v4-pro", limit: "Codex 5-hour limit", from: "codex-cli" })

        // What the resumed turn sends: the visible note, then the synthetic carry-on.
        const parts = SessionPause.continueParts({ note: opts!.note, early: true, model: opts!.model! })
        expect(parts[0]).toMatchObject({
          text: "Continued on Allternit Cloud (DeepSeek V4 Pro) after the Codex 5-hour limit was reached.",
          metadata: { cloudContinue: { modelID: "deepseek-v4-pro" } },
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
