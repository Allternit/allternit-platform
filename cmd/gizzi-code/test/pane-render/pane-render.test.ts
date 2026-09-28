import { describe, expect, test } from "bun:test"
import { PaneRender } from "../../src/runtime/integrations/pane-render"
import { Instance } from "../../src/runtime/context/project/instance"
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

  test("unknown replies are refused", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        expect(await PaneRender.reply({ requestID: "que_nope", result: { ok: true } })).toBe(false)
      },
    })
  })
})
