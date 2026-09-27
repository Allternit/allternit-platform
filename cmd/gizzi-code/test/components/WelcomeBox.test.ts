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
