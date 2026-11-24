import { describe, expect, test } from "bun:test"
import { describeApiError, END_REASON_LABELS, gizziLabel, parseCallUrl, parseSse, runnerLine } from "./meetings-model"

describe("gizziLabel", () => {
  test("uses the same words as the web", () => {
    expect(gizziLabel("scheduled")).toBe("Gizzi joins")
    expect(gizziLabel("waiting_admit")).toBe("Waiting to be let in")
    expect(gizziLabel("not_admitted")).toBe("Not let in")
    expect(gizziLabel("ended")).toBe("Recap ready")
    expect(gizziLabel(undefined)).toBe("Off")
  })
})

describe("parseCallUrl", () => {
  test("accepts Meet, Zoom and Teams links", () => {
    expect(parseCallUrl("https://meet.google.com/abc-defg-hij?authuser=0")?.platform).toBe("meet")
    expect(parseCallUrl("https://us02web.zoom.us/j/12345678901?pwd=x")?.platform).toBe("zoom")
    expect(parseCallUrl("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc/0?context=x")?.platform).toBe("teams")
  })
  test("rejects anything else", () => {
    expect(parseCallUrl("http://meet.google.com/abc-defg-hij")).toBeUndefined()
    expect(parseCallUrl("https://meet.google.com/abc")).toBeUndefined()
    expect(parseCallUrl("https://example.com/j/12345678901")).toBeUndefined()
  })
})

describe("describeApiError", () => {
  test("402 names the paid plan and the upgrade page", () => {
    const m = describeApiError(402, { error: "meetings_requires_paid_plan" })
    expect(m).toContain("Meetings needs a paid plan")
    expect(m).toContain("https://")
  })
  test("401 points at login", () => expect(describeApiError(401)).toContain("gizzi login"))
})

describe("parseSse", () => {
  test("splits frames, skips pings, keeps a partial frame", () => {
    const { events, rest } = parseSse(': ping\n\nid: 4\nevent: meeting.ended\ndata: {"a":1}\n\nid: 5\nevent: x\ndata: par')
    expect(events).toEqual([{ id: "4", event: "meeting.ended", data: '{"a":1}' }])
    expect(rest).toContain("id: 5")
  })
})

describe("runnerLine and no_runtime", () => {
  test("ready cloud computer, desktop, asleep, none", () => {
    expect(runnerLine({ state: "ready", runtime: { id: "a", name: "Work", kind: "cloud_computer" } })).toBe("Gizzi joins from your cloud computer (Work). It can join while your own computer is off.")
    expect(runnerLine({ state: "ready", runtime: { id: "a", name: "My Mac", kind: "desktop" } })).toContain("Keep Allternit open")
    expect(runnerLine({ state: "asleep", runtime: null })).toContain("asleep")
    expect(runnerLine({ state: "none", runtime: null })).toContain("No computer is online")
  })
  test("no_runtime label", () => expect(END_REASON_LABELS.no_runtime).toContain("None of your computers was online"))
})
