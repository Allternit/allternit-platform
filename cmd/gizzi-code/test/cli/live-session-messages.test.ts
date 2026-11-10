import { describe, expect, test } from "bun:test"
import { toLiveMessages } from "../../src/cli/ui/ink-app/services/liveSession"

const at = (id: string) => new Date(1000 + Number(id)).toISOString()
const user = (id: string, text: string, extra: Record<string, unknown> = {}) =>
  ({ type: "user", uuid: id, timestamp: at(id), message: { content: text }, ...extra }) as any
const reply = (id: string, text: string) =>
  ({ type: "assistant", uuid: id, timestamp: at(id), message: { content: [{ type: "text", text }], model: "m" } }) as any

const texts = (messages: ReturnType<typeof toLiveMessages>) =>
  messages.map((m) => [m.role, m.parts.map((p: any) => p.text).join("")])

describe("toLiveMessages: slash commands", () => {
  test("a local command and its output never sync", () => {
    const messages = [
      user("1", "<command-name>/remote</command-name>\n<command-message>remote</command-message>\n<command-args></command-args>"),
      user("2", "<local-command-stdout>(no content)</local-command-stdout>"),
      user("3", "hello"),
      reply("4", "hi"),
    ]
    expect(texts(toLiveMessages(messages, false))).toEqual([
      ["user", "hello"],
      ["assistant", "hi"],
    ])
  })

  test("a prompt command the model runs syncs as readable text", () => {
    const messages = [
      user("1", "<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>diff</command-args>"),
      user("2", "Review the diff for bugs.", { isMeta: true }),
      reply("3", "looks fine"),
    ]
    expect(texts(toLiveMessages(messages, false))).toEqual([
      ["user", "/review diff"],
      ["assistant", "looks fine"],
    ])
  })

  test("a prompt command still waiting for its first reply shows while the turn is busy", () => {
    const messages = [user("1", "<command-name>/review</command-name>\n<command-args></command-args>")]
    expect(texts(toLiveMessages(messages, true))).toEqual([["user", "/review"]])
    expect(toLiveMessages(messages, false)).toEqual([])
  })
})
