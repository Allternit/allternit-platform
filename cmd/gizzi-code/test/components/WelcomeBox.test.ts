import { describe, test, expect } from "bun:test"
import { WelcomeBox } from "../../src/cli/ui/ink-app/components/WelcomeBox"
import {
  CORAL,
  CORAL_BRIGHT,
  WORDMARK_ROWS,
  WORDMARK_WIDTH,
  sentinelRows,
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

  test("sentinel blink swaps the eye glyphs", () => {
    const open = sentinelRows({ beaconColor: CORAL, blinking: false })
    const shut = sentinelRows({ beaconColor: CORAL, blinking: true })
    const eyes = (rows: any) => rows[3][1][0]
    expect(eyes(open)).toContain("●")
    expect(eyes(shut)).toContain("─")
    expect(eyes(shut)).not.toContain("●")
  })

  test("beacon row takes the animated color", () => {
    const rows = sentinelRows({ beaconColor: CORAL_BRIGHT, blinking: false })
    expect(rows[0][0][1]).toBe(CORAL_BRIGHT)
  })
})
