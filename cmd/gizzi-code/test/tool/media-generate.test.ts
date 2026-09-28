import { describe, expect, test, spyOn, afterEach } from "bun:test"
import { MediaGenerateTool, videoTimeoutMs } from "../../src/runtime/tools/builtins/media-generate"
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
    expect(video.output).toContain("canvas scene")
    expect(spy).not.toHaveBeenCalled()
  })

  test("video: renders in the app, links the stored MP4 and attaches the contact sheet", async () => {
    spy = spyOn(PaneRender, "request").mockResolvedValue({
      ok: true,
      image: "data:image/png;base64,BB==",
      video: { url: "/api/v1/media/artifacts/abc-1", mime: "video/mp4", bytes: 1234, duration: 4, fps: 30, audio: true },
    })
    const tool = await MediaGenerateTool.init()
    const result = await tool.execute(
      { kind: "video", prompt: "a logo sting", title: "Sting", code: "ctx.fillRect(0,0,t,1)", duration: 4, audio: "ctx.createOscillator()" },
      ctx,
    )
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "video", format: "canvas", width: 1920, height: 1080, duration: 4, fps: 30, motionBlur: 4, audio: "ctx.createOscillator()" }),
      expect.objectContaining({ timeoutMs: videoTimeoutMs(120, 4) }),
    )
    expect(result.output).toStartWith('Rendered "Sting"')
    expect(result.output).toContain("/api/v1/media/artifacts/abc-1")
    expect(result.output).toContain("with sound")
    expect(result.metadata).toMatchObject({ ok: true, lane: "native", video: { url: "/api/v1/media/artifacts/abc-1" } })
    expect(result.attachments?.[0]).toMatchObject({ mime: "image/png", url: "data:image/png;base64,BB==" })
  })

  test("video: clamps to even sides under 4K, known frame rates and 60s", async () => {
    spy = spyOn(PaneRender, "request").mockResolvedValue({ ok: false, error: "The scene threw at t=1.00s: boom" })
    const tool = await MediaGenerateTool.init()
    const result = await tool.execute(
      { kind: "video", prompt: "x", format: "canvas", code: "x", width: 4001, height: 4001, fps: 47, duration: 600, motionBlur: 50 },
      ctx,
    )
    const [req] = spy.mock.calls[0] as [Record<string, number>]
    expect(req.width % 2).toBe(0)
    expect(req.height % 2).toBe(0)
    expect(req.width * req.height).toBeLessThanOrEqual(3840 * 2160)
    expect(req).toMatchObject({ fps: 30, duration: 60, motionBlur: 8 })
    expect(result.output).toContain("The scene threw at t=1.00s")
    expect(result.attachments).toBeUndefined()
  })

  test("video: only canvas scenes", async () => {
    spy = spyOn(PaneRender, "request")
    const tool = await MediaGenerateTool.init()
    await expect(tool.execute({ kind: "video", prompt: "x", format: "svg", code: "<svg/>" }, ctx)).rejects.toThrow("canvas")
    expect(spy).not.toHaveBeenCalled()
  })

  test("video budget grows with the work and caps at 15 minutes", () => {
    expect(videoTimeoutMs(1, 1)).toBeGreaterThanOrEqual(90_000)
    expect(videoTimeoutMs(3600, 8)).toBe(15 * 60_000)
  })
})
