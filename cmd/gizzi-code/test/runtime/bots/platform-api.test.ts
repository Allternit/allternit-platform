import { afterAll, afterEach, describe, expect, test } from "bun:test"
import { Pairing } from "../../../src/runtime/services/pairing/pairing"

// A stale ALLTERNIT_API_TOKEN left in a shell must not hide a good
// `gizzi login` device token. Pairing.load is stubbed (no files touched:
// Global.Path.data may already point at a real data dir in a shared run).
const realFetch = globalThis.fetch
const realLoad = Pairing.load
const loginToken = "allternit_runtime_good"
;(Pairing as { load: typeof Pairing.load }).load = async () =>
  ({
    version: 1, name: "test", runtimeType: "desktop", hostname: "h", platform: "p",
    publicKey: "pk", publicKeyFingerprint: "fp", privateKey: "sk",
    deviceToken: loginToken, tokenExpiresAt: new Date(Date.now() + 86_400_000).toISOString(),
  }) as Awaited<ReturnType<typeof Pairing.load>>

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })

afterEach(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_API_TOKEN
  delete process.env.GIZZI_PAIR_VIA
})
afterAll(() => {
  ;(Pairing as { load: typeof Pairing.load }).load = realLoad
})

describe("platform credentials", () => {
  test("a rejected env token falls back to the gizzi login token", async () => {
    process.env.ALLTERNIT_API_TOKEN = "stale-token"
    const seen: string[] = []
    globalThis.fetch = (async (_url: string, init: RequestInit = {}) => {
      const auth = String(new Headers(init.headers).get("Authorization"))
      seen.push(auth)
      return auth === `Bearer ${loginToken}` ? json({ agents: [] }) : json({ error: "Unauthorized", message: "Invalid token" }, 401)
    }) as typeof fetch
    const { platformRequest } = await import("../../../src/runtime/bots/platform-api")
    expect(await platformRequest<{ agents: unknown[] }>("GET", "/api/v1/agents")).toEqual({ agents: [] })
    expect(seen).toEqual(["Bearer stale-token", `Bearer ${loginToken}`])
  })

  test("a good env token is used alone", async () => {
    process.env.ALLTERNIT_API_TOKEN = "good-env"
    const seen: string[] = []
    globalThis.fetch = (async (_url: string, init: RequestInit = {}) => {
      seen.push(String(new Headers(init.headers).get("Authorization")))
      return json({ ok: true })
    }) as typeof fetch
    const { platformRequest } = await import("../../../src/runtime/bots/platform-api")
    await platformRequest("GET", "/api/v1/agents")
    expect(seen).toEqual(["Bearer good-env"])
  })

  // Regression (2026-11-12): `gizzi teleport` sent only the first credential, so a stale
  // ALLTERNIT_API_TOKEN failed every teleport with "Invalid token" while other commands worked.
  test("fetchWithPlatformToken falls back too, and resends a byte body", async () => {
    process.env.ALLTERNIT_API_TOKEN = "stale-token"
    const seen: { auth: string; body: number }[] = []
    globalThis.fetch = (async (_url: string, init: RequestInit = {}) => {
      const auth = String(new Headers(init.headers).get("Authorization"))
      seen.push({ auth, body: (init.body as Uint8Array).byteLength })
      return auth === `Bearer ${loginToken}` ? json({ id: "tp_1" }) : json({ message: "Invalid token" }, 401)
    }) as typeof fetch
    const { fetchWithPlatformToken } = await import("../../../src/runtime/bots/platform-api")
    const res = await fetchWithPlatformToken("https://example.test/api/v1/teleports/to/x/teleports/1/files/0", {
      method: "PUT",
      headers: { "Content-Type": "application/octet-stream" },
      body: new Uint8Array([1, 2, 3]),
    })
    expect(res?.status).toBe(200)
    expect(seen).toEqual([{ auth: "Bearer stale-token", body: 3 }, { auth: `Bearer ${loginToken}`, body: 3 }])
  })

  test("gizzi login approves in Allternit Desktop when asked to", () => {
    process.env.GIZZI_PAIR_VIA = "desktop"
    expect(Pairing.desktopApproval()).toBe(true)
    expect(Pairing.desktopApprovalLink("ABCD-1234")).toBe("allternit://pair?code=ABCD-1234")
    process.env.GIZZI_PAIR_VIA = "browser"
    expect(Pairing.desktopApproval()).toBe(false)
    // Default: the browser, even with Desktop installed.
    delete process.env.GIZZI_PAIR_VIA
    expect(Pairing.desktopApproval()).toBe(false)
  })
})
