import { describe, expect, test } from "bun:test"
import { PaneRender } from "../../src/runtime/integrations/pane-render"
import { Instance } from "../../src/runtime/context/project/instance"
import { progressForwarder } from "../../src/runtime/tools/builtins/media-generate"
import { tmpdir } from "../fixture/fixture"

describe("PaneRender", () => {
  test("the app's reply resolves the request", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const pending = PaneRender.request({ sessionID: "ses_test", kind: "image", format: "svg", code: "<svg/>", width: 64, height: 64, title: "t" })
        const [req] = await PaneRender.list()
        expect(req.format).toBe("svg")
        expect(await PaneRender.reply({ requestID: req.id, result: { ok: true, image: "data:image/png;base64,AA==" } })).toBe(true)
        expect(await pending).toEqual({ ok: true, image: "data:image/png;base64,AA==" })
        expect(await PaneRender.list()).toHaveLength(0)
      },
    })
  })

  test("times out with a clear error when no app answers", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const result = await PaneRender.request({ sessionID: "ses_test", kind: "image", format: "svg", code: "<svg/>", width: 64, height: 64, title: "t" }, { timeoutMs: 10 })
        expect(result.ok).toBe(false)
        expect(result.error).toContain("No app rendered")
        expect(await PaneRender.list()).toHaveLength(0)
      },
    })
  })

  test("a cancelled turn ends the request", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const controller = new AbortController()
        const pending = PaneRender.request({ sessionID: "ses_test", kind: "image", format: "html", code: "<p>x</p>", width: 64, height: 64, title: "t" }, { abort: controller.signal })
        controller.abort()
        expect(await pending).toEqual({ ok: false, error: "Cancelled." })
      },
    })
  })

  test("a video request carries its own timeout and the reply's video", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const pending = PaneRender.request(
          { sessionID: "ses_test", kind: "video", format: "canvas", code: "x", width: 64, height: 64, duration: 2, fps: 30, title: "v" },
          { timeoutMs: 300_000 },
        )
        const [req] = await PaneRender.list()
        expect(req).toMatchObject({ kind: "video", duration: 2, fps: 30, timeoutMs: 300_000 })
        const video = { url: "/api/v1/media/artifacts/x", mime: "video/mp4", bytes: 10, duration: 2, fps: 30 }
        expect(await PaneRender.reply({ requestID: req.id, result: { ok: true, video } })).toBe(true)
        expect(await pending).toEqual({ ok: true, video })
      },
    })
  })

  test("unknown replies are refused", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        expect(await PaneRender.reply({ requestID: "que_nope", result: { ok: true } })).toBe(false)
      },
    })
  })

  test("progress reports reach the request's listener, and stop when the request ends", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const seen: PaneRender.ProgressDetail[] = []
        const pending = PaneRender.request(
          { sessionID: "ses_test", kind: "video", format: "canvas", code: "", width: 64, height: 64, title: "t" },
          { onProgress: (d) => seen.push(d) },
        )
        const [req] = await PaneRender.list()
        const detail = { lane: "motion", stage: "rendering", label: "Frame 3 of 90", frame: 3, frames: 90 } as const
        expect(await PaneRender.progress({ requestID: req.id, detail })).toBe(true)
        expect(await PaneRender.progress({ requestID: "que_unknown", detail })).toBe(false)
        await PaneRender.reply({ requestID: req.id, result: { ok: false, error: "x" } })
        await pending
        expect(await PaneRender.progress({ requestID: req.id, detail })).toBe(false)
        expect(seen).toEqual([detail])
      },
    })
  })

  test("a progress body is validated", () => {
    expect(PaneRender.ProgressDetail.safeParse({ lane: "picture", stage: "drawing", label: "x", fraction: 1.5 }).success).toBe(false)
    expect(PaneRender.ProgressDetail.safeParse({ lane: "picture", stage: "nope", label: "x" }).success).toBe(false)
    expect(PaneRender.ProgressDetail.safeParse({ lane: "picture", stage: "drawing", label: "x", preview: "javascript:alert(1)" }).success).toBe(false)
    expect(PaneRender.ProgressDetail.safeParse({ lane: "picture", stage: "drawing", label: "x", preview: "data:image/jpeg;base64,AA==" }).success).toBe(true)
  })

  test("progress is sent about once a second, but a new stage goes through at once", () => {
    let t = 0
    const out: string[] = []
    const send = progressForwarder((d) => out.push(`${d.stage}:${d.label}`), { now: () => t })
    const d = (stage: PaneRender.ProgressStage, label: string) => ({ lane: "motion" as const, stage, label })
    send(d("rendering", "a"))
    t = 300
    send(d("rendering", "b"))
    t = 1000
    send(d("rendering", "c"))
    t = 1100
    send(d("encoding", "d"))
    expect(out).toEqual(["rendering:a", "rendering:c", "encoding:d"])
  })
})
