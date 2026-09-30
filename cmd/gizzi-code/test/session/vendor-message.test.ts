import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { VendorMessage } from "../../src/runtime/session/vendor-message"
import { tmpdir } from "../fixture/fixture"

describe("vendor message append", () => {
  test("appends an attributed assistant message and dedupes by remote_event_id", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const s = await Session.create({ title: "vendor thread" })
        const meta = { vendor: "openai", adapter: "chatgpt", lane: "api", guarantee: "best_effort", remote_event_id: "ev-1" }
        const a = await VendorMessage.append({ sessionID: s.id, text: "hello from the vendor", metadata: meta })
        const b = await VendorMessage.append({ sessionID: s.id, text: "hello from the vendor", metadata: meta })
        expect(a.duplicate).toBe(false)
        expect(b.duplicate).toBe(true)
        expect(b.id).toBe(a.id)
        const msgs = await Session.messages({ sessionID: s.id })
        expect(msgs.length).toBe(1)
        expect(msgs[0].info.role).toBe("assistant")
        const part: any = msgs[0].parts[0]
        expect(part.text).toBe("hello from the vendor")
        expect(part.metadata.source).toBe("vendor")
        expect(part.metadata.remote_event_id).toBe("ev-1")
        await VendorMessage.append({ sessionID: s.id, text: "another", metadata: { ...meta, remote_event_id: "ev-2" } })
        expect((await Session.messages({ sessionID: s.id })).length).toBe(2)
      },
    })
  })
})
