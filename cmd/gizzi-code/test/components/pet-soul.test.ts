import { describe, expect, mock, test } from "bun:test"

// A model call that never settles, like a provider retrying an unreachable
// small model. The pet must give up when its signal fires.
const claudeApi = await import("../../src/cli/ui/ink-app/services/api/claude.js")
mock.module("../../src/cli/ui/ink-app/services/api/claude.js", () => ({
  ...claudeApi,
  queryModelWithoutStreaming: () => new Promise(() => {}),
}))

describe("pet model call", () => {
  test("returns null when aborted even if the request never settles", async () => {
    const { askCompanionModel } = await import("../../src/cli/ui/ink-app/pet/soul")
    const controller = new AbortController()
    setTimeout(() => controller.abort(), 20)
    const started = Date.now()
    expect(await askCompanionModel([], "sys", controller.signal, "companion_hatch")).toBeNull()
    expect(Date.now() - started).toBeLessThan(1000)
  })

  test("returns null immediately for an already-aborted signal", async () => {
    const { askCompanionModel } = await import("../../src/cli/ui/ink-app/pet/soul")
    expect(await askCompanionModel([], "sys", AbortSignal.abort(), "companion_hatch")).toBeNull()
  })
})
