// Clerk JWT authentication for the standalone gizzi server (phase 1 of iOS
// direct-connect support). Port of cmd/allternit-api/src/auth.rs: JWKS is
// fetched from Clerk and cached (jose's remote JWK set handles TTL + refetch
// on unknown kid), tokens are verified as RS256 with an issuer check.
//
// Precedence (wired into server.ts in place of the old password-only block):
//   1. OPTIONS preflight                 → pass through
//   2. Authorization: Bearer alt_…        → Allternit cloud token validation (401 on failure)
//   3. Authorization: Bearer <jwt>        → Clerk JWT validation (401 on failure)
//   4. GIZZI_SERVER_PASSWORD set          → existing HTTP basic auth
//   5. GIZZI_REQUIRE_CLERK_AUTH set       → 401 (Bearer token required)
//   6. otherwise                          → pass through (loopback dev mode)
import { createRemoteJWKSet, jwtVerify } from "jose"
import { basicAuth } from "hono/basic-auth"
import type { MiddlewareHandler } from "hono"
import { Flag } from "@/runtime/context/flag/flag"
import { Log } from "@/shared/util/log"
import { assertNotVendor, isSelfHosted, isVendorUrl } from "@/shared/selfhost/offline"
import { CLOUD_URLS } from "@/shared/constants/cloudUrls"
import { classifyAllternitToken } from "@/shared/utils/allternitToken"

export namespace ClerkAuth {
  const log = Log.create({ service: "clerk-auth" })

  export const DEFAULT_JWKS_URL = `${CLOUD_URLS.clerk}/.well-known/jwks.json`
  export const DEFAULT_ISSUER = CLOUD_URLS.clerk
  // First-party Clerk proxy used by the web surface in Vite/Fabric. Browser
  // session JWTs carry this `iss`; allternit-api already accepts it.
  export const PROXY_ISSUER = "https://allternit.com/__clerk"
  // Token-validation endpoint of allternit-cloud-api (public by design — it
  // only answers "is this token valid" for a token the caller already holds).
  // Used to authenticate durable `alt_`-prefixed Allternit gateway tokens,
  // which are opaque to local JWKS verification.
  export const DEFAULT_VALIDATE_URL = `${CLOUD_URLS.api}/api/v1/auth/validate`

  export function jwksUrl(): string {
    return Flag.GIZZI_CLERK_JWKS_URL ?? DEFAULT_JWKS_URL
  }

  export function issuer(): string {
    return Flag.GIZZI_CLERK_ISSUER ?? DEFAULT_ISSUER
  }

  export function allowedIssuers(): string[] {
    return [...new Set([issuer(), DEFAULT_ISSUER, PROXY_ISSUER])]
  }

  export function validateUrl(): string {
    return Flag.GIZZI_TOKEN_VALIDATE_URL ?? DEFAULT_VALIDATE_URL
  }

  // Whether requests that carry no credentials at all are rejected. JWT
  // validation itself is always active (the defaults are working values), so
  // this flag is what turns "validates tokens when presented" into "requires
  // tokens" — used when the server is intentionally exposed without a
  // password (e.g. non-loopback bind or --tunnel).
  //
  // Read env live rather than the frozen Flag const: the middleware is
  // mounted once at server setup, and tests/CLI flag parsing toggle this var
  // mid-process after the flag module has been evaluated.
  export function required(): boolean {
    const value = (process.env.GIZZI_REQUIRE_CLERK_AUTH ?? "").toLowerCase()
    return value === "true" || value === "1"
  }

  export interface ClerkUser {
    id: string
    email?: string
    name?: string
    avatarUrl?: string
    orgId?: string
    orgRole?: string
    orgSlug?: string
  }

  // jose's createRemoteJWKSet caches keys and refetches on unknown kid
  // (mirrors JwksManager in auth.rs). Rebuilt if the configured URL changes.
  let cached: { url: string; jwks: ReturnType<typeof createRemoteJWKSet> } | undefined
  function jwks() {
    const url = jwksUrl()
    if (!cached || cached.url !== url) {
      cached = { url, jwks: createRemoteJWKSet(new URL(url)) }
    }
    return cached.jwks
  }

  export async function verify(token: string): Promise<ClerkUser> {
    // Self-hosted: the default JWKS lives on clerk.allternit.com, which is never called.
    assertNotVendor(jwksUrl())
    const { payload } = await jwtVerify(token, jwks(), {
      issuer: allowedIssuers(),
      algorithms: ["RS256"],
    })
    // Clerk session token v2 org claim ("o") with v1 fallback (auth.rs parity).
    const org = payload.o as { id?: string; rol?: string; slg?: string } | undefined
    return {
      id: payload.sub!,
      email: payload.email as string | undefined,
      name: payload.name as string | undefined,
      avatarUrl: payload.image_url as string | undefined,
      orgId: org?.id ?? (payload.org_id as string | undefined),
      orgRole: org?.rol ?? (payload.org_role as string | undefined),
      orgSlug: org?.slg ?? (payload.org_slug as string | undefined),
    }
  }

  // Validates a durable `alt_`-prefixed Allternit gateway token by asking
  // allternit-cloud-api (sha256 hash lookup; the token itself is never
  // stored or logged). Successful validations are cached briefly so a burst
  // of requests with the same key does not hammer the cloud API; failures
  // are not cached so a transient network error does not lock a valid key
  // out.
  const VALID_CACHE_TTL_MS = 60_000
  const validCache = new Map<string, { checkedAt: number; user: ClerkUser }>()

  export async function verifyApiKey(token: string): Promise<ClerkUser | undefined> {
    const cached = validCache.get(token)
    if (cached && Date.now() - cached.checkedAt < VALID_CACHE_TTL_MS) return cached.user
    if (validCache.size > 512) validCache.clear()
    // Self-hosted: Allternit gateway keys are validated by api.allternit.com, which is never called.
    if (isSelfHosted() && isVendorUrl(validateUrl())) return undefined

    const response = await fetch(validateUrl(), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
      signal: AbortSignal.timeout(10_000),
    })
    if (!response.ok) {
      log.warn("api key validation endpoint failed", { status: response.status })
      return undefined
    }
    const body = (await response.json().catch(() => undefined)) as
      | { valid?: unknown; user_id?: unknown; name?: unknown }
      | undefined
    if (!body || body.valid !== true || typeof body.user_id !== "string" || !body.user_id) {
      return undefined
    }
    const user: ClerkUser = {
      id: body.user_id,
      name: typeof body.name === "string" ? body.name : undefined,
    }
    validCache.set(token, { checkedAt: Date.now(), user })
    return user
  }

  function unauthorized(c: any, message: string) {
    c.header("WWW-Authenticate", "Bearer")
    return c.json({ error: "unauthorized", message, requestID: c.get("requestID") }, 401)
  }

  export function middleware(): MiddlewareHandler {
    return async (c, next) => {
      // Allow CORS preflight requests to succeed without auth.
      if (c.req.method === "OPTIONS") return next()

      const header = c.req.header("authorization")
      if (header?.startsWith("Bearer ")) {
        const token = header.slice("Bearer ".length).trim()
        // Durable Allternit gateway tokens (`alt_…`) are opaque locally —
        // validate them against the cloud token-validation endpoint instead
        // of Clerk JWKS. Session JWTs keep the JWKS path.
        if (classifyAllternitToken(token).kind === "apiKey") {
          try {
            const user = await verifyApiKey(token)
            if (user) {
              c.set("clerkUser", user)
              return next()
            }
            return unauthorized(c, "Invalid or revoked Allternit API token")
          } catch (err) {
            log.warn("allternit api key validation failed", {
              error: err instanceof Error ? err.message : String(err),
            })
            return unauthorized(c, "Allternit API token validation failed")
          }
        }
        try {
          const user = await verify(token)
          c.set("clerkUser", user)
          return next()
        } catch (err) {
          log.warn("clerk jwt validation failed", {
            error: err instanceof Error ? err.message : String(err),
          })
          return unauthorized(c, "Invalid or expired Clerk token")
        }
      }

      // Read env live rather than the frozen Flag consts (see required()
      // above): tests and CLI flag parsing set these mid-process.
      const password = process.env.GIZZI_SERVER_PASSWORD
      if (password) {
        const username = process.env.GIZZI_SERVER_USERNAME ?? "gizzi"
        return basicAuth({ username, password })(c, next)
      }

      if (required()) return unauthorized(c, "Clerk Bearer token required")
      return next()
    }
  }
}
