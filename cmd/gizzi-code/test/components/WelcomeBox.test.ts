import { describe, test, expect } from "bun:test"
import { WelcomeBox, headerArtMode, headerCanAnimate, headerMascotSize, idleFrame, welcomeRows } from "../../src/cli/ui/ink-app/components/WelcomeBox"
import { CORAL, EYE, FONT35, FONT45, brailleRows, mascotRows, wordmarkRows } from "../../src/cli/ui/ink-app/components/welcomeArt"

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

describe("braille wordmark", () => {
  test("GIZZI CODE in 2 rows of one-column braille; coral G is its own run", async () => {
    const { stringWidth } = await import("../../src/cli/ui/ink-app/ink/stringWidth")
    const rows = wordmarkRows(FONT45, true)
    expect(rows).toHaveLength(2)
    expect(rows[0]![0]![1]).toBe(CORAL)
    for (const row of rows) {
      const line = row.map(([t]) => t).join("")
      expect(line).toMatch(/^[\u2800-\u28ff ]+$/)
      expect(stringWidth(line)).toBe(line.length)
      expect(line).toHaveLength(rows[0]!.map(([t]) => t).join("").length)
    }
    expect(wordmarkRows(FONT35, false)[0]).toHaveLength(1)
    // FONT45 G: top row .### -> dots in the first two cells' upper rows
    expect(brailleRows(["##", "#.", "#.", ".#"])).toEqual([String.fromCharCode(0x2800 + 0x01 + 0x02 + 0x04 + 0x08 + 0x80)])
  })
})

describe("header modes", () => {
  test("env beats the setting; junk means auto", () => {
    expect(headerArtMode({}, undefined)).toBe("auto")
    expect(headerArtMode({}, "blocks")).toBe("blocks")
    expect(headerArtMode({ GIZZI_HEADER_ART: "IMAGE" }, "blocks")).toBe("image")
    expect(headerArtMode({ GIZZI_HEADER_ART: "sparkles" }, undefined)).toBe("auto")
  })

  test("no animation under reduced motion, CI, NO_COLOR or a non-TTY", () => {
    expect(headerCanAnimate({}, true, false)).toBe(true)
    expect(headerCanAnimate({}, false, false)).toBe(false)
    expect(headerCanAnimate({}, true, true)).toBe(false)
    expect(headerCanAnimate({ GIZZI_REDUCED_MOTION: "1" }, true, false)).toBe(false)
    expect(headerCanAnimate({ CI: "true" }, true, false)).toBe(false)
    expect(headerCanAnimate({ CI: "0" }, true, false)).toBe(true)
    expect(headerCanAnimate({ NO_COLOR: "" }, true, false)).toBe(false)
  })

  test("small terminals get the tiny mascot, very narrow ones none", () => {
    expect(headerMascotSize(120, 40)).toBe("mid")
    expect(headerMascotSize(59, 40)).toBe("tiny")
    expect(headerMascotSize(120, 19)).toBe("tiny")
    expect(headerMascotSize(35, 40)).toBeNull()
  })

  test("idle: eyes close once every 4 s; the beacon pulse rises and falls", () => {
    const blinks = Array.from({ length: 40 }, (_, i) => idleFrame(i * 200).blinking).filter(Boolean)
    expect(blinks).toHaveLength(2)
    expect(idleFrame(0).pulse).toBe(0)
    expect(idleFrame(1200).pulse).toBe(1)
  })
})

describe("header image", () => {
  test("8 rows reserved at the mascot's width; nothing without image support unless forced", async () => {
    const { setInlineImageProtocolForTest, inlineImageBlock, inlineImageCellOutput } = await import("../../src/cli/ui/ink-app/ink/inlineImage")
    const { stringWidth } = await import("../../src/cli/ui/ink-app/ink/stringWidth")
    setInlineImageProtocolForTest("iterm")
    const block = inlineImageBlock("t-mascot", "AAAA", 15, 8)!
    expect(block).toHaveLength(8)
    for (const row of block) expect(stringWidth(row)).toBe(15)
    expect(inlineImageCellOutput(block[0]![0]!)).toContain("width=15;height=8;")
    setInlineImageProtocolForTest(null)
    expect(inlineImageBlock("t-mascot-none", "AAAA", 15, 8)).toBeNull()
    setInlineImageProtocolForTest(undefined)
  })
})

describe("half-block mascot", () => {
  const text = (rows: any[]) => rows.map(r => r.map(([t]: any) => t).join(""))
  test("mid: 18 x 8 half blocks in the four mascot colors, never quarter blocks", () => {
    const rows = mascotRows("mid")
    expect(rows).toHaveLength(8)
    for (const line of text(rows)) {
      expect(line).toHaveLength(18)
      expect(line).toMatch(/^[ ▀▄█]+$/)
    }
    const colors = new Set(rows.flat().flatMap(([, fg, bg]) => [fg, bg]).filter(Boolean))
    expect([...colors].sort()).toEqual([CORAL, EYE, "gizziSand", "gizziVisor"].sort())
  })

  test("tiny: 10 x 3", () => {
    const lines = text(mascotRows("tiny"))
    expect(lines).toHaveLength(3)
    for (const line of lines) expect(line).toHaveLength(10)
  })

  test("blink hides the eyes; the pulse recolors the beacon", () => {
    const has = (rows: any[], c: string) => rows.some(r => r.some(([, fg, bg]: any) => fg === c || bg === c))
    expect(has(mascotRows("mid", { blinking: true }), EYE)).toBe(false)
    expect(has(mascotRows("mid", { coral: "gizziShimmer" }), "gizziShimmer")).toBe(true)
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
