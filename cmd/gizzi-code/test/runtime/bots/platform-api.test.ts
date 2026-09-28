import { afterAll, beforeAll, describe, expect, test } from "bun:test"
import { mkdir, writeFile } from "node:fs/promises"
import { join } from "node:path"
import { tmpdir } from "../../fixture/fixture"

// A stale ALLTERNIT_API_TOKEN left in a shell must not hide a good
// `gizzi login` device token.
let tmp: Awaited<ReturnType<typeof tmpdir>>
const realFetch = globalThis.fetch
beforeAll(async () => {
  tmp = await tmpdir()
  process.env.XDG_DATA_HOME = join(tmp.path, "xdg-data")
  const dataDir = join(process.env.XDG_DATA_HOME, "gizzi-code")
  await mkdir(dataDir, { recursive: true })
  await writeFile(
    join(dataDir, "runtime-device.json"),
    JSON.stringify({
      version: 1, name: "test", runtimeType: "desktop", hostname: "h", platform: "p",
      publicKey: "pk", publicKeyFingerprint: "fp", privateKey: "sk",
      deviceToken: "allternit_runtime_good", tokenExpiresAt: new Date(Date.now() + 86_400_000).toISOString(),
    }),
  )
})
afterAll(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_API_TOKEN
})

describe("platform credentials", () => {
  test("a rejected env token falls back to the gizzi login token", async () => {
    process.env.ALLTERNIT_API_TOKEN = "stale-token"
    const seen: string[] = []
    globalThis.fetch = (async (_url: string, init: RequestInit = {}) => {
      const auth = String((init.headers as Record<string, string>).Authorization)
      seen.push(auth)
      return auth === "Bearer allternit_runtime_good"
        ? Response.json({ agents: [] })
        : Response.json({ error: "Unauthorized", message: "Invalid token" }, { status: 401 })
    }) as typeof fetch
    const { platformRequest } = await import("../../../src/runtime/bots/platform-api")
    expect(await platformRequest("GET", "/api/v1/agents")).toEqual({ agents: [] })
    expect(seen).toEqual(["Bearer stale-token", "Bearer allternit_runtime_good"])
  })

  test("a good env token is used alone", async () => {
    process.env.ALLTERNIT_API_TOKEN = "good-env"
    const seen: string[] = []
    globalThis.fetch = (async (_url: string, init: RequestInit = {}) => {
      seen.push(String((init.headers as Record<string, string>).Authorization))
      return Response.json({ ok: true })
    }) as typeof fetch
    const { platformRequest } = await import("../../../src/runtime/bots/platform-api")
    await platformRequest("GET", "/api/v1/agents")
    expect(seen).toEqual(["Bearer good-env"])
  })

  test("gizzi login approves in Allternit Desktop when asked to", async () => {
    const { Pairing } = await import("../../../src/runtime/services/pairing/pairing")
    process.env.GIZZI_PAIR_VIA = "desktop"
    expect(Pairing.desktopApproval()).toBe(true)
    expect(Pairing.desktopApprovalLink("ABCD-1234")).toBe("allternit://pair?code=ABCD-1234")
    process.env.GIZZI_PAIR_VIA = "browser"
    expect(Pairing.desktopApproval()).toBe(false)
    delete process.env.GIZZI_PAIR_VIA
  })
})
