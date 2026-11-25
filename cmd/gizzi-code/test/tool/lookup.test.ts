import { afterEach, describe, expect, test } from "bun:test"
import { checkAnswerResponse, selectAnswerPolicy } from "../../src/runtime/integrations/artifacts/answer-policy"
import { ANSWER_WIDGETS_MANUAL } from "../../src/runtime/integrations/artifacts/answer-contract"
import { AnswerTurnGuard, answerWidgetsManual, toolDeliveredCard } from "../../src/runtime/session/answer-policy"
import { LookupTool, TurnLocations, parseTurnLocation, resolveTurnLocation, turnLocationFromField } from "../../src/runtime/tools/builtins/lookup"
import { clearLookupCache, transport } from "../../src/runtime/primitives/lookup/http"
import { CliBridgeResults } from "../../src/runtime/integrations/cli-bridge/results"
import { promptHints } from "../../src/runtime/server/routes/agent-compat"

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

function ctx(asks: any[], deny = false, extra?: Record<string, unknown>) {
  return {
    ...(extra ? { extra } : {}),
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

// allternit-api client_location::turn_value: the turn-only `turnLocation` field.
const FIELD = { lat: 40.71, lon: -74.01, accuracy_m: 1000, time_zone: "America/New_York", at: "2026-11-17T15:00:00Z" }

// allternit-api client_location::location_layer, after the answer-template line in turnSystem (prose for the model).
const LAYER =
  "Use the weather template for this answer.\n\nWhere the user is (shared from their device just now, approximate): about 40.71, -74.01, within roughly 1 km. Their time zone: America/New_York. Use this only when it helps, for local time, nearby places, weather or directions. Don't mention it otherwise, and don't save it to memory."

describe("lookup: the shared location for \"here\"", () => {
  const realFetch = transport.fetch
  let urls: string[] = []
  function stubNetwork() {
    clearLookupCache()
    urls = []
    transport.fetch = (async (input: string | URL | Request) => {
      const u = String(input)
      urls.push(u)
      const body = u.includes("api.met.no")
        ? { properties: { timeseries: [{ time: "2026-11-17T15:00:00Z", data: { instant: { details: { air_temperature: 7, relative_humidity: 60, wind_speed: 3 } }, next_1_hours: { summary: { symbol_code: "cloudy" } } } }] } }
        : [{ name: "Cafe A", display_name: "Cafe A, 1 Main St", lat: "40.71", lon: "-74.0", category: "amenity", type: "cafe" }]
      return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } })
    }) as typeof transport.fetch
  }
  afterEach(() => {
    transport.fetch = realFetch
    TurnLocations.release("ses_lookup", TurnLocations.get("ses_lookup")!)
  })

  test("the turnLocation field gives the structured location; a bad field gives none", () => {
    expect(turnLocationFromField(FIELD)).toEqual({ lat: 40.71, lon: -74.01, accuracyKm: 1, timeZone: "America/New_York" })
    expect(turnLocationFromField({ ...FIELD, accuracy_m: 4600, place: "Lower Manhattan" })).toEqual({
      lat: 40.71,
      lon: -74.01,
      accuracyKm: 5,
      timeZone: "America/New_York",
      place: "Lower Manhattan",
    })
    expect(turnLocationFromField({ ...FIELD, lat: 140 })).toBeUndefined()
    expect(turnLocationFromField({ lat: "40.71", lon: -74.01 })).toBeUndefined()
    expect(turnLocationFromField(undefined)).toBeUndefined()
  })

  test("the field wins over the turnSystem text, whatever its wording; the text is read only when the field is absent", () => {
    const reworded = "Use the weather template for this answer.\n\nThe user is near 51.51, -0.13 (approx.)."
    expect(resolveTurnLocation(FIELD, reworded)).toEqual({ lat: 40.71, lon: -74.01, accuracyKm: 1, timeZone: "America/New_York" })
    expect(resolveTurnLocation(FIELD, undefined)?.lat).toBe(40.71)
    // An invalid field means no location, not a fall back to the prose.
    expect(resolveTurnLocation({ lat: 999, lon: 0 }, LAYER)).toBeUndefined()
    // An older allternit-api sends only the prose line.
    expect(resolveTurnLocation(undefined, LAYER)).toEqual({ lat: 40.71, lon: -74.01, accuracyKm: 1, timeZone: "America/New_York" })
    expect(resolveTurnLocation(undefined, reworded)).toBeUndefined()
  })

  test("legacy: the turnSystem location line of an older API parses into a structured value; other text does not", () => {
    expect(parseTurnLocation(LAYER)).toEqual({ lat: 40.71, lon: -74.01, accuracyKm: 1, timeZone: "America/New_York" })
    expect(parseTurnLocation("Use the weather template for this answer.")).toBeUndefined()
    expect(parseTurnLocation(undefined)).toBeUndefined()
    expect(parseTurnLocation("Where the user is (approximate): about 140.00, 10.00, within roughly 1 km.")).toBeUndefined()
  })

  test("weather for \"here\" uses the structured turnLocation on the tool context", async () => {
    stubNetwork()
    const tool = await LookupTool.init()
    const asks: any[] = []
    const result = await tool.execute({ kind: "weather", query: "here" }, ctx(asks, false, { location: resolveTurnLocation(FIELD, undefined) }))
    expect(urls.some((u) => u.includes("api.met.no") && u.includes("lat=40.71") && u.includes("lon=-74.01"))).toBe(true)
    expect(result.metadata.card?.template).toBe("weather")
    expect((result.metadata.card as any).data.location).toBe("your location")
    expect(asks).toHaveLength(1)
  })

  test("places \"near me\" uses the session's turn location (the CLI-bridge path, no extra)", async () => {
    stubNetwork()
    TurnLocations.set("ses_lookup", turnLocationFromField(FIELD)!)
    const tool = await LookupTool.init()
    const result = await tool.execute({ kind: "places", query: "coffee", place: "near me" }, ctx([]))
    const search = urls.find((u) => u.includes("viewbox="))
    expect(search).toBeDefined()
    expect(search).toContain("bounded=1")
    expect(result.metadata.error).toBeUndefined()
  })

  test("without a shared location, \"here\" asks for a place: no permission prompt, no fetch", async () => {
    stubNetwork()
    const tool = await LookupTool.init()
    for (const params of [
      { kind: "weather", query: "here" },
      { kind: "weather", query: "weather", place: "near me" },
      { kind: "places", query: "coffee", place: "near me" },
    ] as const) {
      const asks: any[] = []
      const result = await tool.execute(params as any, ctx(asks))
      expect(result.metadata.error).toContain("Which place?")
      expect(result.metadata.card).toBeUndefined()
      expect(asks).toEqual([])
    }
    expect(urls).toEqual([])
  })
})

describe("answer_cards hint", () => {
  test("promptHints keeps answer_cards \"widgets\" beside the other hints, and only that value", () => {
    expect(promptHints({ answer_cards: "widgets" })).toEqual({ answer_cards: "widgets" })
    expect(promptHints({ answer_cards: "widgets", form_factor: "phone", time_zone: "America/Chicago" })).toEqual({
      form_factor: "phone",
      time_zone: "America/Chicago",
      answer_cards: "widgets",
    })
    expect(promptHints({ answer_cards: "all" })).toBeUndefined()
    expect(promptHints({ answer_cards: ["widgets"] })).toBeUndefined()
  })

  test("the widgets hint, carried on the user message, gives the turn the writing-only manual", () => {
    const metadata = promptHints({ answer_cards: "widgets", time_zone: "America/Chicago" })
    const guard = new AnswerTurnGuard("msg_u", "What's the weather in Chicago?", user({ metadata }))
    expect(guard.widgetsManual).toBe(ANSWER_WIDGETS_MANUAL)
    expect(new AnswerTurnGuard("msg_u", "What's the weather in Chicago?", user({ metadata: promptHints({ time_zone: "America/Chicago" }) })).widgetsManual).toBeUndefined()
  })
})

describe("person turn + image_prompt hints", () => {
  test("a person send keeps turn \"person\" and its image_prompt beside the other hints", () => {
    expect(promptHints({ turn: "person" })).toEqual({ turn: "person" })
    expect(promptHints({ turn: "person", image_prompt: "a red fox in snow", time_zone: "America/Chicago" })).toEqual({
      time_zone: "America/Chicago",
      turn: "person",
      image_prompt: "a red fox in snow",
    })
  })

  test("any other turn value is dropped, and its image_prompt with it", () => {
    expect(promptHints({ turn: "bot", image_prompt: "a red fox" })).toBeUndefined()
    expect(promptHints({ turn: "Person", image_prompt: "a red fox" })).toBeUndefined()
    expect(promptHints({ turn: ["person"], image_prompt: "a red fox" })).toBeUndefined()
    expect(promptHints({ turn: "routine", time_zone: "America/Chicago" })).toEqual({ time_zone: "America/Chicago" })
  })

  test("an empty, oversize or non-string image_prompt is dropped; 16000 UTF-16 units passes", () => {
    expect(promptHints({ turn: "person", image_prompt: "" })).toEqual({ turn: "person" })
    expect(promptHints({ turn: "person", image_prompt: "x".repeat(16_001) })).toEqual({ turn: "person" })
    // 8001 emoji = 16002 UTF-16 units, over the limit even though it is 8001 characters.
    expect(promptHints({ turn: "person", image_prompt: "🦊".repeat(8_001) })).toEqual({ turn: "person" })
    expect(promptHints({ turn: "person", image_prompt: { text: "fox" } })).toEqual({ turn: "person" })
    const max = "x".repeat(16_000)
    expect(promptHints({ turn: "person", image_prompt: max })).toEqual({ turn: "person", image_prompt: max })
  })

  test("image_prompt without a person turn is dropped", () => {
    expect(promptHints({ image_prompt: "a red fox" })).toBeUndefined()
    expect(promptHints({ image_prompt: "a red fox", form_factor: "phone" })).toEqual({ form_factor: "phone" })
  })

  test("a send that starts no turn (person: false) drops both", () => {
    expect(promptHints({ turn: "person", image_prompt: "a red fox" }, { person: false })).toBeUndefined()
    expect(promptHints({ turn: "person", image_prompt: "a red fox", time_zone: "America/Chicago" }, { person: false })).toEqual({
      time_zone: "America/Chicago",
    })
  })
})

describe("lookup cards survive CLI-brain turns", () => {
  test("vendor names for the bridged lookup map back to \"lookup\"", () => {
    for (const name of ["mcp__allternit__lookup", "allternit__lookup", "allternit_lookup", "allternit.lookup", "lookup (allternit MCP Server)", "lookup"]) {
      expect(CliBridgeResults.canonicalTool(name)).toBe("lookup")
    }
    expect(CliBridgeResults.isCanonical("lookup")).toBe(true)
    expect(CliBridgeResults.canonicalTool("mcp__other__lookup")).toBe("mcp__other__lookup")
  })

  test("the stashed result keeps metadata.card, so the merged part passes the answer guard", () => {
    const args = { kind: "weather", query: "Chicago" }
    CliBridgeResults.put("ses_cli", "lookup", args, { title: "Weather in Chicago", metadata: { kind: "weather", card: WEATHER_CARD, credit: "Weather data: MET Norway" } })
    // The CLI reports the call back with its args re-serialized (key order differs).
    const taken = CliBridgeResults.take("ses_cli", CliBridgeResults.canonicalTool("mcp__allternit__lookup"), { query: "Chicago", kind: "weather" })
    expect(taken?.metadata.card).toEqual(WEATHER_CARD)
    // What processor.ts builds for the observed result: the canonical name, the merged metadata.
    const part = lookupPart(undefined)
    part.state.metadata = { ...taken!.metadata }
    expect(toolDeliveredCard(part, "weather")).toBe(true)
  })
})
