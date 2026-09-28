import { afterEach, describe, expect, test } from "bun:test"
import {
  detectInlineImageProtocol,
  inlineImageCellOutput,
  inlineImageClear,
  inlineImagePlaceholder,
  INLINE_IMAGE_FILLER,
  setInlineImageProtocolForTest,
} from "../../src/cli/ui/ink-app/ink/inlineImage"
import { stringWidth } from "../../src/cli/ui/ink-app/ink/stringWidth"

afterEach(() => setInlineImageProtocolForTest(undefined))

describe("inline image protocol detection", () => {
  test("image-capable terminals", () => {
    expect(detectInlineImageProtocol({ TERM_PROGRAM: "iTerm.app" })).toBe("iterm")
    expect(detectInlineImageProtocol({ TERM_PROGRAM: "WezTerm" })).toBe("iterm")
    expect(detectInlineImageProtocol({ TERM_PROGRAM: "ghostty" })).toBe("kitty")
    expect(detectInlineImageProtocol({ TERM: "xterm-kitty" })).toBe("kitty")
  })
  test("no images in Apple Terminal, tmux, or when disabled", () => {
    expect(detectInlineImageProtocol({ TERM_PROGRAM: "Apple_Terminal" })).toBeNull()
    expect(detectInlineImageProtocol({ TERM_PROGRAM: "iTerm.app", TMUX: "/tmp/tmux" })).toBeNull()
    expect(detectInlineImageProtocol({ TERM_PROGRAM: "iTerm.app", GIZZI_INLINE_IMAGES: "0" })).toBeNull()
    expect(detectInlineImageProtocol({ TERM_PROGRAM: "Apple_Terminal", GIZZI_INLINE_IMAGES: "kitty" })).toBe("kitty")
  })
})

describe("placeholders", () => {
  test("null when the terminal can't show images", () => {
    setInlineImageProtocolForTest(null)
    expect(inlineImagePlaceholder("t-none", "AAAA", 20)).toBeNull()
  })

  test("reserve exactly `cols` cells; head draws without moving, every cell advances one column", () => {
    setInlineImageProtocolForTest("iterm")
    const ph = inlineImagePlaceholder("t-iterm", "QUJD", 20)!
    expect(stringWidth(ph)).toBe(20)
    const head = inlineImageCellOutput(ph[0]!)
    expect(head.startsWith("\x1b7\x1b]1337;File=inline=1;")).toBe(true)
    expect(head).toContain("width=20;height=1;preserveAspectRatio=1;doNotMoveCursor=1:QUJD\x07")
    expect(head.endsWith("\x1b8\x1b[C")).toBe(true)
    expect(inlineImageCellOutput(INLINE_IMAGE_FILLER)).toBe("\x1b[C")
    expect(inlineImageCellOutput("a")).toBe("a")
  })

  test("kitty payloads are chunked at 4096 bytes and keep the cursor still", () => {
    setInlineImageProtocolForTest("kitty")
    const b64 = "A".repeat(5000)
    const head = inlineImageCellOutput(inlineImagePlaceholder("t-kitty", b64, 20)![0]!)
    expect(head).toMatch(/\x1b_Gf=100,a=T,i=\d+,p=1,c=20,r=1,C=1,q=2,m=1;/)
    expect(head).toContain("\x1b_Gm=0;")
    expect(head.endsWith("\x1b8\x1b[C")).toBe(true)
  })
})

describe("clearing", () => {
  test("kitty deletes the image by id; iTerm2 needs nothing", () => {
    setInlineImageProtocolForTest("kitty")
    const head = inlineImageCellOutput(inlineImagePlaceholder("t-clear", "QUJD", 12, 5)![0]!)
    const id = head.match(/a=T,i=(\d+),p=1/)![1]
    expect(inlineImageClear("t-clear")).toBe(`\x1b_Ga=d,d=I,i=${id},q=2\x1b\\`)
    expect(inlineImageClear("never-registered")).toBe("")
    setInlineImageProtocolForTest("iterm")
    expect(inlineImageClear("t-clear")).toBe("")
  })
})
