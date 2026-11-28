// Clerk session token check for the relay upgrade.
//
// Same approach as services/remote-control-push (jose + Clerk JWKS), with the
// issuer rule from cmd/allternit-api/src/auth.rs: RS256, signature, expiry with
// 60 s leeway, and `iss` must be one of the Clerk issuers the platform accepts.

import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from "jose";

export const DEFAULT_CLERK_JWKS_URL = "https://clerk.allternit.com/.well-known/jwks.json";
export const DEFAULT_CLERK_ISSUERS = ["https://clerk.allternit.com", "https://allternit.com/__clerk"];

export interface AuthConfig {
  keys: JWTVerifyGetKey;
  issuers: string[];
}

let remoteKeys: { url: string; keys: JWTVerifyGetKey } | null = null;

/** JWKS for a URL, cached per isolate (jose refetches on an unknown `kid`). */
export function remoteJwks(url: string): JWTVerifyGetKey {
  if (remoteKeys?.url !== url) {
    remoteKeys = { url, keys: createRemoteJWKSet(new URL(url), { cooldownDuration: 30_000 }) };
  }
  return remoteKeys.keys;
}

export function parseIssuers(value: string | undefined): string[] {
  const list = (value ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  return list.length > 0 ? list : DEFAULT_CLERK_ISSUERS;
}

/** Returns the Clerk user id (`sub`) for a valid token, otherwise null. */
export async function verifyClerkToken(token: string, config: AuthConfig): Promise<string | null> {
  try {
    const { payload } = await jwtVerify(token, config.keys, {
      algorithms: ["RS256"],
      issuer: config.issuers,
      clockTolerance: 60,
    });
    return typeof payload.sub === "string" && payload.sub.length > 0 ? payload.sub : null;
  } catch {
    // Never log the token or the reason detail; a bad token is just a 401.
    return null;
  }
}
