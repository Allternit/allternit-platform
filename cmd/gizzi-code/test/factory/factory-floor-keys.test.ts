import { describe, expect, test } from "bun:test"
import { handleFactoryFloorKey, type FactoryFloorKeyHandlers } from "../../src/cli/ui/tui/screens/factory-floor/keys"
import { buildAgentRow } from "../../src/cli/ui/tui/screens/factory-floor/rows"

function spy() {
  const calls: string[] = []
  const h = new Proxy({} as FactoryFloorKeyHandlers, { get: (_t, name) => () => calls.push(String(name)) })
  return { calls, h }
}

describe("factory floor keys", () => {
  test.each([
    ["a", {}, "approveSelected"],
    ["w", {}, "openWall"],
    ["r", {}, "refresh"],
    ["q", {}, "exit"],
    ["k", {}, "moveUp"],
    ["j", {}, "moveDown"],
    ["", { return: true }, "openSelected"],
    ["", { tab: true }, "switchFocus"],
    ["", { escape: true }, "exit"],
  ])("%j %j → %s", (input, key, expected) => {
    const { calls, h } = spy()
    expect(handleFactoryFloorKey(input, key, h)).toBe(true)
    expect(calls).toEqual([expected])
  })

  test("modifier chords and unknown keys stay unclaimed", () => {
    const { calls, h } = spy()
    expect(handleFactoryFloorKey("a", { ctrl: true }, h)).toBe(false)
    expect(handleFactoryFloorKey("z", {}, h)).toBe(false)
    expect(calls).toEqual([])
  })
})

describe("factory floor rows", () => {
  test("badges, state and proof; missing values are a dash, never invented", () => {
    expect(
      buildAgentRow({
        id: "a",
        address: "coder@core",
        binding: { type: "terminal", harness: "codex" },
        state: "needs_you",
        currentNode: { dagId: "d", nodeId: "n", title: "Fix login" },
        proof: { proven: 2, total: 5 },
      }),
    ).toEqual({ address: "coder@core", badge: "Terminal · Codex", state: "needs you", stateColor: "warning", node: "Fix login", proof: "2/5" })
    expect(buildAgentRow({ id: "b" })).toMatchObject({ badge: "—", state: "—", node: "—", proof: "—" })
  })
})
