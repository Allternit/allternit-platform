import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import { discoverAllternitDevices } from "../../src/runtime/providers/discovery/allternit-devices"
import { Pairing } from "../../src/runtime/services/pairing/pairing"
import { Instance } from "../../src/runtime/context/project/instance"
import { Provider } from "../../src/runtime/providers/provider"
import { tmpdir } from "../fixture/fixture"

// Phones serving a model through the cloud relay (allternit-cloud-api
// model_hosts.rs). Only the network edge and the pairing file are faked.
const realFetch = globalThis.fetch
const realLoad = Pairing.load
const deviceToken = "allternit_runtime_devices"
let paired = true
;(Pairing as { load: typeof Pairing.load }).load = async () =>
  (paired
    ? {
        version: 1, name: "test", runtimeType: "desktop", hostname: "h", platform: "p",
        publicKey: "pk", publicKeyFingerprint: "fp", privateKey: "sk",
        deviceToken, tokenExpiresAt: new Date(Date.now() + 86_400_000).toISOString(),
      }
    : undefined) as Awaited<ReturnType<typeof Pairing.load>>

type Hosts = { status: number; data?: Array<{ id: string; owned_by?: string; context_length?: number }> }
let hosts: Hosts = { status: 200, data: [] }
let seenAuth: string | null = null

beforeEach(() => {
  paired = true
  hosts = { status: 200, data: [] }
  seenAuth = null
  // The developer's own shell may export these; each test sets what it needs.
  delete process.env.ALLTERNIT_API_KEY
  delete process.env.ALLTERNIT_API_TOKEN
  process.env.ALLTERNIT_CLOUD_API_URL = "http://cloud.test/"
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input instanceof Request ? input.url : input)
    if (url === "http://cloud.test/api/v1/model-hosts/v1/models") {
      seenAuth = new Headers(init?.headers).get("authorization")
      if (hosts.status !== 200) return new Response("down", { status: hosts.status })
      return new Response(JSON.stringify({ object: "list", data: hosts.data }), {
        status: 200,
        headers: { "content-type": "application/json" },
      })
    }
    return realFetch(input, init)
  }) as typeof fetch
})

afterEach(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_CLOUD_API_URL
  delete process.env.ALLTERNIT_API_KEY
  delete process.env.ALLTERNIT_API_TOKEN
})

afterAll(() => {
  ;(Pairing as { load: typeof Pairing.load }).load = realLoad
})

const phone = { id: "phone-1/lfm2.5-350m", owned_by: "iPhone", context_length: 8192 }

describe("discoverAllternitDevices", () => {
  test("signed out: no provider and no request", async () => {
    paired = false
    hosts.data = [phone]
    expect(await discoverAllternitDevices()).toEqual([])
    expect(seenAuth).toBeNull()
  })

  test("lists online hosts as a bearer provider with the paired device token", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_env_key"
    hosts.data = [phone]
    const [provider] = await discoverAllternitDevices()
    expect(seenAuth).toBe(`Bearer ${deviceToken}`)
    expect(provider).toMatchObject({
      id: "allternit-devices",
      name: "Your devices",
      auth_type: "bearer",
      token: deviceToken,
      base_url: "http://cloud.test/api/v1/model-hosts/v1",
    })
    expect(provider!.models).toEqual([
      { id: "phone-1/lfm2.5-350m", name: "lfm2.5-350m (iPhone)", context: 8192, output: 2048 },
    ])
  })
})

describe("allternit-devices provider", () => {
  test("slash model ids resolve and requests carry the discovered token", async () => {
    hosts.data = [phone]
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const ref = Provider.parseModel("allternit-devices/phone-1/lfm2.5-350m")
        expect(ref).toEqual({ providerID: "allternit-devices", modelID: "phone-1/lfm2.5-350m" })
        const model = await Provider.getModel(ref.providerID, ref.modelID)
        expect(model.api.id).toBe("phone-1/lfm2.5-350m")

        // Regression: the discovery merge used to drop dp.token, so the
        // request went out unauthenticated.
        const plan = await Provider.prepareAuth(ref)
        expect(plan.authType).toBe("bearer")
        expect(plan.token).toBe(deviceToken)
        expect(plan.baseURL).toBe("http://cloud.test/api/v1/model-hosts/v1")
      },
    })
  })

  test("rescans follow hosts coming and going; errors keep the last list", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        // No phone online at startup: not connected.
        expect((await Provider.list())["allternit-devices"]).toBeUndefined()

        hosts.data = [phone]
        await Provider.refreshAllternitDevices({ force: true })
        expect(Object.keys((await Provider.list())["allternit-devices"]!.models)).toEqual(["phone-1/lfm2.5-350m"])

        hosts = { status: 502 }
        await Provider.refreshAllternitDevices({ force: true })
        expect(Object.keys((await Provider.list())["allternit-devices"]!.models)).toEqual(["phone-1/lfm2.5-350m"])

        hosts = { status: 200, data: [] }
        await Provider.refreshAllternitDevices({ force: true })
        expect((await Provider.list())["allternit-devices"]).toBeUndefined()
      },
    })
  })
})
