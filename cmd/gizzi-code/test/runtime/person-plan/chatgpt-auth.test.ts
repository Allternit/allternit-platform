import { describe, expect, test } from "bun:test"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import {
  DYNAMIC_CLIENT,
  RESOURCE,
  TOKEN_URL,
  authorizeUrl,
  jwkThumbprintUrn,
  loadSession,
  pkcePair,
  readCallback,
  refreshForm,
  refreshSession,
  saveSession,
  usableSession,
  type Deps,
  type Session,
} from "../../../src/runtime/providers/person-plan/chatgpt-auth"
import type { CredentialWriter } from "../../../src/runtime/context/config/credential-store"

function memoryStore(): CredentialWriter {
  const data = new Map<string, string>()
  return {
    name: "memory",
    write: async (s, a, v) => void data.set(`${s}/${a}`, v),
    read: async (s, a) => data.get(`${s}/${a}`) ?? null,
    remove: async (s, a) => void data.delete(`${s}/${a}`),
  }
}

const base = {
  clientId: DYNAMIC_CLIENT,
  hostId: "urn:ietf:params:oauth:jwk-thumbprint:sha-256:abc",
  redirectUri: "http://127.0.0.1:1455/auth/callback",
  state: "st",
  nonce: "no",
  challenge: "ch",
}

describe("authorize URL", () => {
  test("first sign-in registers with dynamic_agent_client and the Gizzi Code name hint", () => {
    const u = new URL(authorizeUrl({ ...base, idTokenHint: "old.id.token", loginHint: "a@b.c" }))
    expect(`${u.origin}${u.pathname}`).toBe("https://auth.openai.com/api/accounts/authorize")
    const q = u.searchParams
    expect(q.get("client_id")).toBe("dynamic_agent_client")
    expect(q.get("agent_name_hint")).toBe("Gizzi Code")
    expect(q.get("ext_agent_host_id")).toBe(base.hostId)
    expect(q.get("response_type")).toBe("code")
    expect(q.get("redirect_uri")).toBe("http://127.0.0.1:1455/auth/callback")
    expect(q.get("scope")).toBe("openid profile email offline_access resource.invoke chatgpt.tokens.use.direct")
    expect(q.get("resource")).toBe("https://api.openai.com/v1")
    expect(q.get("code_challenge_method")).toBe("S256")
    expect(q.get("code_challenge")).toBe("ch")
    expect(q.get("state")).toBe("st")
    expect(q.get("nonce")).toBe("no")
    // Hints belong to a returning sign-in only.
    expect(q.has("id_token_hint")).toBe(false)
    expect(q.has("login_hint")).toBe(false)
  })

  test("a returning sign-in uses the issued client id, sends hints and no name hint", () => {
    const q = new URL(authorizeUrl({ ...base, clientId: "oaiapp_123", idTokenHint: "old.id.token", loginHint: "a@b.c" })).searchParams
    expect(q.get("client_id")).toBe("oaiapp_123")
    expect(q.has("agent_name_hint")).toBe(false)
    expect(q.get("id_token_hint")).toBe("old.id.token")
    expect(q.get("login_hint")).toBe("a@b.c")
  })
})

describe("PKCE and host id", () => {
  test("S256 challenge matches the RFC 7636 example", async () => {
    const p = await pkcePair("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
    expect(p.challenge).toBe("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
  })

  test("host id is the RFC 9278 URN over the required members only", async () => {
    const pub = { kty: "EC", crv: "P-256", x: "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU", y: "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" }
    const a = await jwkThumbprintUrn(pub)
    const b = await jwkThumbprintUrn({ y: pub.y, x: pub.x, crv: "P-256", kty: "EC", d: "secret", use: "sig" } as any)
    expect(a).toBe(b)
    expect(a).toMatch(/^urn:ietf:params:oauth:jwk-thumbprint:sha-256:[A-Za-z0-9_-]{43}$/)
    await expect(jwkThumbprintUrn({ kty: "RSA" } as any)).rejects.toThrow()
  })
})

describe("callback", () => {
  const cb = (q: string) => new URL(`http://127.0.0.1:1455/auth/callback?${q}`)

  test("registration returns the issued client id", () => {
    const r = readCallback(cb("code=c1&state=st&client_id=oaiapp_9&scope=openid+chatgpt.tokens.use.direct"), { state: "st", clientId: DYNAMIC_CLIENT })
    expect(r).toEqual({ code: "c1", clientId: "oaiapp_9", scope: ["openid", "chatgpt.tokens.use.direct"] })
  })

  test("refuses a wrong state, a cancel, and a registration without an issued id", () => {
    expect(() => readCallback(cb("code=c&state=evil"), { state: "st", clientId: DYNAMIC_CLIENT })).toThrow(/did not match/)
    expect(() => readCallback(cb("error=access_denied&state=st"), { state: "st", clientId: DYNAMIC_CLIENT })).toThrow(/cancelled/)
    expect(() => readCallback(cb("code=c&state=st"), { state: "st", clientId: DYNAMIC_CLIENT })).toThrow(/registering/)
    expect(() => readCallback(cb("code=c&state=st&client_id=dynamic_agent_client"), { state: "st", clientId: DYNAMIC_CLIENT })).toThrow(/registering/)
  })

  test("a returning sign-in may omit the client id but not change it", () => {
    expect(readCallback(cb("code=c&state=st"), { state: "st", clientId: "oaiapp_1" }).clientId).toBe("oaiapp_1")
    expect(() => readCallback(cb("code=c&state=st&client_id=oaiapp_2"), { state: "st", clientId: "oaiapp_1" })).toThrow(/different app/)
  })
})

describe("refresh", () => {
  const T0 = 1_800_000_000_000
  const session = (over: Partial<Session> = {}): Session => ({
    issuer: "https://auth.openai.com",
    subject: "user-1",
    client_id: "oaiapp_1",
    ext_agent_host_id: base.hostId,
    id_token: "id",
    access_token: "old-access",
    refresh_token: "rt-1",
    token_type: "Bearer",
    expires_at: T0 + 30_000,
    scopes: ["chatgpt.tokens.use.direct"],
    saved_at: "x",
    ...over,
  })

  function deps(handler: (form: URLSearchParams) => Response): { deps: Deps; calls: URLSearchParams[] } {
    const calls: URLSearchParams[] = []
    return {
      calls,
      deps: {
        store: memoryStore(),
        now: () => T0,
        lockDir: mkdtempSync(join(tmpdir(), "gizzi-chatgpt-")),
        fetch: (async (url: string, init: RequestInit) => {
          expect(url).toBe(TOKEN_URL)
          const form = new URLSearchParams(String(init.body))
          calls.push(form)
          return handler(form)
        }) as unknown as typeof fetch,
      },
    }
  }

  test("refresh form keeps the grant: issued client id, resource, no scope", () => {
    const f = refreshForm(session())
    expect(f).toEqual({ grant_type: "refresh_token", client_id: "oaiapp_1", refresh_token: "rt-1", resource: RESOURCE })
  })

  test("a refresh rotates the refresh token and replaces the token set together", async () => {
    const { deps: d, calls } = deps(() => Response.json({ access_token: "new-access", refresh_token: "rt-2", expires_in: 3600, scope: "openid chatgpt.tokens.use.direct" }))
    await saveSession(session(), d)
    const next = await usableSession(d)
    expect(next.access_token).toBe("new-access")
    expect(next.refresh_token).toBe("rt-2")
    expect(next.expires_at).toBe(T0 + 3_600_000)
    expect(calls).toHaveLength(1)
    expect(calls[0]!.get("refresh_token")).toBe("rt-1")
    // The rotated token is what the next refresh uses.
    expect((await loadSession(d))!.refresh_token).toBe("rt-2")
  })

  test("concurrent callers share one refresh", async () => {
    const { deps: d, calls } = deps(() => Response.json({ access_token: "a2", refresh_token: "rt-2", expires_in: 3600 }))
    await saveSession(session(), d)
    const [a, b] = await Promise.all([usableSession(d), usableSession(d)])
    expect(a.access_token).toBe("a2")
    expect(b.access_token).toBe("a2")
    expect(calls).toHaveLength(1)
  })

  test("a token still fresh is not refreshed", async () => {
    const { deps: d, calls } = deps(() => Response.json({ access_token: "x" }))
    await saveSession(session({ expires_at: T0 + 3_600_000 }), d)
    expect((await usableSession(d)).access_token).toBe("old-access")
    expect(calls).toHaveLength(0)
  })

  test("a reused refresh token clears the tokens and asks for a new sign-in", async () => {
    const { deps: d } = deps(() => Response.json({ error: "refresh_token_reused" }, { status: 400 }))
    await saveSession(session(), d)
    await expect(refreshSession(d)).rejects.toThrow(/sign in again/)
    expect(await loadSession(d)).toBeUndefined()
  })

  test("a server error keeps the tokens (not a sign-out)", async () => {
    const { deps: d } = deps(() => new Response("upstream", { status: 503 }))
    await saveSession(session(), d)
    await expect(refreshSession(d)).rejects.toThrow()
    expect((await loadSession(d))!.refresh_token).toBe("rt-1")
  })
})
