import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

// POST /session/import (self-hosted Agents Server rehydration): a transcript in
// gizzi's stored shape becomes a new session whose next turn sees the earlier
// messages, with fresh ids and each assistant reply still linked to its prompt.
describe("Session.importSession", () => {
  test("rebuilds a transcript in order with new ids and remapped parents", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const source = await Session.create({})
        const userID = Identifier.ascending("message")
        await Session.updateMessage({
          id: userID,
          sessionID: source.id,
          role: "user",
          time: { created: Date.now() },
          agent: "build",
          model: { providerID: "p", modelID: "m" },
        } as MessageV2.User)
        await Session.updatePart({
          id: Identifier.ascending("part"),
          sessionID: source.id,
          messageID: userID,
          type: "text",
          text: "echo ok > /workspace/a",
        } as MessageV2.TextPart)
        const assistantID = Identifier.ascending("message")
        await Session.updateMessage({
          id: assistantID,
          sessionID: source.id,
          role: "assistant",
          parentID: userID,
          mode: "build",
          agent: "build",
          path: { cwd: "/", root: "/" },
          cost: 0,
          tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
          modelID: "m",
          providerID: "p",
          time: { created: Date.now() },
        } as MessageV2.Assistant)
        await Session.updatePart({
          id: Identifier.ascending("part"),
          sessionID: source.id,
          messageID: assistantID,
          type: "text",
          text: "Done.",
        } as MessageV2.TextPart)

        const transcript = await Session.messages({ sessionID: source.id })
        const imported = await Session.importSession({ title: "Restored", agentID: "build", messages: transcript })

        expect(imported.id).not.toBe(source.id)
        expect(imported.title).toBe("Restored")
        const restored = await Session.messages({ sessionID: imported.id })
        expect(restored.map((m) => m.info.role)).toEqual(["user", "assistant"])
        expect(restored.map((m) => (m.parts[0] as MessageV2.TextPart).text)).toEqual(["echo ok > /workspace/a", "Done."])
        const [user, assistant] = restored
        expect(user!.info.id).not.toBe(userID)
        expect(assistant!.info.id).not.toBe(assistantID)
        expect((assistant!.info as MessageV2.Assistant).parentID).toBe(user!.info.id)
        for (const m of restored) {
          expect(m.info.sessionID).toBe(imported.id)
          for (const part of m.parts) expect(part.sessionID).toBe(imported.id)
        }
        // The source session is untouched.
        expect((await Session.messages({ sessionID: source.id })).map((m) => m.info.id)).toEqual([userID, assistantID])

        await Session.remove(source.id)
        await Session.remove(imported.id)
      },
    })
  })
})
