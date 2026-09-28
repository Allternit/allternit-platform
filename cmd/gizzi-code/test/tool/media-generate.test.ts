import { describe, expect, test, spyOn, afterEach } from "bun:test"
import { MediaGenerateTool } from "../../src/runtime/tools/builtins/media-generate"
import { PaneRender } from "../../src/runtime/integrations/pane-render"

const ctx = {
  sessionID: "ses_test",
  messageID: "msg_test",
  callID: "call_test",
  agent: "test-agent",
  abort: AbortSignal.any([]),
  messages: [],
  metadata: () => {},
  ask: async () => {},
} as unknown as Parameters<Awaited<ReturnType<typeof MediaGenerateTool.init>>["execute"]>[1]

describe("tool.media_generate", () => {
  let spy: ReturnType<typeof spyOn> | undefined
  afterEach(() => spy?.mockRestore())

  test("native: renders in the app and attaches the PNG", async () => {
    spy = spyOn(PaneRender, "request").mockResolvedValue({ ok: true, image: "data:image/png;base64,AA==" })
    const tool = await MediaGenerateTool.init()
    const result = await tool.execute(
      { kind: "image", prompt: "a red circle", title: "Circle", format: "svg", code: "<svg/>", width: 10, height: 9000 },
      ctx,
    )
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ sessionID: "ses_test", format: "svg", width: 64, height: 4096, title: "Circle" }),
      expect.anything(),
    )
    expect(result.metadata).toMatchObject({ ok: true, lane: "native" })
    expect(result.attachments?.[0]).toMatchObject({ mime: "image/png", url: "data:image/png;base64,AA==" })
  })

  test("a failed render tells the model to fix and retry", async () => {
    spy = spyOn(PaneRender, "request").mockResolvedValue({ ok: false, error: "SVG didn't parse" })
    const tool = await MediaGenerateTool.init()
    const result = await tool.execute({ kind: "image", prompt: "x", format: "svg", code: "<svg" }, ctx)
    expect(result.output).toContain("SVG didn't parse")
    expect(result.attachments).toBeUndefined()
  })

  test("without code or a native lane it explains the lanes instead of guessing", async () => {
    spy = spyOn(PaneRender, "request")
    const tool = await MediaGenerateTool.init()
    const result = await tool.execute({ kind: "image", prompt: "a photo of my dog", lane: "cloud" }, ctx)
    expect(result.output).toContain("cloud lane isn't available")
    expect(result.output).toContain("ask the user")
    expect(spy).not.toHaveBeenCalled()
    const video = await tool.execute({ kind: "video", prompt: "a clip" }, ctx)
    expect(video.output).toContain("isn't available")
  })
})
