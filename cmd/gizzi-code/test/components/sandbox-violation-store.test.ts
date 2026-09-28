import { describe, expect, test } from "bun:test"

// ctrl+o (transcript view) mounts SandboxViolationExpandedView, which calls
// store.subscribe/getTotalCount. The stub store lacked both and the throw
// took the whole TUI down.
describe("sandbox violation store", () => {
  test("has the shape the transcript view and footer hint use", async () => {
    const { SandboxManager } = await import("../../src/cli/ui/ink-app/utils/sandbox/sandbox-adapter")
    const store = SandboxManager.getSandboxViolationStore()
    expect(store.getTotalCount()).toBe(0)
    expect(store.getViolations()).toEqual([])
    const unsubscribe = store.subscribe(() => {})
    expect(typeof unsubscribe).toBe("function")
    unsubscribe()
  })
})
