import { describe, expect, test, spyOn, afterEach, beforeEach } from "bun:test"
import {
  ArtifactCheckTool,
  STALL_CHECKS,
  recordRound,
  resetCheckRounds,
  resolveReference,
} from "../../src/runtime/tools/builtins/artifact-check"
import { PaneRender } from "../../src/runtime/integrations/pane-render"
import { Artifacts } from "../../src/runtime/integrations/artifacts"

const ctx = {
  sessionID: "ses_test",
  messageID: "msg_test",
  callID: "call_test",
  agent: "test-agent",
  abort: AbortSignal.any([]),
  messages: [],
  metadata: () => {},
  ask: async () => {},
} as unknown as Parameters<Awaited<ReturnType<typeof ArtifactCheckTool.init>>["execute"]>[1]

describe("tool.artifact_check", () => {
  const spies: Array<{ mockRestore(): void }> = []
  beforeEach(() => resetCheckRounds())
  afterEach(() => spies.splice(0).forEach((s) => s.mockRestore()))

  test("sends the body to the app and reports what it saw", async () => {
    spies.push(
      spyOn(Artifacts, "read").mockResolvedValue({
        id: "art_1", kind: "page", title: "Landing", version: 3, body: "<h1>x</h1>", body_format: "text/html", meta: {}, persisted: true,
      }),
    )
    const render = spyOn(PaneRender, "request").mockResolvedValue({
      ok: true,
      checks: [
        { width: 390, height: 844, image: "data:image/jpeg;base64,AA==", blank: false, overflowX: 120, errors: ["Uncaught ReferenceError: x"] },
        { width: 1440, height: 900, image: "data:image/jpeg;base64,AA==", blank: false, overflowX: 0, errors: [] },
      ],
    })
    spies.push(render)
    const tool = await ArtifactCheckTool.init()
    const result = await tool.execute({ id: "art_1", widths: [390, 1440] }, ctx)
    expect(render).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "check", code: "<h1>x</h1>", artifactKind: "page", widths: [390, 1440] }),
      expect.anything(),
    )
    expect(result.title).toBe("Check: Landing — 2 problems")
    expect(result.output).toContain("390×844: scrolls sideways by 120px; error: Uncaught ReferenceError: x.")
    expect(result.output).toContain("1440×900: no problems found.")
    expect(result.attachments?.map((a) => a.filename)).toEqual(["Landing 390px.jpg", "Landing 1440px.jpg"])
  })

  test("says stop after checks stop improving, and not while they improve", () => {
    expect(recordRound("k", 4, 0.5).stalled).toBe(false)
    expect(recordRound("k", 3, 0.6).stalled).toBe(false) // better
    for (let i = 0; i < STALL_CHECKS - 1; i++) expect(recordRound("k", 3, 0.6).stalled).toBe(false)
    expect(recordRound("k", 5, 0.4).stalled).toBe(true) // worse than the best: still no progress
    expect(recordRound("other", 0, 0.95).stalled).toBe(false)
  })

  test("a clean, close result is never told to stop", () => {
    for (let i = 0; i < 12; i++) {
      const r = recordRound("done", 0, 0.93)
      expect(r.stalled || r.capped).toBe(false)
    }
  })

  test("reference 'attachment' takes the latest picture the user attached", async () => {
    const messages = [
      { info: { role: "user" }, parts: [{ type: "file", mime: "image/png", url: "data:image/png;base64,OLD" }] },
      { info: { role: "assistant" }, parts: [{ type: "file", mime: "image/png", url: "data:image/png;base64,MINE" }] },
      { info: { role: "user" }, parts: [{ type: "text", text: "like this" }, { type: "file", mime: "image/jpeg", url: "data:image/jpeg;base64,NEW" }] },
    ] as never
    expect(await resolveReference("attachment", messages)).toBe("data:image/jpeg;base64,NEW")
    await expect(resolveReference("attachment", [])).rejects.toThrow("hasn't attached a picture")
    await expect(resolveReference("/tmp/notes.txt", [])).rejects.toThrow("png, jpg, webp or gif")
  })
})
