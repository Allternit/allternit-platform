import { describe, expect, test } from "bun:test"
import { callerStartMode, checkStart, nextDepth } from "../../src/cli/work/start-guards"

describe("start guards", () => {
  test("mode defaults to the caller's and never goes above it", () => {
    expect(checkStart({ callerMode: "auto" })).toEqual({ ok: true, mode: "auto", depth: 1 })
    expect(checkStart({ callerMode: "auto", requestedMode: "plan" })).toMatchObject({ ok: true, mode: "plan" })
    expect(checkStart({ callerMode: "default", requestedMode: "auto" })).toMatchObject({ ok: false, error: "mode_not_allowed" })
    expect(checkStart({ callerMode: "plan", requestedMode: "default" })).toMatchObject({ ok: false, error: "mode_not_allowed" })
    expect(checkStart({ callerMode: "bypass", requestedMode: "bypass" })).toMatchObject({ ok: true })
  })
  test("depth is parent + 1 and refused above 3", () => {
    expect(nextDepth(undefined)).toBe(1)
    expect(checkStart({ callerMode: "auto", parentDepth: 2 })).toMatchObject({ ok: true, depth: 3 })
    expect(checkStart({ callerMode: "auto", parentDepth: 3 })).toMatchObject({ ok: false, error: "depth_exceeded" })
  })
  test("Ink permission modes map onto contract modes", () => {
    expect(callerStartMode("acceptEdits")).toBe("auto")
    expect(callerStartMode("bypassPermissions")).toBe("bypass")
    expect(callerStartMode("dontAsk")).toBe("default")
    expect(callerStartMode("plan")).toBe("plan")
  })
})
