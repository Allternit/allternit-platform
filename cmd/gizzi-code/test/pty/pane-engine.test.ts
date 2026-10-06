// @ts-nocheck
// Verifies the mux-backed Pty namespace against a live allternit-mux daemon.
// Requires ALLTERNIT_MUX_STATE_DIR (+ SOCKET) pointing at a running mux.
import { describe, expect, test } from "bun:test"
import { Pty } from "@/runtime/integrations/pty"
import { Instance } from "@/runtime/context/project/instance"

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

describe("pty (mux-backed)", () => {
  test("create/list/get/write/resize/connect/remove", async () => {
    await Instance.provide({
      directory: process.cwd(),
      fn: async () => {
        const info = await Pty.create({
          command: "/bin/bash",
          title: "mux e2e",
          env: { E2E_MARKER: "gizzi-mux-ok" },
        })
        expect(info.status).toBe("running")
        expect(info.id.startsWith("pty")).toBe(true)

        const listed = await Pty.list()
        expect(listed.some((p) => p.id === info.id)).toBe(true)

        await Pty.write(info.id, "echo $E2E_MARKER\n")
        await sleep(800)

        const got = await Pty.get(info.id)
        expect(got?.status).toBe("running")

        await Pty.resize(info.id, 100, 40)

        // connect(): replay + live streaming over a fake ws.
        const chunks: string[] = []
        const fakeWs = {
          readyState: 1,
          send: (d: any) => chunks.push(typeof d === "string" ? d : ""),
          close: () => {},
        }
        const conn = await Pty.connect(info.id, fakeWs as any)
        await sleep(800)
        await Pty.write(info.id, "echo gizzi-mux-live\n")
        await sleep(800)
        const joined = chunks.join("")
        expect(joined.includes("gizzi-mux-ok")).toBe(true)
        expect(joined.includes("gizzi-mux-live")).toBe(true)
        conn?.onClose()

        await Pty.remove(info.id)
        const after = await Pty.get(info.id)
        expect(after).toBeUndefined()
      },
    })
  }, 30_000)
})
