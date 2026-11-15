import { describe, expect, test } from "bun:test"
import {
  isPaidAllternitPlan,
  pickDefaultBrain,
  resolveDefaultBrain,
} from "../../src/runtime/providers/default-brain"
import type { DiscoveredProvider } from "../../src/runtime/providers/discovery"

const FREE = { id: "free", label: "Free", plan_tier: "free", status: "none" }
const PLUS = { id: "plus", label: "Plus", plan_tier: "pro", status: "active" }

// Catalog-shaped rows: vendor order (rank) with the vendor's default flagged.
const claude: DiscoveredProvider = {
  id: "claude-cli",
  name: "Claude Code",
  auth_type: "subprocess",
  source: "subprocess",
  signedIn: true,
  defaultRank: 1,
  models: [
    { id: "claude-fable-5-2", name: "Claude Fable 5.2", rank: 0 },
    { id: "claude-opus-5-5", name: "Claude Opus 5.5", rank: 1 },
    { id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5", rank: 2, default: true },
    { id: "claude-haiku-5-5", name: "Claude Haiku 5.5", rank: 3 },
  ],
}

const kimi: DiscoveredProvider = {
  id: "kimi-cli",
  name: "Kimi Code",
  auth_type: "subprocess",
  source: "subprocess",
  defaultRank: 3,
  models: [
    { id: "kimi-for-coding", name: "Kimi for Coding", rank: 0, default: true },
    { id: "kimi-k3", name: "Kimi K3", rank: 1 },
  ],
}

const grokSignedOut: DiscoveredProvider = {
  id: "grok",
  name: "Grok",
  auth_type: "subprocess",
  source: "subprocess",
  signedIn: false,
  defaultRank: 5,
  models: [{ id: "grok-4.6", name: "Grok 4.6", rank: 0, default: true }],
}

const cursorUnranked: DiscoveredProvider = {
  id: "cursor-agent",
  name: "Cursor Agent",
  auth_type: "subprocess",
  source: "subprocess",
  signedIn: true,
  models: [{ id: "default", name: "Cursor default" }],
}

const cloud: DiscoveredProvider = {
  id: "allternit",
  name: "Allternit Cloud",
  auth_type: "api_key",
  source: "platform",
  models: [
    { id: "llama-3.1-8b", name: "Llama 3.1 8B", rank: 0 },
    { id: "allternit-auto", name: "Allternit Auto", rank: 1, default: true },
    { id: "allternit-legacy", name: "Allternit Legacy", rank: 2, hidden: true },
  ],
}

describe("isPaidAllternitPlan", () => {
  test("Plus/Super/Ultra active or trialing are paid", () => {
    expect(isPaidAllternitPlan(PLUS)).toBe(true)
    expect(isPaidAllternitPlan({ id: "super", label: "Super", plan_tier: "team", status: "trialing" })).toBe(true)
    expect(isPaidAllternitPlan({ id: "ultra", label: "Ultra", plan_tier: "team", status: "active" })).toBe(true)
  })

  test("Free and canceled are not paid", () => {
    expect(isPaidAllternitPlan(FREE)).toBe(false)
    expect(isPaidAllternitPlan({ id: "plus", label: "Plus", plan_tier: "pro", status: "canceled" })).toBe(false)
    expect(isPaidAllternitPlan(null)).toBe(false)
  })
})

describe("resolveDefaultBrain", () => {
  test("owner's machine: model_auto false + dead nail-35 local model → signed-in Claude's default", () => {
    const brain = resolveDefaultBrain({
      modelAuto: false,
      currentModel: "omlx/Nail-35b-a3b",
      plan: null,
      providers: [grokSignedOut, kimi, claude],
    })
    expect(brain).toMatchObject({ model: "claude-cli/claude-sonnet-5-5", pinned: false })
  })

  test("model_auto false with no model is not a pin", () => {
    const brain = resolveDefaultBrain({ modelAuto: false, plan: FREE, providers: [claude, kimi] })
    expect(brain).toMatchObject({ model: "claude-cli/claude-sonnet-5-5", pinned: false })
  })

  test("a pin on a retired id maps to the current id and stays pinned", () => {
    const brain = resolveDefaultBrain({
      modelAuto: false,
      currentModel: "claude-cli/claude-fable-5-1",
      plan: PLUS,
      providers: [cloud, claude],
    })
    expect(brain).toEqual({ model: "claude-cli/claude-fable-5-2", reason: "pinned", pinned: true })
  })

  test("paid picks the Cloud's flagged model, not its first", () => {
    const brain = resolveDefaultBrain({
      currentModel: "claude-cli/claude-sonnet-5-5",
      modelAuto: true,
      plan: PLUS,
      providers: [cloud, claude],
    })
    expect(brain).toMatchObject({ model: "allternit/allternit-auto", pinned: false })
  })

  test("a hidden model is never picked automatically", () => {
    const onlyHidden: DiscoveredProvider = {
      ...cloud,
      models: [
        { id: "allternit-legacy", name: "Legacy", rank: 0, hidden: true, default: true },
        { id: "llama-3.1-8b", name: "Llama 3.1 8B", rank: 1 },
      ],
    }
    expect(resolveDefaultBrain({ plan: PLUS, providers: [onlyHidden] }).model).toBe("allternit/llama-3.1-8b")
  })

  test("a pin on a hidden model is still honoured", () => {
    const brain = resolveDefaultBrain({
      modelAuto: false,
      currentModel: "allternit/allternit-legacy",
      plan: PLUS,
      providers: [cloud],
    })
    expect(brain).toMatchObject({ model: "allternit/allternit-legacy", pinned: true })
  })

  test("unknown sign-in is the fallback; signed-out and unranked CLIs are never picked", () => {
    expect(resolveDefaultBrain({ plan: null, providers: [cursorUnranked, grokSignedOut, kimi] }).model).toBe(
      "kimi-cli/kimi-for-coding",
    )
  })

  test("nothing usable → null with a reason", () => {
    const brain = resolveDefaultBrain({ plan: FREE, providers: [cursorUnranked, grokSignedOut] })
    expect(brain.model).toBeNull()
    expect(brain.pinned).toBe(false)
    expect(brain.reason.length).toBeGreaterThan(0)
  })

  test("unpaid keeps an earlier automatic CLI pick (retired id mapped)", () => {
    const brain = resolveDefaultBrain({
      modelAuto: true,
      currentModel: "kimi-cli/kimi-k2.7-code",
      plan: null,
      providers: [claude, kimi],
    })
    expect(brain).toMatchObject({ model: "kimi-cli/kimi-for-coding", pinned: false })
  })

  test("an automatic pick on a signed-out CLI is replaced", () => {
    const brain = resolveDefaultBrain({
      modelAuto: true,
      currentModel: "grok/grok-4.6",
      plan: null,
      providers: [grokSignedOut, claude],
    })
    expect(brain.model).toBe("claude-cli/claude-sonnet-5-5")
  })
})

describe("pickDefaultBrain", () => {
  test("a pinned /model is left alone even after a sub is bought", () => {
    expect(
      pickDefaultBrain({
        modelAuto: false,
        currentModel: "kimi-cli/kimi-k3",
        plan: { id: "ultra", label: "Ultra", plan_tier: "team", status: "active" },
        providers: [cloud, claude, kimi],
      }),
    ).toBeNull()
  })

  test("the dead state (model_auto false, no model) gets a model to persist", () => {
    expect(pickDefaultBrain({ modelAuto: false, plan: null, providers: [claude] })).toBe("claude-cli/claude-sonnet-5-5")
  })

  test("no-ops when the target is already selected", () => {
    expect(
      pickDefaultBrain({
        modelAuto: true,
        currentModel: "allternit/allternit-auto",
        plan: { id: "super", label: "Super", plan_tier: "team", status: "active" },
        providers: [cloud],
      }),
    ).toBeNull()
  })
})
