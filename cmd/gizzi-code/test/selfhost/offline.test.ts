import { afterEach, describe, expect, test } from "bun:test"
import {
  __resetSelfHostedGuardForTests,
  installSelfHostedGuard,
  isVendorHost,
  SelfHostedEgressError,
} from "../../src/shared/selfhost/offline"
import { isEssentialTrafficOnly } from "../../src/shared/utils/privacyLevel"

// ALLTERNIT_SELF_HOSTED=1: gizzi makes no calls to Allternit hosts, and
// everything else (the customer's own server, model provider) still works.

const originalFetch = globalThis.fetch
const originalWebSocket = globalThis.WebSocket
const savedEnv = { ...process.env }

afterEach(() => {
  globalThis.fetch = originalFetch
  globalThis.WebSocket = originalWebSocket
  for (const key of Object.keys(process.env)) if (!(key in savedEnv)) delete process.env[key]
  Object.assign(process.env, savedEnv)
  __resetSelfHostedGuardForTests()
})

describe("self-hosted offline switch", () => {
  test("vendor host matching covers subdomains only of Allternit's domains", () => {
    expect(isVendorHost("api.allternit.com")).toBe(true)
    expect(isVendorHost("allternit.com")).toBe(true)
    expect(isVendorHost("MAIL.NEWS.ALLTERNIT.COM.")).toBe(true)
    expect(isVendorHost("install.gizziio.com")).toBe(true)
    expect(isVendorHost("notallternit.com")).toBe(false)
    expect(isVendorHost("allternit.com.evil.net")).toBe(false)
  })

  test("off unless ALLTERNIT_SELF_HOSTED is set", () => {
    delete process.env.ALLTERNIT_SELF_HOSTED
    expect(installSelfHostedGuard()).toBe(false)
    expect(globalThis.fetch).toBe(originalFetch)
  })

  test("fetch and WebSocket to Allternit fail fast; the customer's own server still answers", async () => {
    process.env.ALLTERNIT_SELF_HOSTED = "1"
    expect(installSelfHostedGuard()).toBe(true)
    expect(isEssentialTrafficOnly()).toBe(true)
    expect(process.env.GIZZI_DISABLE_NONESSENTIAL_TRAFFIC).toBe("1")

    await expect(fetch("https://api.allternit.com/v1/models")).rejects.toBeInstanceOf(SelfHostedEgressError)
    await expect(fetch(new Request("https://clerk.allternit.com/.well-known/jwks.json"))).rejects.toThrow(/host_not_allowed/)
    expect(() => new WebSocket("wss://remotecontrol.allternit.com/ws")).toThrow(SelfHostedEgressError)

    const own = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response("own server") })
    try {
      const res = await fetch(`http://127.0.0.1:${own.port}/v1/server/info`)
      expect(await res.text()).toBe("own server")
    } finally {
      own.stop(true)
    }
  })
})
