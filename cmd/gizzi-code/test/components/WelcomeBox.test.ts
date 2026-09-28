import { describe, test, expect } from "bun:test"
import { WelcomeBox } from "../../src/cli/ui/ink-app/components/WelcomeBox"
import { CORAL, EYE } from "../../src/cli/ui/ink-app/components/welcomeArt"

describe("WelcomeBox", () => {
  test("exports a component", () => {
    expect(typeof WelcomeBox).toBe("function")
  })
})

describe("header images", () => {
  test("mark block reserves cols x rows; wordmark one row; nothing without image support", async () => {
    const { setInlineImageProtocolForTest, inlineImageBlock, inlineImagePlaceholder, inlineImageCellOutput } = await import("../../src/cli/ui/ink-app/ink/inlineImage")
    const { stringWidth } = await import("../../src/cli/ui/ink-app/ink/stringWidth")
    setInlineImageProtocolForTest("iterm")
    const block = inlineImageBlock("t-mark", "AAAA", 6, 3)!
    expect(block).toHaveLength(3)
    for (const row of block) expect(stringWidth(row)).toBe(6)
    expect(inlineImageCellOutput(block[0]![0]!)).toContain("width=6;height=3;")
    expect(inlineImagePlaceholder("t-wm", "AAAA", 20)!).toHaveLength(20)
    setInlineImageProtocolForTest(null)
    expect(inlineImageBlock("t-mark-none", "AAAA", 6, 3)).toBeNull()
    setInlineImageProtocolForTest(undefined)
  })
})

describe("text-only header mark", () => {
  test("four rows, 12 wide, ink in the text color with coral beacon and nose", async () => {
    const { textMarkRows } = await import("../../src/cli/ui/ink-app/components/welcomeArt")
    const rows = textMarkRows()
    expect(rows).toHaveLength(4)
    for (const row of rows) expect(row.map(([t]) => t).join("")).toHaveLength(12)
    const colors = new Set(rows.flat().flatMap(([, fg, bg]) => [fg, bg]).filter(Boolean))
    expect([...colors].sort()).toEqual(["gizzi", "text"])
  })
})

describe("gizzi pet sprite", () => {
  test("every pose fills the 12x5 companion slot", async () => {
    const { gizziPetRows, GIZZI_PET_WIDTH } = await import("../../src/cli/ui/ink-app/pet/gizziSprite")
    expect(GIZZI_PET_WIDTH).toBe(12)
    for (const pose of ["idle", "blink", "glance", "wink"] as const) {
      const rows = gizziPetRows(pose)
      expect(rows).toHaveLength(5)
      for (const row of rows) expect(row.map(([t]) => t).join("")).toHaveLength(12)
    }
  })

  test("blink hides the eyes; the other poses show them", async () => {
    const { gizziPetRows } = await import("../../src/cli/ui/ink-app/pet/gizziSprite")
    const hasEye = (pose: any) => gizziPetRows(pose).some(r => r.some(([, fg, bg]) => fg === EYE || bg === EYE))
    expect(hasEye("blink")).toBe(false)
    for (const pose of ["idle", "glance", "wink"]) expect(hasEye(pose)).toBe(true)
  })

  test("the beacon takes the pulse color", async () => {
    const { gizziPetRows } = await import("../../src/cli/ui/ink-app/pet/gizziSprite")
    expect(gizziPetRows("idle", "gizziShimmer")[0]!.some(([, fg]) => fg === "gizziShimmer")).toBe(true)
  })
})
