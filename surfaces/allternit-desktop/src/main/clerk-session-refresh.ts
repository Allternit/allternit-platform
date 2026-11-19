/**
 * Refresh the Clerk session JWT without a window.
 *
 * Clerk session JWTs live 60 s. ClerkTokenBroker refreshes ~10 s before
 * expiry, and each refresh used to load the whole Clerk renderer in a new
 * hidden 560x680 "Allternit" auth window, read one token and close it: about
 * once a minute for as long as the app ran after pairing (seen in the Linux
 * lab as "Clerk session token received ... closeAuthWindow called" every
 * ~52 s). clerk-js itself refreshes with one Frontend API call,
 * POST /v1/client/sessions/<sid>/tokens, authenticated by the `__client`
 * cookie that sign-in left in the auth partition. auth-manager.ts makes that
 * call directly and falls back to the hidden window only when it fails.
 *
 * This file holds the pure parts (no Electron), so they are unit tested.
 */

/** Clerk Frontend API host from a publishable key (pk_(test|live)_<base64(host)$>). */
export function clerkFrontendApiHost(publishableKey: string | null | undefined): string | null {
  const encoded = publishableKey?.split('_')[2];
  if (!encoded) return null;
  try {
    const host = Buffer.from(encoded, 'base64').toString('utf8').replace(/\$+$/, '');
    return /^[a-z0-9.-]+$/i.test(host) ? host : null;
  } catch {
    return null;
  }
}

/** The session (and active organization) a Clerk session JWT belongs to. */
export function clerkSessionFromJwtPayload(payload: Record<string, unknown> | null): {
  sessionId: string;
  organizationId: string | null;
} | null {
  if (!payload) return null;
  const sid = payload.sid;
  if (typeof sid !== 'string' || !/^sess_[A-Za-z0-9]+$/.test(sid)) return null;
  // Session token v2 nests the organization as o.id; v1 used org_id.
  const o = payload.o;
  const v2 = o && typeof o === 'object' && typeof (o as { id?: unknown }).id === 'string' ? (o as { id: string }).id : null;
  const v1 = typeof payload.org_id === 'string' ? payload.org_id : null;
  return { sessionId: sid, organizationId: v2 ?? v1 };
}

/** The Frontend API request clerk-js makes for a fresh session token. */
export function clerkTokenRequest(input: {
  frontendApiHost: string;
  /** The auth window's origin host (accounts.<domain>), which the Frontend API accepts. */
  originHost: string | null;
  sessionId: string;
  organizationId: string | null;
}): { url: string; method: 'POST'; headers: Record<string, string>; body: string } {
  const headers: Record<string, string> = { 'Content-Type': 'application/x-www-form-urlencoded' };
  if (input.originHost) headers.Origin = `https://${input.originHost}`;
  const body = new URLSearchParams();
  if (input.organizationId) body.set('organizationId', input.organizationId);
  return {
    url: `https://${input.frontendApiHost}/v1/client/sessions/${encodeURIComponent(input.sessionId)}/tokens`,
    method: 'POST',
    headers,
    body: body.toString(),
  };
}

/**
 * The JWT from a Frontend API token response, only when it is for the same
 * session (a different `sid` means the cookie belongs to another sign-in).
 */
export function jwtFromClerkTokenResponse(
  json: unknown,
  expectedSessionId: string,
  decode: (jwt: string) => Record<string, unknown> | null,
): string | null {
  const jwt = json && typeof json === 'object' ? (json as { jwt?: unknown }).jwt : null;
  if (typeof jwt !== 'string' || jwt.split('.').length !== 3) return null;
  return clerkSessionFromJwtPayload(decode(jwt))?.sessionId === expectedSessionId ? jwt : null;
}
