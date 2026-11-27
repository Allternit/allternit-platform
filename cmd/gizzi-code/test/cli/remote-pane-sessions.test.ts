import { describe, expect, test } from "bun:test"
import {
  fetchPaneSessions,
  paneReach,
  paneSessionLine,
  paneSessions,
} from "../../src/cli/ui/ink-app/services/liveSession"

const rows = [
  { address: "me/api-codex", name: "api-codex", harness: "codex", status: "blocked", doing: "Run cargo test?", steer: "terminal", terminalId: "term_1", paneId: "p1" },
  { address: "me/web-claude", name: "web-claude", harness: "claude", status: "busy", steer: "none", paneId: "p2" },
  { address: "me/me-gizzi", harness: "gizzi-tui", status: "idle", steer: "terminal", terminalId: "term_9" },
  { address: "me/old", harness: "grok", status: "offline", paneId: "p3" },
  { address: "me/hooks-only", harness: "claude", status: "idle" },
]

describe("/remote pane sessions", () => {
  test("lists pane sessions only, not gizzi itself, offline or hook-only rows", () => {
    expect(paneSessions(rows).map(r => r.address)).toEqual(["me/api-codex", "me/web-claude"])
  })
  test("steer decides which devices can reach it", () => {
    expect(paneReach({ steer: "terminal" })).toBe("phone, web and Desktop")
    expect(paneReach({ steer: "delivery" })).toBe("phone, web and Desktop")
    expect(paneReach({ steer: "none" })).toBe("view only")
    expect(paneReach({})).toBe("view only")
  })
  test("each line carries status, doing and reach", () => {
    expect(paneSessionLine(rows[0])).toBe("api-codex (codex) · blocked · Run cargo test? · phone, web and Desktop")
    expect(paneSessionLine(rows[1])).toBe("web-claude (claude) · busy · view only")
  })
  test("fetch accepts a bare list or {sessions}", async () => {
    expect((await fetchPaneSessions(async () => rows)).length).toBe(2)
    expect((await fetchPaneSessions(async () => ({ sessions: rows }))).length).toBe(2)
  })
})
