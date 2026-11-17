import { describe, expect, test } from "bun:test"
import { checkAnswerResponse, selectAnswerPolicy } from "../../src/runtime/integrations/artifacts/answer-policy"
import { ANSWER_WIDGETS_MANUAL } from "../../src/runtime/integrations/artifacts/answer-contract"
import { AnswerTurnGuard, answerWidgetsManual, toolDeliveredCard } from "../../src/runtime/session/answer-policy"
import { LookupTool } from "../../src/runtime/tools/builtins/lookup"

const WEATHER_CARD = {
  version: 1,
  template: "weather",
  title: "Weather in Chicago",
  data: {
    location: "Chicago",
    observedAt: "2026-11-17T20:00:00Z",
    temperature: 4,
    unit: "C",
    conditions: "Cloudy",
    humidity: 70,
    wind: "12 km/h W",
    source: "MET Norway",
    credit: "Weather data: MET Norway",
  },
}

function lookupPart(card: unknown, status: "completed" | "error" = "completed", tool = "lookup") {
  const state =
    status === "completed"
      ? { status, input: { kind: "weather", query: "Chicago" }, output: "4°C, cloudy", title: "Weather in Chicago", metadata: { kind: "weather", card }, time: { start: 0, end: 1 } }
      : { status, input: { kind: "weather", query: "Chicago" }, error: "failed", time: { start: 0, end: 1 } }
  return { id: "prt_1", sessionID: "ses_1", messageID: "msg_a", type: "tool", callID: "call_1", tool, state } as any
}

function user(extra: Record<string, unknown> = {}) {
  return { id: "msg_u", sessionID: "ses_1", role: "user", time: { created: 0 }, agent: "build", model: { providerID: "p", modelID: "m" }, ...extra } as any
}

function ctx(asks: any[], deny = false) {
  return {
    sessionID: "ses_lookup",
    messageID: "msg_assistant",
    callID: "call_1",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [{ info: user({ metadata: { time_zone: "America/Chicago" } }), parts: [] }],
    metadata: () => {},
    ask: async (req: any) => {
      asks.push(req)
      if (deny) throw new Error("denied")
    },
  } as any
}

describe("answer guard: a card delivered by lookup", () => {
  const policy = selectAnswerPolicy("What is the weather in Chicago?", { visuals: true })

  test("a weather turn requires the weather template, and prose alone fails the template check", () => {
    expect(policy).toMatchObject({ mode: "template", template: "weather" })
    expect(checkAnswerResponse(policy, "It is 4°C and cloudy in Chicago (MET Norway).").valid).toBe(false)
  })

  test("a completed lookup part with the required card counts as delivered", () => {
    if (policy.mode !== "template") throw new Error("expected a template policy")
    expect(toolDeliveredCard(lookupPart(WEATHER_CARD), policy.template)).toBe(true)
  })

  test("a different template, a failed call, no card or another tool does not count", () => {
    expect(toolDeliveredCard(lookupPart({ ...WEATHER_CARD, template: "places" }), "weather")).toBe(false)
    expect(toolDeliveredCard(lookupPart(WEATHER_CARD, "error"), "weather")).toBe(false)
    expect(toolDeliveredCard(lookupPart(undefined), "weather")).toBe(false)
    expect(toolDeliveredCard(lookupPart(WEATHER_CARD, "completed", "webfetch"), "weather")).toBe(false)
    expect(toolDeliveredCard({ type: "text", text: "x" } as any, "weather")).toBe(false)
  })
})

describe("answer_cards: widgets", () => {
  test("the hint adds the writing-only manual", () => {
    expect(answerWidgetsManual(user({ metadata: { answer_cards: "widgets" } }))).toBe(ANSWER_WIDGETS_MANUAL)
    const guard = new AnswerTurnGuard("msg_u", "Write an email to Sam saying Friday works", user({ metadata: { answer_cards: "widgets" } }))
    expect(guard.widgetsManual).toBe(ANSWER_WIDGETS_MANUAL)
    // The short manual never forces a template on clients that draw no catalog cards.
    expect(guard.required).toBe(false)
  })

  test("no hint, another value, or a turn that already carries the full manual adds nothing", () => {
    expect(answerWidgetsManual(user())).toBeUndefined()
    expect(answerWidgetsManual(user({ metadata: { answer_cards: "all" } }))).toBeUndefined()
    expect(answerWidgetsManual(user({ metadata: { answer_cards: "widgets" }, system: "## Enforced answer templates\n..." }))).toBeUndefined()
  })
})

describe("lookup permissions", () => {
  test("time and unit conversion ask nothing", async () => {
    const tool = await LookupTool.init()
    const asks: any[] = []
    const time = await tool.execute({ kind: "time", query: "now", zones: ["Asia/Tokyo"] }, ctx(asks))
    expect(time.metadata.error).toBeUndefined()
    const units = await tool.execute({ kind: "convert", query: "5 km in mi", amount: 5, from: "km", to: "mi" }, ctx(asks))
    expect(units.metadata.card?.template).toBe("conversion")
    expect(asks).toEqual([])
  })

  test("network kinds ask under websearch, a link preview under webfetch, before any fetch", async () => {
    const tool = await LookupTool.init()
    const cases = [
      [{ kind: "weather", query: "Chicago" }, "websearch"],
      [{ kind: "places", query: "coffee", place: "Riverwalk, Chicago" }, "websearch"],
      [{ kind: "convert", query: "100 USD in EUR", amount: 100, from: "USD", to: "EUR" }, "websearch"],
      [{ kind: "images", query: "Chicago skyline" }, "websearch"],
      [{ kind: "link", query: "https://example.com/" }, "webfetch"],
    ] as const
    for (const [params, permission] of cases) {
      const asks: any[] = []
      // A denied ask stops the call, so nothing reaches the network.
      await expect(tool.execute(params as any, ctx(asks, true))).rejects.toThrow("denied")
      expect(asks).toHaveLength(1)
      expect(asks[0].permission).toBe(permission)
      expect(asks[0].metadata).toMatchObject({ lookup: params.kind })
    }
  })
})
