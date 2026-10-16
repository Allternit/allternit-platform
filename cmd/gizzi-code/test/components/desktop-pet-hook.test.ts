import { afterAll, describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

const dir = mkdtempSync(join(tmpdir(), "pet-hook-"))
process.env.GIZZI_DESKTOP_PET_FILE = join(dir, "desktop-companion.json")

const { desktopPetHookPath, desktopPetSignalFor, reportDesktopPet, sayOnDesktopPet } = await import(
  "../../src/cli/ui/ink-app/pet/desktopPetHook"
)

const received: Array<{ path: string; auth: string | null; body: unknown }> = []
const server = Bun.serve({
  port: 0,
  hostname: "127.0.0.1",
  async fetch(req) {
    received.push({ path: new URL(req.url).pathname, auth: req.headers.get("authorization"), body: await req.json() })
    return Response.json({ ok: true })
  },
})
afterAll(() => {
  server.stop(true)
  rmSync(dir, { recursive: true, force: true })
})
const settle = () => new Promise((r) => setTimeout(r, 100))

describe("desktop pet hook", () => {
  test("maps REPL state to pet signals", () => {
    expect(desktopPetSignalFor({ isLoading: false, toolRunning: false, waiting: false })).toBe("idle")
    expect(desktopPetSignalFor({ isLoading: true, toolRunning: false, waiting: false })).toBe("thinking")
    expect(desktopPetSignalFor({ isLoading: true, toolRunning: true, waiting: false })).toBe("tool")
    expect(desktopPetSignalFor({ isLoading: true, toolRunning: true, waiting: true })).toBe("waiting")
  })

  test("sends nothing without Desktop's hook file", async () => {
    reportDesktopPet("tool")
    await settle()
    expect(received).toEqual([])
  })

  test("never sends the token anywhere but loopback", async () => {
    writeFileSync(desktopPetHookPath(), JSON.stringify({ url: "http://example.com:80", token: "t" }))
    reportDesktopPet("tool")
    await settle()
    expect(received).toEqual([])
  })

  test("posts states and bubbles with the token", async () => {
    writeFileSync(desktopPetHookPath(), JSON.stringify({ version: 1, url: `http://127.0.0.1:${server.port}`, port: server.port, token: "abc" }))
    reportDesktopPet("tool")
    sayOnDesktopPet("Tests pass")
    await settle()
    expect(received).toContainEqual({ path: "/state", auth: "Bearer abc", body: { state: "tool", client: "gizzi" } })
    expect(received).toContainEqual({ path: "/bubble", auth: "Bearer abc", body: { text: "Tests pass", client: "gizzi" } })
  })

  test("GIZZI_DESKTOP_PET_HOOK=0 turns it off", async () => {
    received.length = 0
    process.env.GIZZI_DESKTOP_PET_HOOK = "0"
    reportDesktopPet("tool")
    await settle()
    delete process.env.GIZZI_DESKTOP_PET_HOOK
    expect(received).toEqual([])
  })
})
