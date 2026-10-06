// @ts-nocheck
// Verifies the Pty namespace against a live Allternit Factory pane engine.
// Needs the allternit-factory binary (ALLTERNIT_FACTORY_BIN, next to gizzi, on
// PATH, or a monorepo target/ build); `pane tty ensure` starts the engine.
// Run it with a scratch HOME so it gets its own engine:
//   HOME=$(mktemp -d) ALLTERNIT_FACTORY_BIN=… bun test test/pty/pane-engine.test.ts
import { describe, expect, test } from "bun:test"
import { Pty } from "@/runtime/integrations/pty"
import { Instance } from "@/runtime/context/project/instance"

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

async function until(check: () => boolean | Promise<boolean>, ms = 5000) {
  const deadline = Date.now() + ms
  while (Date.now() < deadline) {
    if (await check()) return true
    await sleep(50)
  }
  return false
}

const bin = Pty.factoryBinary()
if (!bin) console.warn("pane-engine.test: allternit-factory not found; set ALLTERNIT_FACTORY_BIN to run it")

describe("pty (pane engine)", () => {
  test.skipIf(!bin)("create/list/get/write/resize/connect/remove", async () => {
    await Instance.provide({
      directory: process.cwd(),
      fn: async () => {
        const info = await Pty.create({
          command: "/bin/bash",
          title: "pane engine e2e",
          env: { E2E_MARKER: "gizzi-pane-ok" },
        })
        expect(info.status).toBe("running")
        expect(info.id.startsWith("pty")).toBe(true)
        expect(info.pid).toBeGreaterThan(0)

        const listed = await Pty.list()
        expect(listed.some((p) => p.id === info.id)).toBe(true)

        await Pty.write(info.id, "echo $E2E_MARKER\r")
        await Pty.resize(info.id, 100, 40)

        // connect(): replay (includes the earlier echo), cursor frame, live.
        const chunks: string[] = []
        let cursorFrames = 0
        const fakeWs = {
          readyState: 1,
          send: (d: any) => (typeof d === "string" ? chunks.push(d) : cursorFrames++),
          close: () => {},
        }
        const conn = await Pty.connect(info.id, fakeWs as any)
        expect(await until(() => chunks.join("").includes("gizzi-pane-ok"))).toBe(true)
        expect(await until(() => cursorFrames === 1)).toBe(true)

        conn?.onMessage("echo gizzi-pane-$((40+2))\r")
        expect(await until(() => chunks.join("").includes("gizzi-pane-42"))).toBe(true)

        // The resize reached the PTY.
        await Pty.write(info.id, "stty size\r")
        expect(await until(() => chunks.join("").includes("40 100"))).toBe(true)

        // A second client (a reconnect) gets the same scrollback.
        const again: string[] = []
        const conn2 = await Pty.connect(info.id, { readyState: 1, send: (d: any) => typeof d === "string" && again.push(d), close: () => {} } as any)
        expect(await until(() => again.join("").includes("gizzi-pane-42"))).toBe(true)
        conn?.onClose()
        conn2?.onClose()

        expect((await Pty.get(info.id))?.status).toBe("running")
        await Pty.remove(info.id)
        expect(await Pty.get(info.id)).toBeUndefined()
      },
    })
  }, 30_000)

  test.skipIf(!bin)("exit is reported with its code", async () => {
    await Instance.provide({
      directory: process.cwd(),
      fn: async () => {
        const info = await Pty.create({ command: "/bin/bash", args: ["--noprofile", "--norc"], title: "exit" })
        await Pty.write(info.id, "exit 7\r")
        expect(await until(async () => (await Pty.get(info.id))?.status === "exited")).toBe(true)
        await Pty.remove(info.id)
      },
    })
  }, 30_000)
})
