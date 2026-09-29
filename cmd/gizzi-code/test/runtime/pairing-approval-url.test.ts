import { describe, expect, test } from "bun:test"
import { Pairing } from "../../src/runtime/services/pairing/pairing"

describe("Pairing.approvalUrl", () => {
  test("sends platform.allternit.com/pair links to the working page on ai.allternit.com", () => {
    expect(Pairing.approvalUrl("https://platform.allternit.com/pair?code=YU8G-V9KQ")).toBe(
      "https://ai.allternit.com/pair?code=YU8G-V9KQ",
    )
  })

  test("leaves other links alone", () => {
    for (const url of ["https://ai.allternit.com/pair?code=A", "http://localhost:3013/pair?code=A", "https://platform.allternit.com/devices", "not a url"]) {
      expect(Pairing.approvalUrl(url)).toBe(url)
    }
  })
})

describe("Pairing.exchange 403", () => {
  const { privateKey } = require("node:crypto").generateKeyPairSync("ed25519")
  const stored = { privateKey: privateKey.export({ format: "pem", type: "pkcs8" }).toString() } as any
  const pairing = { pairingId: "p1", deviceCode: "d1", challenge: "c1" } as any

  async function exchangeWith(body: unknown) {
    const realFetch = globalThis.fetch
    globalThis.fetch = (async () => new Response(JSON.stringify(body), { status: 403 })) as unknown as typeof fetch
    try {
      return await Pairing.exchange(stored, pairing)
    } finally {
      globalThis.fetch = realFetch
    }
  }

  test("the user clicking Deny is a denial", async () => {
    expect(await exchangeWith({ error: "access_denied", status: "denied" })).toEqual({ status: "denied" })
  })

  test("a platform limit after approval is a refusal with its reason, not a cancel", async () => {
    const message = "Active runtime limit reached (5/5). Revoke an existing runtime or upgrade your plan."
    expect(await exchangeWith({ error: "FORBIDDEN", code: "FORBIDDEN", message })).toEqual({ status: "refused", message })
  })
})

describe("Pairing.machineHostname", () => {
  test("on macOS it is the stable LocalHostName, not the network-assigned name", () => {
    const name = Pairing.machineHostname()
    expect(name.length).toBeGreaterThan(0)
    if (process.platform === "darwin") {
      const local = require("node:child_process").execFileSync("scutil", ["--get", "LocalHostName"], { encoding: "utf8" }).trim()
      expect(name).toBe(local)
    }
  })
})
