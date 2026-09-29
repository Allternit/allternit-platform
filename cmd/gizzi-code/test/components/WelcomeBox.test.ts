import { describe, test, expect } from "bun:test"
import { WelcomeBox, welcomeRows } from "../../src/cli/ui/ink-app/components/WelcomeBox"
import { CORAL, EYE } from "../../src/cli/ui/ink-app/components/welcomeArt"

describe("WelcomeBox", () => {
  test("exports a component", () => {
    expect(typeof WelcomeBox).toBe("function")
  })
})

describe("header rows", () => {
  test("a /bots chat names the bot and its pinned model, not this terminal's", () => {
    const chat = { botName: "live-check", model: "claude-cli/claude-opus-5" } as any
    expect(welcomeRows("~/p", "ses_1", "local-mlx/gemma", chat).map(([l, v]) => `${l}=${v}`)).toEqual([
      "Directory=~/p",
      "Session=ses_1",
      "Bot=live-check",
      "Model=claude-cli/claude-opus-5",
    ])
    expect(welcomeRows("~/p", "ses_1", "local-mlx/gemma", { ...chat, model: null }).at(-1)![1]).toBe("platform default model")
    expect(welcomeRows("~/p", "ses_1", "local-mlx/gemma", null).map(([l]) => l)).toEqual(["Directory", "Session", "Model"])
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
  test("the mascot in its own colors, 8 wide and 4 rows", async () => {
    const { textMarkRows } = await import("../../src/cli/ui/ink-app/components/welcomeArt")
    const rows = textMarkRows()
    expect(rows).toHaveLength(4)
    for (const row of rows) expect(row.map(([t]) => t).join("")).toHaveLength(8)
    const colors = new Set(rows.flat().flatMap(([, fg, bg]) => [fg, bg]).filter(Boolean))
    expect([...colors].sort()).toEqual([CORAL, "gizziEye", "gizziSand", "gizziVisor"].sort())
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

describe("256-color downgrade", () => {
  test("keeps the Gizzi colors warm instead of rounding them to pink or olive", async () => {
    const { nearestAnsi256 } = await import("../../src/cli/ui/ink-app/ink/colorize")
    expect(nearestAnsi256(212, 176, 140)).toBe(180) // sand -> (215,175,135)
    expect(nearestAnsi256(181, 151, 111)).toBe(137) // face panel -> (175,135,95)
    expect(nearestAnsi256(217, 119, 87)).toBe(173) // coral -> (215,135,95)
    expect(nearestAnsi256(17, 19, 24)).toBe(233) // eye -> gray ramp
  })
})
