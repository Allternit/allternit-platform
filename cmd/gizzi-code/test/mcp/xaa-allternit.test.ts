// @ts-nocheck
import { test, expect, beforeEach } from "bun:test"
import {
  fetchAllternitIdJag,
  performAllternitXaa,
  resetAllternitXaaCache,
} from "../../src/runtime/services/mcp/xaaAllternit"

const creds = { base: "http://127.0.0.1:8013", token: "local-token" }
const SERVER = "https://mcp.vendor.example/mcp"

const jag = {
  id_jag: "jag-1",
  token_endpoint: "https://as.vendor.example/token",
  client_id: "c1",
  client_secret: "s1",
  expires_in: 300,
  connector_id: "vendor",
  org_id: "org_A",
  as_issuer: "https://as.vendor.example",
  resource: SERVER,
  scopes: ["read"],
}

function json(status: number, body: unknown) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } })
}

beforeEach(() => resetAllternitXaaCache())

test("not configured (404) or unknown (503) means the normal sign-in runs, and is remembered", async () => {
  for (const status of [404, 503]) {
    resetAllternitXaaCache()
    let calls = 0
    const fetchFn = async () => {
      calls++
      return json(status, { error: { code: "not_configured", message: "x" } })
    }
    expect(await fetchAllternitIdJag(SERVER, { creds, fetchFn })).toBeNull()
    expect(await fetchAllternitIdJag(SERVER, { creds, fetchFn })).toBeNull()
    expect(calls).toBe(1)
  }
})

test("a configured server that is refused throws the plain reason, never falls back", async () => {
  const fetchFn = async () => json(403, { error: { code: "role_not_allowed", message: "Your role isn't allowed." } })
  await expect(performAllternitXaa(SERVER, "vendor", { creds, fetchFn })).rejects.toThrow("Your role isn't allowed.")
})

test("ID-JAG then jwt-bearer grant with Basic client auth; nothing without credentials", async () => {
  const seen: Array<{ url: string; body: string; auth?: string }> = []
  const fetchFn = async (url, init) => {
    seen.push({ url: String(url), body: String(init.body), auth: init.headers?.Authorization ?? init.headers?.authorization })
    if (String(url).endsWith("/api/v1/managed-auth/id-jag")) return json(200, jag)
    return json(200, { access_token: "at-1", expires_in: 600 })
  }
  const tokens = await performAllternitXaa(SERVER, "vendor", { creds, fetchFn })
  expect(tokens.access_token).toBe("at-1")
  expect(tokens.authorizationServerUrl).toBe("https://as.vendor.example")
  expect(JSON.parse(seen[0].body)).toEqual({ server_url: SERVER })
  expect(seen[0].auth).toBe("Bearer local-token")
  expect(seen[1].url).toBe(jag.token_endpoint)
  expect(seen[1].body).toContain("assertion=jag-1")
  expect(seen[1].body).toContain("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer")
  expect(seen[1].body).not.toContain("client_secret")
  expect(seen[1].auth).toBe(`Basic ${Buffer.from("c1:s1").toString("base64")}`)

  expect(await performAllternitXaa(SERVER, "vendor", { creds: null, fetchFn })).toBeNull()
})

test("a non-HTTPS vendor token endpoint is refused before the assertion is sent", async () => {
  const sent: string[] = []
  const fetchFn = async (url) => {
    sent.push(String(url))
    return json(200, { ...jag, token_endpoint: "http://as.vendor.example/token" })
  }
  await expect(performAllternitXaa(SERVER, "vendor", { creds, fetchFn })).rejects.toThrow("non-HTTPS")
  expect(sent).toHaveLength(1)
})
