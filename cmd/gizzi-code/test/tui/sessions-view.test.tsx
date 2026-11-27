import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import * as React from "react"
import { renderToString } from "../../src/cli/ui/ink-app/utils/staticRender"
import { SessionRow, SessionTile } from "../../src/cli/ui/ink-app/screens/sessions/SessionTile"
import {
  blockedNoticeBytes,
  confirmMatches,
  steerSession,
  toRow,
  toRows,
  type LiveSessionRow,
} from "../../src/cli/ui/ink-app/screens/sessions/liveSessions"
import { SessionsPoller } from "../../src/cli/ui/ink-app/screens/sessions/useLiveSessions"

const NOW = Date.parse("2026-11-27T12:00:00Z")

function engineRow(over: Record<string, unknown> = {}) {
  return {
    address: "eoj/api-claude",
    name: "api-claude",
    harness: "claude",
    machine: "local",
    machineName: "studio",
    sessionId: "s1",
    paneId: "w1:p2",
    botId: "local:api-claude-t1",
    cwd: "/Users/eoj/allternit",
    status: "busy",
    since: "2026-11-27T11:55:00Z",
    doing: "editing live_sessions.rs",
    tier: "push",
    unread: 0,
    holders: [],
    ...over,
  }
}

function row(over: Record<string, unknown> = {}): LiveSessionRow {
  return toRow(engineRow(over))!
}

describe("live session rows", () => {
  test("an engine without blocked (legacy) shows idle; one with it shows blocked", () => {
    expect(row({ status: "idle" }).status).toBe("idle")
    const blocked = row({ status: "blocked", blockedSince: "2026-11-27T11:59:00Z", steer: "terminal", terminalId: "term_9" })
    expect(blocked.status).toBe("blocked")
    expect(blocked.steer).toBe("terminal")
    expect(blocked.terminalId).toBe("term_9")
    expect(row({ status: "busy" }).status).toBe("working")
    expect(row({ status: "offline" }).status).toBe("done")
  })

  test("a row without steer takes the send route; rows without an address are dropped", () => {
    expect(row().steer).toBe("delivery")
    expect(toRows([engineRow(), { name: "no address" }]).map((r) => r.address)).toEqual(["eoj/api-claude"])
  })

  test.each([
    ["working", { status: "busy" }],
    ["blocked", { status: "blocked", doing: "Allow edit to main.rs?" }],
    ["idle", { status: "idle", doing: null }],
  ])("list row shows %s with runtime, folder, doing and age", async (word, over) => {
    const r = row(over)
    const out = await renderToString(<SessionRow row={r} lines={["last output line"]} selected={false} width={110} now={NOW} keys="" />, 120)
    expect(out).toContain("api-claude")
    expect(out).toContain("claude")
    expect(out).toContain("allternit")
    expect(out).toContain(word)
    expect(out).toContain("5m")
    expect(out).toContain(r.doing ?? "last output line")
  })

  test("grid tile shows the last output lines and the selected session's keys", async () => {
    const out = await renderToString(
      <SessionTile row={row({ status: "blocked" })} lines={["one", "two", "three"]} selected width={40} now={NOW} keys="s steer · x stop" />,
      60,
    )
    expect(out).toContain("blocked")
    expect(out).toContain("one")
    expect(out).toContain("three")
    expect(out).toContain("x stop")
  })
})

describe("blocked notice", () => {
  function poller(lists: LiveSessionRow[][], enabled = true) {
    const writes: string[] = []
    let i = 0
    const p = new SessionsPoller({
      list: async () => lists[Math.min(i++, lists.length - 1)]!,
      capture: async () => [],
      noticeEnabled: () => enabled,
    })
    p.addWriter((d) => writes.push(d))
    return { p, writes }
  }

  test("one bell + OSC 9 per transition to blocked, none while it stays blocked", async () => {
    const idle = row({ status: "idle" })
    const blocked = row({ status: "blocked" })
    const { p, writes } = poller([[idle], [blocked], [blocked], [idle], [blocked]])
    for (let k = 0; k < 5; k++) await p.tick()
    p.stop()
    expect(writes).toEqual([blockedNoticeBytes("api-claude"), blockedNoticeBytes("api-claude")])
    expect(writes[0]).toBe("\x07\x1b]9;api-claude needs you\x07")
    expect(p.snapshot.blockedCount).toBe(1)
  })

  test("a session already blocked when Gizzi opens does not ring; the setting turns notices off", async () => {
    const blocked = row({ status: "blocked" })
    const first = poller([[blocked], [blocked]])
    await first.p.tick()
    await first.p.tick()
    expect(first.writes).toEqual([])
    expect(first.p.snapshot.blockedCount).toBe(1)

    const off = poller([[row({ status: "idle" })], [blocked]], false)
    await off.p.tick()
    await off.p.tick()
    expect(off.writes).toEqual([])
    expect(off.p.snapshot.blockedCount).toBe(1)
  })
})

describe("stop needs the typed name", () => {
  test("only the exact session name confirms", () => {
    const r = row()
    expect(confirmMatches(r, "api-claude")).toBe(true)
    expect(confirmMatches(r, "  api-claude ")).toBe(true)
    expect(confirmMatches(r, "y")).toBe(false)
    expect(confirmMatches(r, "")).toBe(false)
    expect(confirmMatches(r, "api")).toBe(false)
  })
})

describe("steer", () => {
  const realFetch = globalThis.fetch
  const env = { ...process.env }
  let calls: { url: string; body: string }[] = []

  beforeEach(() => {
    calls = []
    process.env.ALLTERNIT_FACTORY_URL = "http://engine.test"
    process.env.ALLTERNIT_API_URL = "http://api.test"
    process.env.ALLTERNIT_API_TOKEN = "t"
    globalThis.fetch = (async (url: string, init?: RequestInit) => {
      calls.push({ url: String(url), body: String(init?.body ?? "") })
      return new Response(JSON.stringify({ state: "delivered", tier: "push" }), { status: 200 })
    }) as typeof fetch
  })
  afterEach(() => {
    globalThis.fetch = realFetch
    process.env = { ...env }
  })

  test("a delivery session calls the send route once with its address", async () => {
    const r = await steerSession(row({ steer: "delivery" }), "run the tests")
    expect(calls).toHaveLength(1)
    expect(calls[0]!.url).toBe("http://engine.test/api/factory/send")
    expect(JSON.parse(calls[0]!.body)).toEqual({ to: "eoj/api-claude", text: "run the tests" })
    expect(r.via).toBe("delivery")
  })

  test("a terminal session types into its terminal once, with Enter", async () => {
    await steerSession(row({ steer: "terminal", terminalId: "term_9" }), "yes")
    expect(calls).toHaveLength(1)
    expect(calls[0]!.url).toEndWith("/terminal/term_9/input")
    expect(JSON.parse(calls[0]!.body)).toEqual({ content: "yes\r" })
  })

  test("a session with steer none is refused without a request", async () => {
    await expect(steerSession(row({ steer: "none" }), "hi")).rejects.toThrow("can't be steered")
    expect(calls).toHaveLength(0)
  })
})
