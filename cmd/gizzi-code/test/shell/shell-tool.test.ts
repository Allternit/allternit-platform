import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Bus } from "../../src/shared/bus"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"
import { tmpdir } from "../fixture/fixture"

const APP = { app_id: "notes", source: "catalog" as const, ref: "notes@1.0.0", digest: "a".repeat(64) }

function context(ask: (req: any) => Promise<unknown>) {
  return {
    sessionID: "ses_test",
    messageID: "msg_test",
    callID: "call_test",
    agent: "build",
    abort: new AbortController().signal,
    messages: [],
    metadata: () => {},
    ask,
  } as any
}

/** The app side: answers every shell request ok and keeps what it saw. */
async function app() {
  const { ShellBridge } = await import("../../src/runtime/tools/builtins/shell")
  const seen: any[] = []
  const unsubscribe = Bus.subscribe(ShellBridge.Event.Requested, (event) => {
    seen.push(event.properties)
    const id = event.properties.id
    const command_id = event.properties.command?.command_id ?? "none"
    setTimeout(() => void ShellBridge.reply({ requestID: id, result: { ok: true, command_id, state_version: 1 } }), 1)
  })
  return { seen, stop: unsubscribe }
}

async function tool() {
  const { ShellTool } = await import("../../src/runtime/tools/builtins/shell")
  return ShellTool.init()
}

describe("shell install_app check_id", () => {
  test("an ask that resolves with no person's approval refuses install_app and sends nothing to the app", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const t = await tool()
        const shell = await app()
        try {
          // A headless context's ask (selfhost worker, debug agent) resolves without asking anyone.
          const result = await t.execute({ action: "install_app", app_source: APP }, context(async () => {}))
          expect(result.metadata).toMatchObject({ ok: false, error: "approval_required" })
          expect(shell.seen.length).toBe(0)
        } finally {
          shell.stop()
        }
      },
    })
  })

  test("after the person approves, the command carries the approved request's id as check_id", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const t = await tool()
        const shell = await app()
        const asked: any[] = []
        try {
          // What PermissionNext.reply does on "once": publish Replied for the request id, then resolve the ask.
          const approve = async (req: any) => {
            asked.push(req)
            await Bus.publish(PermissionNext.Event.Replied, { sessionID: "ses_test", requestID: req.id, reply: "once" })
          }
          const result = await t.execute({ action: "install_app", app_source: APP }, context(approve))
          expect(result.metadata).toMatchObject({ ok: true, action: "install_app" })
          expect(asked.length).toBe(1)
          expect(asked[0].id).toMatch(/^per/)
          expect(asked[0].metadata.check_id).toBe(asked[0].id)
          expect(shell.seen[0].command.check_id).toBe(asked[0].id)
        } finally {
          shell.stop()
        }
      },
    })
  })
})
