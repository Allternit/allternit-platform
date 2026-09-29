import { describe, expect, test } from "bun:test"
import { PaneArtifact } from "../../src/runtime/integrations/pane-artifact"
import { Instance } from "../../src/runtime/context/project/instance"
import { tmpdir } from "../fixture/fixture"

describe("PaneArtifact", () => {
  test("the app's reply resolves the request", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const pending = PaneArtifact.request({ sessionID: "ses_test", action: "describe" })
        const [req] = await PaneArtifact.list()
        expect(req.action).toBe("describe")
        expect(await PaneArtifact.reply({ requestID: req.id, result: { ok: true, artifact: "Document \"Plan\"", text: "Tools" } })).toBe(true)
        expect(await pending).toEqual({ ok: true, artifact: "Document \"Plan\"", text: "Tools" })
        expect(await PaneArtifact.list()).toHaveLength(0)
      },
    })
  })

  test("times out with a clear error when no app answers", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const result = await PaneArtifact.request({ sessionID: "ses_test", action: "describe" }, { timeoutMs: 10 })
        expect(result.ok).toBe(false)
        expect(result.error).toContain("didn't answer")
        expect(await PaneArtifact.list()).toHaveLength(0)
      },
    })
  })

  test("a cancelled turn ends the request", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const controller = new AbortController()
        const pending = PaneArtifact.request({ sessionID: "ses_test", action: "call", tool: "insert_content", input: { html: "<p>x</p>" } }, { abort: controller.signal })
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
        expect(await PaneArtifact.reply({ requestID: "que_nope", result: { ok: true } })).toBe(false)
      },
    })
  })

  test("a picture in the reply reaches the model as an image attachment", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const { PaneArtifactTool } = await import("../../src/runtime/tools/builtins/pane-artifact")
        const tool = await PaneArtifactTool.init()
        const image = "data:image/png;base64,iVBORw0KGgo="
        const run = tool.execute(
          { action: "call", tool: "screenshot_output", input: {} },
          { sessionID: "ses_test", messageID: "", callID: "", agent: "build", abort: AbortSignal.any([]), messages: [], metadata: () => {}, ask: async () => {} } as any,
        )
        let req
        for (let i = 0; i < 50 && !req; i++) {
          ;[req] = await PaneArtifact.list()
          if (!req) await new Promise((r) => setTimeout(r, 10))
        }
        await PaneArtifact.reply({ requestID: req!.id, result: { ok: true, artifact: "Build \"Site\"", text: "Rendered", image } })
        const result = (await run) as any
        expect(result.attachments).toEqual([{ type: "file", mime: "image/png", url: image, filename: "screenshot_output.png" }])
      },
    })
  })

  test("a reply whose image isn't a data URL is refused", () => {
    expect(PaneArtifact.Result.safeParse({ ok: true, image: "https://x/y.png" }).success).toBe(false)
  })
})
