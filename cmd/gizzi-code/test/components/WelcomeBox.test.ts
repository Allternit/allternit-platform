import { describe, test, expect } from "bun:test"
import { WelcomeBox } from "../../src/cli/ui/ink-app/components/WelcomeBox"
import {
  CORAL,
  EYE,
  HEADER_MARK_HEIGHT,
  HEADER_MARK_WIDTH,
  headerMarkRows,
} from "../../src/cli/ui/ink-app/components/welcomeArt"

describe("WelcomeBox", () => {
  test("exports a component", () => {
    expect(typeof WelcomeBox).toBe("function")
  })
})

describe("header mark", () => {
  test("is three rows tall and a fixed width", () => {
    const rows = headerMarkRows({ blinking: false })
    expect(rows).toHaveLength(HEADER_MARK_HEIGHT)
    expect(HEADER_MARK_HEIGHT).toBe(3)
    for (const row of rows) expect(row.map(([t]) => t).join("")).toHaveLength(HEADER_MARK_WIDTH)
  })

  test("blink closes the eyes into the face panel", () => {
    const colors = (rows: any) => rows.flatMap((r: any) => r.flatMap(([, fg, bg]: any) => [fg, bg]))
    expect(colors(headerMarkRows({ blinking: false }))).toContain(EYE)
    expect(colors(headerMarkRows({ blinking: true }))).not.toContain(EYE)
  })

  test("uses theme keys, not hardcoded colors", () => {
    for (const row of headerMarkRows({ blinking: false })) {
      for (const [, fg, bg] of row) {
        expect(fg).not.toMatch(/^#|^rgb/)
        expect(bg ?? "").not.toMatch(/^#|^rgb/)
      }
    }
    expect(CORAL).toBe("gizzi")
  })
})

describe("GIZZI CODE wordmark", () => {
  test("is three rows tall, fixed width, with one coral core", async () => {
    const { wordmarkRows, WORDMARK_WIDTH } = await import("../../src/cli/ui/ink-app/components/welcomeArt")
    const rows = wordmarkRows()
    expect(rows).toHaveLength(3)
    expect(WORDMARK_WIDTH).toBe(51)
    for (const row of rows) expect(row.map(([t]) => t).join("")).toHaveLength(51)
    const coral = rows.flat().filter(([, fg, bg]) => fg === CORAL || bg === CORAL)
    expect(coral.map(([t]) => t).join("")).toHaveLength(1)
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
