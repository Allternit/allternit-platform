import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { SessionRevert } from "../../src/runtime/session/revert"
import { SessionRoutes } from "../../src/runtime/server/routes/session"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

const model = { providerID: "anthropic", modelID: "claude-sonnet" }

async function userTurn(sessionID: string, text: string) {
  const id = Identifier.ascending("message")
  await Session.updateMessage({
    id,
    sessionID,
    role: "user",
    agent: "build",
    model,
    time: { created: Date.now() },
  } as MessageV2.User)
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text } as MessageV2.TextPart)
  return id
}

async function reply(sessionID: string, parentID: string, text: string, toolOutput?: string) {
  const id = Identifier.ascending("message")
  await Session.updateMessage({
    id,
    sessionID,
    role: "assistant",
    parentID,
    mode: "build",
    agent: "build",
    path: { cwd: "/", root: "/" },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    ...model,
    time: { created: Date.now(), completed: Date.now() },
  } as MessageV2.Assistant)
  if (toolOutput !== undefined) {
    await Session.updatePart({
      id: Identifier.ascending("part"),
      messageID: id,
      sessionID,
      type: "tool",
      callID: "call-1",
      tool: "bash",
      state: {
        status: "completed",
        input: { command: "cat big.log" },
        output: toolOutput,
        title: "cat big.log",
        metadata: {},
        time: { start: Date.now(), end: Date.now() },
      },
    } as MessageV2.ToolPart)
  }
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text } as MessageV2.TextPart)
  return id
}

const textOf = (msg: MessageV2.WithParts) =>
  msg.parts
    .filter((p) => p.type === "text")
    .map((p) => (p as MessageV2.TextPart).text)
    .join("")

describe("revert keeps the replaced turn as a branch", () => {
  test("revert → send → branches has one row with the old reply", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        const firstUser = await userTurn(session.id, "first question")
        await reply(session.id, firstUser, "first answer")
        const retried = await userTurn(session.id, "second question")
        await reply(session.id, retried, "old reply", "x".repeat(20_000))

        // Retry/Edit: revert at the user message, then the next send runs
        // cleanup() before its new turn (SessionPrompt.prompt does exactly this).
        await SessionRevert.revert({ sessionID: session.id, messageID: retried })
        expect(SessionRevert.branches(session.id)).toHaveLength(0)
        await SessionRevert.cleanup(await Session.get(session.id))

        const left = await Session.messages({ sessionID: session.id })
        expect(left.map((m) => m.info.id)).not.toContain(retried)
        expect(left.map(textOf)).toEqual(["first question", "first answer"])
        expect((await Session.get(session.id)).revert).toBeUndefined()

        const branches = SessionRevert.branches(session.id)
        expect(branches).toHaveLength(1)
        const [branch] = branches
        expect(branch.id).toStartWith("brn_")
        expect(branch.anchorMessageId).toBe(retried)
        expect(branch.messages.map((m) => m.info.role)).toEqual(["user", "assistant"])
        expect(branch.messages[0].info.id).toBe(retried)
        expect(textOf(branch.messages[0])).toBe("second question")
        expect(textOf(branch.messages[1])).toBe("old reply")
        const tool = branch.messages[1].parts.find((p) => p.type === "tool") as MessageV2.ToolPart
        const output = (tool.state as MessageV2.ToolStateCompleted).output
        expect(output.startsWith("x".repeat(SessionRevert.BRANCH_OUTPUT_LIMIT))).toBe(true)
        expect(output.length).toBeLessThan(SessionRevert.BRANCH_OUTPUT_LIMIT + 32)
        expect(output).toEndWith("[truncated]")

        // The HTTP route returns the same rows, newest first.
        await userTurn(session.id, "third question").then((id) => reply(session.id, id, "reply v1"))
        const msgs = await Session.messages({ sessionID: session.id })
        await SessionRevert.revert({ sessionID: session.id, messageID: msgs[2].info.id })
        await SessionRevert.cleanup(await Session.get(session.id))

        const res = await SessionRoutes().request(`/${session.id}/branches`)
        expect(res.status).toBe(200)
        const body = (await res.json()) as SessionRevert.Branch[]
        expect(body).toHaveLength(2)
        expect(textOf(body[0].messages[1])).toBe("reply v1")
        expect(textOf(body[1].messages[1])).toBe("old reply")
        expect(Object.keys(body[0]).sort()).toEqual(["anchorMessageId", "createdAt", "id", "messages"])

        // Clearing the session drops its branches with its messages.
        await Session.clear(session.id)
        expect(SessionRevert.branches(session.id)).toHaveLength(0)
      },
    })
  }, 60_000)

  test("nothing to remove writes no branch row", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        await SessionRevert.cleanup(await Session.get(session.id))
        expect(SessionRevert.branches(session.id)).toHaveLength(0)
      },
    })
  })
})
