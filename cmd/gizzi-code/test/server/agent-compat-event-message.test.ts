import { describe, expect, test } from "bun:test"

import { eventMessage } from "../../src/runtime/server/routes/agent-compat"

describe("eventMessage", () => {
  const messages = [{ info: { id: "msg_user" } }, { info: { id: "msg_reply" } }]

  test("a user's message.updated read after the reply row exists still names the user message", () => {
    expect(eventMessage(messages, "msg_user")?.info.id).toBe("msg_user")
  })

  test("falls back to the newest message only when the event names none", () => {
    expect(eventMessage(messages, undefined)?.info.id).toBe("msg_reply")
    expect(eventMessage(messages, "msg_gone")).toBeUndefined()
  })
})
