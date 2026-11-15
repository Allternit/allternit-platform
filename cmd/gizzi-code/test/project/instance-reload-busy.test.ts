import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { tmpdir } from "../fixture/fixture"

// Regression: saving global settings reloaded every instance at once, which
// aborted running turns and erased questions waiting for an answer.
describe("Instance.disposeAll", () => {
  test("a busy instance keeps its state until it goes idle, then reloads", async () => {
    const tmp = await tmpdir({ git: true })
    let busy = true
    const unregister = Instance.registerBusyCheck(() => busy)
    const store = Instance.state(() => ({ id: Math.random() }))
    try {
      const before = await Instance.provide({ directory: tmp.path, fn: () => store().id })
      await Instance.disposeAll()
      expect(await Instance.provide({ directory: tmp.path, fn: () => store().id })).toBe(before)

      busy = false
      await new Promise((r) => setTimeout(r, 2_600))
      expect(await Instance.provide({ directory: tmp.path, fn: () => store().id })).not.toBe(before)
    } finally {
      unregister()
    }
  }, 15_000)

  test("an idle instance reloads at once", async () => {
    const tmp = await tmpdir({ git: true })
    const store = Instance.state(() => ({ id: Math.random() }))
    const before = await Instance.provide({ directory: tmp.path, fn: () => store().id })
    await Instance.disposeAll()
    expect(await Instance.provide({ directory: tmp.path, fn: () => store().id })).not.toBe(before)
  })
})
