// Meeting SDK JWT for SDKAuth (AuthContext.jwt_token).
//
// Zoom: header {alg: HS256, typ: JWT}; payload appKey (client id), iat, exp,
// tokenExp; exp and tokenExp at least 1800 s after iat, at most 48 h; signed
// with the client secret; generate it server-side.
// https://developers.zoom.us/docs/meeting-sdk/auth/
//
// BotIdentity.zoom_obf carries `sdkKey` and `sdkSecretRef` (adapter.ts). The ref is
// resolved here, so the client secret does not have to enter the pod:
//   jwt:<token>   a JWT cloud-api already signed (preferred: the secret stays in cloud-api)
//   env:<NAME>    the client secret is in the pod env var NAME; the pod signs (dev / own-account tests)

import { createHmac } from "node:crypto";

const b64url = (b: Buffer | string): string => Buffer.from(b).toString("base64url");

/** Sign a Meeting SDK JWT. `ttlSec` is clamped to Zoom's 1800 s .. 48 h window. */
export function signSdkJwt(clientId: string, clientSecret: string, nowSec = Math.floor(Date.now() / 1000), ttlSec = 2 * 3600): string {
  if (!clientId) throw new Error("Zoom client id is empty");
  if (!clientSecret) throw new Error("Zoom client secret is empty");
  const ttl = Math.min(Math.max(ttlSec, 1800), 48 * 3600);
  const iat = nowSec - 30; // small back-date for clock skew between pod and Zoom
  const exp = iat + ttl;
  const header = b64url(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const payload = b64url(JSON.stringify({ appKey: clientId, iat, exp, tokenExp: exp }));
  const sig = createHmac("sha256", clientSecret).update(`${header}.${payload}`).digest("base64url");
  return `${header}.${payload}.${sig}`;
}

/** Resolve BotIdentity.zoom_obf's sdkSecretRef into an SDK JWT. Throws with a message safe to log (no secret material). */
export function resolveSdkJwt(sdkKey: string, sdkSecretRef: string, env: Record<string, string | undefined> = process.env): string {
  if (sdkSecretRef.startsWith("jwt:")) {
    const token = sdkSecretRef.slice(4);
    if (token.split(".").length !== 3) throw new Error("sdkSecretRef jwt: is not a JWT");
    return token;
  }
  if (sdkSecretRef.startsWith("env:")) {
    const name = sdkSecretRef.slice(4);
    const secret = env[name];
    if (!secret) throw new Error(`sdkSecretRef names env ${name}, which is not set`);
    return signSdkJwt(sdkKey, secret);
  }
  throw new Error("sdkSecretRef must start with jwt: or env:");
}
