import { describe, test, expect } from "bun:test"
import { WelcomeBox } from "../../src/cli/ui/ink-app/components/WelcomeBox"
import {
  CORAL,
  WORDMARK_ROWS,
  WORDMARK_WIDTH,
  EYE,
  EYE_ROW,
  GIZZI_HEIGHT,
  GIZZI_WIDTH,
  VISOR,
  gizziRows,
} from "../../src/cli/ui/ink-app/components/welcomeArt"

describe("WelcomeBox", () => {
  test("exports a component", () => {
    expect(typeof WelcomeBox).toBe("function")
  })
})

describe("welcomeArt", () => {
  test("wordmark rows share a consistent width", () => {
    expect(WORDMARK_ROWS).toHaveLength(5)
    for (const row of WORDMARK_ROWS) {
      expect(row.length).toBe(WORDMARK_WIDTH)
    }
  })

  test("gizzi rows are a fixed-size grid", () => {
    const rows = gizziRows({ beaconColor: CORAL, blinking: false })
    expect(rows).toHaveLength(GIZZI_HEIGHT)
    for (const row of rows) {
      expect(row.map(([t]) => t).join("")).toHaveLength(GIZZI_WIDTH)
    }
    // The coral A:// mark sits on the face panel.
    expect(rows.some(row => row.some(([t, fg, bg]) => t === "A://" && fg === CORAL && bg === VISOR))).toBe(true)
  })

  test("blink closes the eyes into the face panel", () => {
    const colors = (rows: any) => rows[EYE_ROW].flatMap(([, fg, bg]: any) => [fg, bg])
    expect(colors(gizziRows({ beaconColor: CORAL, blinking: false }))).toContain(EYE)
    expect(colors(gizziRows({ beaconColor: CORAL, blinking: true }))).not.toContain(EYE)
  })

  test("beacon row takes the animated color", () => {
    const rows = gizziRows({ beaconColor: "rgb(245,149,117)", blinking: false })
    expect(rows[0].some(([, fg]) => fg === "rgb(245,149,117)")).toBe(true)
  })

  test("static parts use theme keys, not hardcoded colors", () => {
    const rows = gizziRows({ beaconColor: CORAL, blinking: false })
    for (const row of rows.slice(1)) {
      for (const [, fg, bg] of row) {
        expect(fg).not.toMatch(/^#|^rgb/)
        expect(bg ?? "").not.toMatch(/^#|^rgb/)
      }
    }
  })
})

describe("GIZZI block wordmark", () => {
  test("uses the approved block letters with one coral core in the G", async () => {
    const { WORDMARK_ROWS, WORDMARK_CORE, WORDMARK_BLOCK } = await import("../../src/cli/ui/ink-app/components/welcomeArt")
    expect(WORDMARK_ROWS).toHaveLength(5)
    // G I Z Z I with one-block gaps: 25 grid columns, two cells each.
    expect(WORDMARK_ROWS[0]).toHaveLength(49)
    expect(WORDMARK_ROWS[WORDMARK_CORE.row]![WORDMARK_CORE.col]).toBe(WORDMARK_BLOCK)
    expect(WORDMARK_CORE).toEqual({ row: 2, col: 4 })
  })
})

describe("gizzi buddy sprite", () => {
  test("every pose fills the 12x5 companion slot", async () => {
    const { gizziBuddyRows, GIZZI_BUDDY_WIDTH } = await import("../../src/cli/ui/ink-app/buddy/gizziSprite")
    expect(GIZZI_BUDDY_WIDTH).toBe(12)
    for (const pose of ["idle", "blink", "glance", "wink"] as const) {
      const rows = gizziBuddyRows(pose)
      expect(rows).toHaveLength(5)
      for (const row of rows) expect(row.map(([t]) => t).join("")).toHaveLength(12)
    }
  })

  test("blink hides the eyes; the other poses show them", async () => {
    const { gizziBuddyRows } = await import("../../src/cli/ui/ink-app/buddy/gizziSprite")
    const hasEye = (pose: any) => gizziBuddyRows(pose).some(r => r.some(([, fg, bg]) => fg === EYE || bg === EYE))
    expect(hasEye("blink")).toBe(false)
    for (const pose of ["idle", "glance", "wink"]) expect(hasEye(pose)).toBe(true)
  })

  test("the beacon takes the pulse color", async () => {
    const { gizziBuddyRows } = await import("../../src/cli/ui/ink-app/buddy/gizziSprite")
    expect(gizziBuddyRows("idle", "gizziShimmer")[0]!.some(([, fg]) => fg === "gizziShimmer")).toBe(true)
  })
})
