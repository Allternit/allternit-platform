import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import fs from "fs/promises"
import { Clock } from "../../src/runtime/primitives/clock"
import { CurrentTimeTool, PROCESS_ZONE_NOTE } from "../../src/runtime/tools/builtins/current-time"

// 2026-11-17 20:03:05 UTC, a Tuesday.
const AT = new Date("2026-11-17T20:03:05Z")

async function clearState() {
  await fs.rm(Clock.stateFile(), { force: true })
  Clock.resetCache()
}

function ctx(metadata?: Record<string, unknown>) {
  return {
    sessionID: "ses_clock",
    messageID: "msg_assistant",
    callID: "call_1",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [
      { info: { id: "msg_user", sessionID: "ses_clock", role: "user", time: { created: 0 }, metadata }, parts: [] },
    ],
    metadata: () => {},
    ask: async () => {},
  } as any
}

describe("Clock.resolveTimeZone", () => {
  beforeEach(clearState)
  afterEach(clearState)

  test("priority: argument, then this turn's zone, then the last client zone, then the process zone", async () => {
    expect(await Clock.resolveTimeZone({})).toEqual({ zone: Clock.processZone(), source: "process" })

    await Clock.rememberClientZone("Europe/Berlin")
    expect(await Clock.resolveTimeZone({})).toEqual({ zone: "Europe/Berlin", source: "last_client" })
    expect(await Clock.resolveTimeZone({ turnZone: "America/Chicago" })).toEqual({
      zone: "America/Chicago",
      source: "client",
    })
    expect(await Clock.resolveTimeZone({ argument: "Asia/Tokyo", turnZone: "America/Chicago" })).toEqual({
      zone: "Asia/Tokyo",
      source: "argument",
    })
  })

  test("an invalid zone falls through to the next source", async () => {
    await Clock.rememberClientZone("Europe/Berlin")
    expect(await Clock.resolveTimeZone({ argument: "Mars/Olympus", turnZone: "America/Chicago" })).toEqual({
      zone: "America/Chicago",
      source: "client",
    })
    expect(await Clock.resolveTimeZone({ argument: "+05:00", turnZone: "not a zone" })).toEqual({
      zone: "Europe/Berlin",
      source: "last_client",
    })
    // An invalid client zone is never stored.
    await Clock.rememberClientZone("Nowhere/Land")
    await Clock.rememberClientZone("x".repeat(65))
    expect((await Clock.resolveTimeZone({})).zone).toBe("Europe/Berlin")
  })

  test("the last client zone is persisted and read back after a restart", async () => {
    await Clock.rememberClientZone("America/Los_Angeles")
    const stored = JSON.parse(await fs.readFile(Clock.stateFile(), "utf8"))
    expect(stored.zone).toBe("America/Los_Angeles")

    Clock.resetCache() // a new process: nothing in memory
    expect(await Clock.resolveTimeZone({})).toEqual({ zone: "America/Los_Angeles", source: "last_client" })

    // A newer client zone replaces it.
    await Clock.rememberClientZone("Asia/Kolkata")
    Clock.resetCache()
    expect(await Clock.resolveTimeZone({})).toEqual({ zone: "Asia/Kolkata", source: "last_client" })
  })
})

describe("Clock.now", () => {
  test("local ISO time with offset, zone and weekday", () => {
    expect(Clock.now("America/Chicago", AT)).toEqual({
      iso: "2026-11-17T14:03:05-06:00",
      time_zone: "America/Chicago",
      utc_offset: "-06:00",
      weekday: "Tuesday",
    })
    // A half-hour zone past midnight: the date and weekday move too.
    expect(Clock.now("Asia/Kolkata", AT)).toEqual({
      iso: "2026-11-18T01:33:05+05:30",
      time_zone: "Asia/Kolkata",
      utc_offset: "+05:30",
      weekday: "Wednesday",
    })
    expect(Clock.now("UTC", AT).utc_offset).toBe("+00:00")
  })

  test("the environment date is the user's date", () => {
    expect(Clock.dateString("America/Chicago", AT)).toBe("Tue Nov 17 2026")
    expect(Clock.dateString("Asia/Kolkata", AT)).toBe("Wed Nov 18 2026")
  })
})

describe("tool.current_time", () => {
  beforeEach(clearState)
  afterEach(clearState)

  test("uses this turn's zone from the user message and returns the source", async () => {
    const tool = await CurrentTimeTool.init()
    const result = await tool.execute({}, ctx({ time_zone: "America/New_York" }))
    expect(result.metadata.time_zone).toBe("America/New_York")
    expect(result.metadata.source).toBe("client")
    expect(result.metadata.iso).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}[+-]\d{2}:\d{2}$/)
    // The model reads the same object the metadata carries (Tool.define adds only `truncated`).
    const shown = JSON.parse(result.output.split("\n").slice(0, 7).join("\n"))
    expect(Object.keys(shown).sort()).toEqual(["iso", "source", "time_zone", "utc_offset", "weekday"])
    const { truncated: _truncated, ...metadata } = result.metadata
    expect(shown).toEqual(metadata)
    expect(result.output).not.toContain(PROCESS_ZONE_NOTE)
  })

  test("an explicit timezone wins; an invalid one says so and falls back", async () => {
    const tool = await CurrentTimeTool.init()
    const tokyo = await tool.execute({ timezone: "Asia/Tokyo" }, ctx({ time_zone: "America/New_York" }))
    expect(tokyo.metadata).toMatchObject({ time_zone: "Asia/Tokyo", utc_offset: "+09:00", source: "argument" })

    const bad = await tool.execute({ timezone: "Moon/Base" }, ctx({ time_zone: "America/New_York" }))
    expect(bad.metadata).toMatchObject({ time_zone: "America/New_York", source: "client" })
    expect(bad.output).toContain('"Moon/Base" is not a valid IANA time zone')
  })

  test("with no client zone at all it labels the machine's zone", async () => {
    const tool = await CurrentTimeTool.init()
    const result = await tool.execute({}, ctx())
    expect(result.metadata.source).toBe("process")
    expect(result.metadata.time_zone).toBe(Clock.processZone())
    expect(result.output).toContain(PROCESS_ZONE_NOTE)
  })

  test("a turn without a zone uses the zone a client sent earlier", async () => {
    await Clock.rememberClientZone("Australia/Sydney")
    const tool = await CurrentTimeTool.init()
    const result = await tool.execute({}, ctx())
    expect(result.metadata).toMatchObject({ time_zone: "Australia/Sydney", source: "last_client" })
  })
})
