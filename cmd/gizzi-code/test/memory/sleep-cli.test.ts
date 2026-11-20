import { describe, expect, test } from "bun:test"
import { interpretSleepResponse, NO_SLEEP_MESSAGE } from "../../src/runtime/memory/sleep/client"

describe("interpretSleepResponse", () => {
  test("sleep_disabled refusal, with 409 or 200", () => {
    for (const status of [409, 200]) {
      const r = interpretSleepResponse(status, { accepted: false, reason: "sleep_disabled", stage: "orchestrator" })
      expect(r).toEqual({ kind: "refused", message: "Sleep is off. Turn it on with `gizzi memory sleep on`." })
    }
  })
  test("stage_not_built names the stage", () => {
    const r = interpretSleepResponse(409, { accepted: false, reason: "stage_not_built", stage: "disputes" })
    expect(r).toEqual({ kind: "refused", message: "Not built yet: disputes." })
  })
  test("bare 404 is an older runtime; a 404 with an error body is a real not-found", () => {
    expect(interpretSleepResponse(404, undefined)).toEqual({ kind: "no_sleep", message: NO_SLEEP_MESSAGE })
    expect(interpretSleepResponse(404, { error: "Cycle not found" })).toEqual({
      kind: "error",
      status: 404,
      message: "Cycle not found",
    })
  })
  test("2xx passes the body through unchanged", () => {
    const body = { available: true, enabled: false }
    expect(interpretSleepResponse(200, body)).toEqual({ kind: "ok", body })
  })
})
