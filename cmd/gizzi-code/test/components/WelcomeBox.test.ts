import { describe, test, expect } from "bun:test"
import { WelcomeBox } from "../../src/cli/ui/ink-app/components/WelcomeBox"
import { CORAL, EYE } from "../../src/cli/ui/ink-app/components/welcomeArt"

describe("WelcomeBox", () => {
  test("exports a component", () => {
    expect(typeof WelcomeBox).toBe("function")
  })
})

describe("header lockup", () => {
  test("reserves the lockup image in image-capable terminals, typed name elsewhere", async () => {
    const { setInlineImageProtocolForTest, inlineImagePlaceholder } = await import("../../src/cli/ui/ink-app/ink/inlineImage")
    const { GIZZI_LOCKUP_ASPECT } = await import("../../src/cli/ui/ink-app/components/gizziLockupImage")
    setInlineImageProtocolForTest("kitty")
    expect(inlineImagePlaceholder("t-lockup", "AAAA", Math.round(GIZZI_LOCKUP_ASPECT * 2))).toHaveLength(23)
    setInlineImageProtocolForTest(null)
    expect(inlineImagePlaceholder("t-lockup-none", "AAAA", 23)).toBeNull()
    setInlineImageProtocolForTest(undefined)
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
