import { describe, expect, test } from "bun:test"
import { lastUserText } from "../../src/cli/ui/ink-app/services/memoryDrive/capture"

const user = (uuid: string, content: unknown, extra: Record<string, unknown> = {}) =>
  ({ type: "user", uuid, message: { role: "user", content }, ...extra }) as never

describe("Gizzi turn capture", () => {
  test("takes the last real thing the person typed", () => {
    const messages = [
      user("u1", "I use pnpm, never npm"),
      { type: "assistant", uuid: "a1", message: { content: [{ type: "text", text: "ok" }] } } as never,
      user("u2", [{ type: "tool_result", tool_use_id: "t", content: "done" }]),
    ]
    expect(lastUserText(messages)).toEqual({ id: "u1", text: "I use pnpm, never npm" })
  })

  test("skips commands, injected context and meta messages", () => {
    expect(lastUserText([user("c", "/memory")])).toBeNull()
    expect(lastUserText([user("r", "<system-reminder>x</system-reminder>")])).toBeNull()
    expect(lastUserText([user("m", "Remember this", { isMeta: true })])).toBeNull()
    expect(lastUserText([user("b", [{ type: "text", text: "Ship it on Friday" }])])).toEqual({ id: "b", text: "Ship it on Friday" })
  })
})
