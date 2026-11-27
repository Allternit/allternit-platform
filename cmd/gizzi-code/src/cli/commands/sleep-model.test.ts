import { describe, expect, test } from "bun:test"
import { formatSleepStatus, sleepError, stateWord, NO_SLEEP } from "@/cli/work/sleep-model"

const now = Date.parse("2026-11-27T12:00:00Z")
describe("formatSleepStatus", () => {
  test("never ran", () => {
    const t = formatSleepStatus({ enabled: true, last_cycle: null }, now)
    expect(t).toContain("has not run yet")
    expect(t).toContain("Last run:  never")
  })
  test("running", () => {
    const s = { enabled: true, last_cycle: { id: "c1", kind: "nap", state: "running", stage: "pages", started_at: "2026-11-27T11:59:00Z" } }
    expect(stateWord(s)).toBe("running")
    expect(formatSleepStatus(s, now)).toContain("Sleeping now (nap, pages)")
  })
  test("paused with report", () => {
    const s = { enabled: false, last_cycle: { id: "c1", state: "done", finished_at: "2026-11-27T10:00:00Z", summary: "Merged 3 notes" } }
    const t = formatSleepStatus(s, now)
    expect(t).toContain("Sleep is paused")
    expect(t).toContain("2h ago")
    expect(t).toContain("Report:    Merged 3 notes")
    expect(t).toContain("paused (resume to continue)")
  })
})
describe("sleepError", () => {
  test("404 means an older runtime", () => {
    expect(sleepError(new Error("Allternit API GET /api/v1/memory/sleep/status failed: 404 Not Found"))).toBe(NO_SLEEP)
  })
  test("401 means signed out", () => {
    expect(sleepError(new Error("failed: 401 Unauthorized"))).toContain("gizzi login")
  })
})
