import { describe, expect, test } from "bun:test"
import { missionBadgeText } from "../../src/cli/ui/ink-app/components/MissionBadge"
import type { Mission } from "../../src/cli/mission/state"

const ms = (id: string) => ({ id, title: id, tracks: [], validate: "true", status: "pending", started: 0, fixes: [], fixesStarted: 0, attempts: 0, swarms: [], workers: [] })
function m(over: Partial<Mission> = {}): Mission {
  return {
    id: "mi-1", goal: "g", root: "/r", baseBranch: "main", branch: "mission/mi-1", worktree: "/r", brain: "b",
    maxWorkers: 2, maxAttempts: 2, status: "running", milestones: [ms("M1"), ms("M2"), ms("M3")] as Mission["milestones"],
    current: 1, createdAt: 0, log: [], ...over,
  }
}

describe("missionBadgeText", () => {
  test("running shows progress", () => expect(missionBadgeText(m())).toBe("◆ M2/3"))
  test("needs_you gets a tail", () => expect(missionBadgeText(m({ status: "needs_you" }))).toBe("◆ M2/3 · needs you"))
  test("other states and none are hidden", () => {
    for (const s of ["planning", "awaiting_approval", "done", "stopped", "failed"] as const) expect(missionBadgeText(m({ status: s }))).toBeNull()
    expect(missionBadgeText(null)).toBeNull()
  })
  test("current past the end clamps", () => expect(missionBadgeText(m({ current: 9 }))).toBe("◆ M3/3"))
})
