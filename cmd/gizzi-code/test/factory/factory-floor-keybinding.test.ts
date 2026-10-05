import { describe, expect, test } from "bun:test"
import { DEFAULT_BINDINGS } from "../../src/cli/ui/tui/keybindings/defaultBindings"
import { parseBindings } from "../../src/cli/ui/tui/keybindings/parser"
import { resolveKeyWithChordState } from "../../src/cli/ui/tui/keybindings/resolver"

const bindings = parseBindings(DEFAULT_BINDINGS as never)
const k = (over: Record<string, boolean> = {}) =>
  ({ ctrl: false, meta: false, shift: false, super: false, escape: false, ...over }) as never

describe("factory floor keybindings", () => {
  test("ctrl+x f at the prompt opens the factory floor", () => {
    const first = resolveKeyWithChordState("x", k({ ctrl: true }), ["Chat", "Global"] as never, bindings, null)
    expect(first.type).toBe("chord_started")
    const second = resolveKeyWithChordState("f", k(), ["Chat", "Global"] as never, bindings, (first as any).pending)
    expect(second).toMatchObject({ type: "match", action: "app:openFactory" })
  })

  test("ctrl+x is not a chord prefix outside the prompt (dashboard uses it to stop)", () => {
    const r = resolveKeyWithChordState("x", k({ ctrl: true }), ["Dashboard", "Global"] as never, bindings, null)
    expect(r.type).not.toBe("chord_started")
  })

  test("q and Esc leave the factory floor", () => {
    for (const [input, key] of [["q", {}], ["", { escape: true }]] as const) {
      const r = resolveKeyWithChordState(input, k(key), ["Factory", "Global"] as never, bindings, null)
      expect(r).toMatchObject({ type: "match", action: "factory:exit" })
    }
  })
})
