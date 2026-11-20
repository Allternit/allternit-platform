import { describe, expect, test } from "bun:test"
import { isPersonOnlyRef, refuseIfPersonOnly, wireModelID } from "../../../src/runtime/providers/person-plan/rules"
import { planProvidersFromRows } from "../../../src/runtime/providers/person-plan/discovery"
import { parseChatCompletionsStream, planErrorText, readErrorBody } from "../../../src/runtime/providers/person-plan/stream"

const ID = "linked/claude/claude-sonnet-5-5"

describe("claude-org person-only routing", () => {
  test("claude_org_is_person_only", () => {
    expect(isPersonOnlyRef("claude-org/claude-sonnet-5-5")).toBe(true)
    for (const kind of ["headless", "mission", "subagent", "bot"] as const) {
      expect(refuseIfPersonOnly("claude-org/claude-sonnet-5-5", kind)).toContain("your own plan")
    }
  })
  test("wire_model_id_maps_claude_org_to_linked", () => {
    expect(wireModelID("claude-org", "claude-sonnet-5-5")).toBe(ID)
  })
})

describe("claude-org discovery", () => {
  test("discovery_maps_claude_org_row_models_when_on", () => {
    const out = planProvidersFromRows([{ id: "claude-org", state: "on", models: [ID] }])
    expect(out).toHaveLength(1)
    expect(out[0]!.id).toBe("claude-org")
    expect(out[0]!.name).toBe("Claude organization")
    expect(out[0]!.options).toEqual({ runtime: "person-plan" })
    expect(out[0]!.models.map((m) => [m.id, m.name])).toEqual([["claude-sonnet-5-5", "Claude Sonnet 5.5 (your organization)"]])
  })
  test("discovery_skips_claude_org_when_needs_or_off", () => {
    for (const state of ["needs", "off", "connecting"]) {
      expect(planProvidersFromRows([{ id: "claude-org", state, models: [ID] }])).toEqual([])
    }
  })
})

describe("claude-org refusals", () => {
  test("claude_link_refusal_message_shown", async () => {
    const body = JSON.stringify({ error: { code: "claude_link_spend_limit", message: "Spend limit reached." }, fallbacks: ["allternit-credits"] })
    const err = readErrorBody(body)
    expect(planErrorText(err.code, 402, err.message, err.fallbacks)).toBe("Spend limit reached. Switch to Allternit credits to continue.")
    expect(planErrorText("claude_link_disconnected", 401, undefined, undefined)).toContain("disconnected")
    const enc = new TextEncoder()
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(enc.encode(`data: ${body}\n\ndata: [DONE]\n\n`))
        c.close()
      },
    })
    const events: any[] = []
    for await (const ev of parseChatCompletionsStream(stream)) events.push(ev)
    expect(events[0]).toEqual({ type: "error", code: "claude_link_spend_limit", message: "Spend limit reached. Switch to Allternit credits to continue." })
  })
})
