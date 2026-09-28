import { describe, expect, test } from "bun:test"
import { PaneBrowser } from "../../src/runtime/integrations/pane-browser"
import { Instance } from "../../src/runtime/context/project/instance"
import { tmpdir } from "../fixture/fixture"

describe("PaneBrowser", () => {
  test("the app's reply resolves the request", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const pending = PaneBrowser.request({ sessionID: "ses_test", action: "read" })
        const [req] = await PaneBrowser.list()
        expect(req.action).toBe("read")
        expect(await PaneBrowser.reply({ requestID: req.id, result: { ok: true, url: "https://a.com", text: "Hello" } })).toBe(true)
        expect(await pending).toEqual({ ok: true, url: "https://a.com", text: "Hello" })
        expect(await PaneBrowser.list()).toHaveLength(0)
      },
    })
  })

  test("times out with a clear error when no app answers", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const result = await PaneBrowser.request({ sessionID: "ses_test", action: "read" }, { timeoutMs: 10 })
        expect(result.ok).toBe(false)
        expect(result.error).toContain("didn't answer")
        expect(await PaneBrowser.list()).toHaveLength(0)
      },
    })
  })

  test("a cancelled turn ends the request", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const controller = new AbortController()
        const pending = PaneBrowser.request({ sessionID: "ses_test", action: "click", target: "#go" }, { abort: controller.signal })
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
        expect(await PaneBrowser.reply({ requestID: "que_nope", result: { ok: true } })).toBe(false)
      },
    })
  })
})
