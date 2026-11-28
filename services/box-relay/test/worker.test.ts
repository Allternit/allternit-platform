import { SignJWT, createLocalJWKSet, exportJWK, generateKeyPair } from "jose";
import { beforeAll, describe, expect, it } from "vitest";
import { DEFAULT_CLERK_ISSUERS, verifyClerkToken, type AuthConfig } from "../src/auth";
import { handleRequest, type Deps, type Env } from "../src/index";

let config: AuthConfig;
let privateKey: CryptoKey;

beforeAll(async () => {
  const pair = await generateKeyPair("RS256");
  privateKey = pair.privateKey as CryptoKey;
  const jwk = { ...(await exportJWK(pair.publicKey)), kid: "test-key", alg: "RS256" };
  config = { keys: createLocalJWKSet({ keys: [jwk] }), issuers: DEFAULT_CLERK_ISSUERS };
});

function sign(claims: { sub?: string; iss?: string; exp?: number }) {
  const jwt = new SignJWT(claims.sub ? { sub: claims.sub } : {})
    .setProtectedHeader({ alg: "RS256", kid: "test-key" })
    .setIssuedAt()
    .setIssuer(claims.iss ?? "https://clerk.allternit.com")
    .setExpirationTime(claims.exp ?? "5m");
  return jwt.sign(privateKey);
}

describe("Clerk token check", () => {
  it("accepts a token from either platform issuer", async () => {
    expect(await verifyClerkToken(await sign({ sub: "user_1" }), config)).toBe("user_1");
    expect(
      await verifyClerkToken(await sign({ sub: "user_2", iss: "https://allternit.com/__clerk" }), config),
    ).toBe("user_2");
  });

  it("rejects another issuer, an expired token, a token with no user, and junk", async () => {
    expect(await verifyClerkToken(await sign({ sub: "u", iss: "https://evil.example" }), config)).toBeNull();
    const longAgo = Math.floor(Date.now() / 1000) - 3600;
    expect(await verifyClerkToken(await sign({ sub: "u", exp: longAgo }), config)).toBeNull();
    expect(await verifyClerkToken(await sign({}), config)).toBeNull();
    expect(await verifyClerkToken("not.a.jwt", config)).toBeNull();
  });
});

describe("Upgrade gate", () => {
  const forwarded: Request[] = [];
  const namespace = {
    idFromName: (name: string) => name,
    get: (id: string) => ({
      fetch: async (req: Request) => {
        forwarded.push(req);
        return new Response(`object:${id}`);
      },
    }),
  } as unknown as DurableObjectNamespace;
  const env: Env = { RELAY_ENABLED: "true", BOX_RELAY: namespace };
  const deps: Deps = { verify: (token) => verifyClerkToken(token, config) };
  const upgrade = { upgrade: "websocket" };

  it("answers 503 relay_off unless RELAY_ENABLED is exactly true", async () => {
    for (const value of [undefined, "false", "TRUE", "1"]) {
      const res = await handleRequest(
        new Request("https://relay.allternit.com/wisp/v1?t=x", { headers: upgrade }),
        { ...env, RELAY_ENABLED: value },
        deps,
      );
      expect(res.status).toBe(503);
      expect(await res.json()).toEqual({ error: "relay_off" });
    }
  });

  it("refuses a request with no token or a bad token with 401", async () => {
    const none = await handleRequest(new Request("https://relay.allternit.com/wisp/v1", { headers: upgrade }), env, deps);
    expect(none.status).toBe(401);
    const bad = await handleRequest(
      new Request("https://relay.allternit.com/wisp/v1?t=not.a.jwt", { headers: upgrade }),
      env,
      deps,
    );
    expect(bad.status).toBe(401);
    expect(forwarded).toHaveLength(0);
  });

  it("refuses plain HTTP and other paths", async () => {
    const token = await sign({ sub: "user_1" });
    const plain = await handleRequest(new Request(`https://relay.allternit.com/wisp/v1?t=${token}`), env, deps);
    expect(plain.status).toBe(426);
    const other = await handleRequest(
      new Request(`https://relay.allternit.com/other?t=${token}`, { headers: upgrade }),
      env,
      deps,
    );
    expect(other.status).toBe(404);
  });

  it("hands a signed-in user's socket to that user's object, without the token", async () => {
    const token = await sign({ sub: "user_42" });
    const res = await handleRequest(
      new Request(`https://relay.allternit.com/wisp/v1/?t=${token}`, { headers: upgrade }),
      env,
      deps,
    );
    expect(await res.text()).toBe("object:user_42");
    const req = forwarded.at(-1)!;
    expect(req.url).not.toContain(token);
    expect(req.headers.get("x-box-relay-user")).toBe("user_42");
  });
});
