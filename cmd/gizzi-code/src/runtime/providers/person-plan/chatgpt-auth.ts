/**
 * Sign in with ChatGPT on this computer: OpenAI's open-source token-sharing
 * path (CONTRACT-connections-2026-11 §9.9;
 * https://developers.openai.com/siwc/token-sharing-open-source/sign-in).
 *
 * - First sign-in registers this host: `client_id=dynamic_agent_client`,
 *   `agent_name_hint=Gizzi Code`, `ext_agent_host_id=<JWK thumbprint URN>`.
 *   The callback carries the issued `client_id` (`oaiapp_…`), saved per host
 *   and used for every later sign-in, the code exchange, refresh and revoke.
 * - Host id: RFC 9278 thumbprint URN of a P-256 key made once per computer
 *   and kept in the credential store (macOS Keychain; a 0600 file elsewhere).
 * - PKCE S256, fresh `state` and `nonce`, loopback redirect on
 *   `127.0.0.1:1455` (another port when 1455 is busy; only the port varies).
 * - The ID token is checked against OpenAI's JWKS (signature, issuer,
 *   audience, expiry, nonce) and the grant must include
 *   `chatgpt.tokens.use.direct` before any chat.
 * - Refresh rotates the refresh token; refreshes are serialized across
 *   processes with a lock file. Sign-out revokes the refresh token at the
 *   discovery document's `revocation_endpoint`, then deletes local tokens.
 *   The registration (issued client id) and host id stay for the next sign-in.
 * - Tokens never leave this computer and are never sent to Allternit.
 */
import fs from "fs"
import path from "path"
import { createCredentialWriter, defaultCredentialDir, type CredentialWriter } from "@/runtime/context/config/credential-store"

export const AUTH_ISSUER = "https://auth.openai.com"
export const AUTHORIZE_URL = `${AUTH_ISSUER}/api/accounts/authorize`
export const TOKEN_URL = `${AUTH_ISSUER}/api/accounts/oauth/token`
export const DISCOVERY_URL = `${AUTH_ISSUER}/.well-known/openid-configuration`
export const RESOURCE = "https://api.openai.com/v1"
export const DYNAMIC_CLIENT = "dynamic_agent_client"
export const AGENT_NAME = "Gizzi Code"
export const DIRECT_SCOPE = "chatgpt.tokens.use.direct"
export const SCOPES = ["openid", "profile", "email", "offline_access", "resource.invoke", DIRECT_SCOPE]
export const LOOPBACK_PORT = 1455
export const CALLBACK_PATH = "/auth/callback"

const HOST_SERVICE = "gizzi-chatgpt-host"
const SESSION_SERVICE = "gizzi-chatgpt"
/** Refresh this long before the access token expires. */
const REFRESH_MARGIN_MS = 120_000
const SIGN_IN_TIMEOUT_MS = 10 * 60_000

/** Refresh errors after which the tokens are unusable: sign in again (with the saved client id). */
const TERMINAL_REFRESH = new Set([
  "invalid_grant",
  "invalid_refresh_token",
  "token_expired",
  "refresh_token_expired",
  "refresh_token_invalidated",
  "refresh_token_reused",
  "invalid_client",
])

export class ChatgptAuthError extends Error {
  constructor(
    message: string,
    readonly code: string,
  ) {
    super(message)
    this.name = "ChatgptAuthError"
  }
}

/** The host registration: kept across sign-outs (one per ChatGPT account on this host). */
export type Registration = {
  client_id: string
  ext_agent_host_id: string
  email?: string
  subject?: string
  /** The last validated ID token, sent as `id_token_hint` on a later sign-in. */
  id_token?: string
}

export type Session = {
  email?: string
  issuer: string
  subject: string
  client_id: string
  ext_agent_host_id: string
  id_token: string
  access_token: string
  refresh_token: string
  token_type: string
  /** Epoch ms. */
  expires_at: number
  scopes: string[]
  saved_at: string
  plan?: string
  models?: { slug: string; name: string }[]
  models_at?: number
}

export type TokenResponse = {
  id_token?: string
  access_token: string
  refresh_token?: string
  token_type?: string
  expires_in?: number
  scope?: string
}

export type Deps = {
  store: CredentialWriter
  fetch: typeof fetch
  now: () => number
  lockDir: string
}

export function defaultDeps(): Deps {
  return {
    store: createCredentialWriter("auto"),
    fetch: ((input: RequestInfo | URL, init?: RequestInit) => fetch(input, init)) as typeof fetch,
    now: () => Date.now(),
    lockDir: path.join(defaultCredentialDir(), "chatgpt"),
  }
}

// ─── Encoding helpers ─────────────────────────────────────────────────────────

export function base64url(bytes: ArrayBuffer | Uint8Array): string {
  return Buffer.from(bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes)).toString("base64url")
}

export function randomToken(bytes = 32): string {
  return base64url(crypto.getRandomValues(new Uint8Array(bytes)))
}

export async function pkcePair(verifier = randomToken(48)): Promise<{ verifier: string; challenge: string }> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))
  return { verifier, challenge: base64url(digest) }
}

/** RFC 7638 thumbprint of a P-256 public JWK, as the RFC 9278 URN. */
export async function jwkThumbprintUrn(jwk: { kty?: string; crv?: string; x?: string; y?: string }): Promise<string> {
  if (jwk.kty !== "EC" || jwk.crv !== "P-256" || !jwk.x || !jwk.y) throw new Error("host key must be a P-256 EC key")
  // Required members only, in lexicographic order, no whitespace.
  const canonical = `{"crv":"P-256","kty":"EC","x":"${jwk.x}","y":"${jwk.y}"}`
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(canonical))
  return `urn:ietf:params:oauth:jwk-thumbprint:sha-256:${base64url(digest)}`
}

function jwtPart(token: string, index: number): any {
  const part = token.split(".")[index]
  if (!part) throw new ChatgptAuthError("ChatGPT sent a malformed sign-in token.", "bad_id_token")
  return JSON.parse(Buffer.from(part, "base64url").toString("utf8"))
}

// ─── Authorize URL and callback ───────────────────────────────────────────────

export type AuthorizeParams = {
  clientId: string
  hostId: string
  redirectUri: string
  state: string
  nonce: string
  challenge: string
  idTokenHint?: string
  loginHint?: string
}

/**
 * The authorize URL. A first sign-in (`dynamic_agent_client`) sends
 * `agent_name_hint`; a returning one sends the issued client id with the
 * retained `id_token_hint` / `login_hint` and no name hint.
 */
export function authorizeUrl(p: AuthorizeParams): string {
  const registering = p.clientId === DYNAMIC_CLIENT
  const q = new URLSearchParams()
  q.set("client_id", p.clientId)
  if (registering) q.set("agent_name_hint", AGENT_NAME)
  q.set("ext_agent_host_id", p.hostId)
  if (!registering && p.idTokenHint) q.set("id_token_hint", p.idTokenHint)
  if (!registering && p.loginHint) q.set("login_hint", p.loginHint)
  q.set("response_type", "code")
  q.set("redirect_uri", p.redirectUri)
  q.set("scope", SCOPES.join(" "))
  q.set("resource", RESOURCE)
  q.set("state", p.state)
  q.set("nonce", p.nonce)
  q.set("code_challenge_method", "S256")
  q.set("code_challenge", p.challenge)
  return `${AUTHORIZE_URL}?${q.toString()}`
}

export type CallbackResult = { code: string; clientId: string; scope?: string[] }

/**
 * Checks the loopback callback. State must match; `access_denied` stops the
 * sign-in; a registration must return the issued client id (never
 * `dynamic_agent_client`); a returning sign-in may omit it but must not
 * return a different one.
 */
export function readCallback(url: URL, expected: { state: string; clientId: string }): CallbackResult {
  const q = url.searchParams
  if (q.get("state") !== expected.state) throw new ChatgptAuthError("The sign-in reply did not match this sign-in. Try again.", "state_mismatch")
  const error = q.get("error")
  if (error === "access_denied") throw new ChatgptAuthError("Sign-in was cancelled on the ChatGPT page.", "access_denied")
  if (error) throw new ChatgptAuthError(`ChatGPT refused the sign-in (${error}).`, error)
  const code = q.get("code")
  if (!code) throw new ChatgptAuthError("ChatGPT did not send a sign-in code. Try again.", "no_code")
  const returned = q.get("client_id") ?? undefined
  const scope = q.get("scope")?.split(/[\s+]+/).filter(Boolean)
  if (expected.clientId === DYNAMIC_CLIENT) {
    if (!returned || returned === DYNAMIC_CLIENT)
      throw new ChatgptAuthError("ChatGPT did not finish registering Gizzi Code on this computer. Try again.", "registration_incomplete")
    return { code, clientId: returned, scope }
  }
  if (returned && returned !== expected.clientId)
    throw new ChatgptAuthError("ChatGPT answered for a different app registration. Try again.", "client_mismatch")
  return { code, clientId: expected.clientId, scope }
}

// ─── Token endpoint ───────────────────────────────────────────────────────────

async function tokenCall(deps: Deps, form: Record<string, string>): Promise<TokenResponse> {
  let res: Response
  try {
    res = await deps.fetch(TOKEN_URL, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json" },
      body: new URLSearchParams(form).toString(),
      signal: AbortSignal.timeout(30_000),
    })
  } catch (e) {
    throw new ChatgptAuthError(`Could not reach ChatGPT sign-in: ${e instanceof Error ? e.message : String(e)}`, "network")
  }
  const text = await res.text()
  let body: any = {}
  try {
    body = JSON.parse(text)
  } catch {}
  if (!res.ok) {
    const code = typeof body.error === "string" ? body.error : typeof body.error?.code === "string" ? body.error.code : `http_${res.status}`
    const detail = typeof body.error_description === "string" ? body.error_description : typeof body.error?.message === "string" ? body.error.message : ""
    throw new ChatgptAuthError(`ChatGPT sign-in failed (${code})${detail ? `: ${detail}` : ""}.`, code)
  }
  if (typeof body.access_token !== "string") throw new ChatgptAuthError("ChatGPT sign-in sent no access token.", "no_access_token")
  return body as TokenResponse
}

export function exchangeForm(p: { clientId: string; code: string; verifier: string; redirectUri: string }): Record<string, string> {
  return {
    grant_type: "authorization_code",
    client_id: p.clientId,
    code: p.code,
    code_verifier: p.verifier,
    redirect_uri: p.redirectUri,
    resource: RESOURCE,
  }
}

/** Refresh keeps the grant: no `scope`, the issued client id, the same resource. */
export function refreshForm(session: Pick<Session, "client_id" | "refresh_token">): Record<string, string> {
  return { grant_type: "refresh_token", client_id: session.client_id, refresh_token: session.refresh_token, resource: RESOURCE }
}

/** Replace access token, expiry, scopes and the rotating refresh token together. */
export function applyRefresh(session: Session, tokens: TokenResponse, now: number): Session {
  return {
    ...session,
    access_token: tokens.access_token,
    refresh_token: tokens.refresh_token ?? session.refresh_token,
    id_token: tokens.id_token ?? session.id_token,
    token_type: tokens.token_type ?? session.token_type,
    expires_at: now + (tokens.expires_in ?? 3600) * 1000,
    scopes: tokens.scope ? tokens.scope.split(/\s+/).filter(Boolean) : session.scopes,
    saved_at: new Date(now).toISOString(),
  }
}

// ─── Discovery + ID token check ───────────────────────────────────────────────

let discoveryCache: { at: number; doc: any } | undefined

async function discovery(deps: Deps): Promise<any> {
  if (discoveryCache && deps.now() - discoveryCache.at < 3_600_000) return discoveryCache.doc
  const res = await deps.fetch(DISCOVERY_URL, { signal: AbortSignal.timeout(15_000) })
  if (!res.ok) throw new ChatgptAuthError(`Could not read ChatGPT sign-in settings (HTTP ${res.status}).`, "discovery")
  const doc = await res.json()
  discoveryCache = { at: deps.now(), doc }
  return doc
}

/** Signature (JWKS), issuer, audience, expiry and nonce of an ID token. Returns its claims. */
export async function verifyIdToken(deps: Deps, idToken: string, expected: { clientId: string; nonce?: string }): Promise<any> {
  const header = jwtPart(idToken, 0)
  const claims = jwtPart(idToken, 1)
  const doc = await discovery(deps)
  const issuer: string = doc.issuer ?? AUTH_ISSUER
  if (claims.iss !== issuer) throw new ChatgptAuthError("The sign-in token came from the wrong issuer.", "bad_issuer")
  const aud = Array.isArray(claims.aud) ? claims.aud : [claims.aud]
  if (!aud.includes(expected.clientId)) throw new ChatgptAuthError("The sign-in token was not issued for this app.", "bad_audience")
  if (typeof claims.exp === "number" && claims.exp * 1000 < deps.now() - 60_000)
    throw new ChatgptAuthError("The sign-in token has expired. Try again.", "expired_id_token")
  if (expected.nonce !== undefined && claims.nonce !== expected.nonce)
    throw new ChatgptAuthError("The sign-in token did not match this sign-in. Try again.", "bad_nonce")

  const jwksRes = await deps.fetch(doc.jwks_uri, { signal: AbortSignal.timeout(15_000) })
  if (!jwksRes.ok) throw new ChatgptAuthError(`Could not read ChatGPT signing keys (HTTP ${jwksRes.status}).`, "jwks")
  const jwks = (await jwksRes.json()) as { keys?: any[] }
  const jwk = (jwks.keys ?? []).find((k) => k.kid === header.kid) ?? (jwks.keys?.length === 1 ? jwks.keys[0] : undefined)
  if (!jwk) throw new ChatgptAuthError("ChatGPT's signing key was not found.", "jwks")
  const [h, p, s] = idToken.split(".")
  const data = new TextEncoder().encode(`${h}.${p}`)
  const sig = Buffer.from(s!, "base64url")
  let ok = false
  if (header.alg === "RS256") {
    const key = await crypto.subtle.importKey("jwk", jwk, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"])
    ok = await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, sig, data)
  } else if (header.alg === "ES256") {
    const key = await crypto.subtle.importKey("jwk", jwk, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"])
    ok = await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key, sig, data)
  } else {
    throw new ChatgptAuthError(`Unsupported sign-in token algorithm ${header.alg}.`, "bad_alg")
  }
  if (!ok) throw new ChatgptAuthError("The sign-in token's signature did not verify.", "bad_signature")
  return claims
}

export function hasDirectScope(scopes: string[] | undefined): boolean {
  return !!scopes?.includes(DIRECT_SCOPE)
}

/** ChatGPT plan name from the ID token's `https://api.openai.com/auth` claim, if present. */
export function planFromClaims(claims: any): string | undefined {
  const raw = claims?.["https://api.openai.com/auth"]?.chatgpt_plan_type
  if (typeof raw !== "string" || !raw.trim()) return undefined
  const plan = raw.trim()
  return `ChatGPT ${plan.charAt(0).toUpperCase()}${plan.slice(1)}`
}

// ─── Stored state ─────────────────────────────────────────────────────────────

async function readJson<T>(deps: Deps, service: string, account: string): Promise<T | undefined> {
  const raw = await deps.store.read(service, account).catch(() => null)
  if (!raw) return undefined
  try {
    return JSON.parse(raw) as T
  } catch {
    return undefined
  }
}

/** This computer's host id; the P-256 key is made on first use and kept. */
export async function hostId(deps: Deps = defaultDeps()): Promise<string> {
  const saved = await readJson<{ privateJwk: JsonWebKey; publicJwk: JsonWebKey }>(deps, HOST_SERVICE, "host-key")
  if (saved?.publicJwk) return jwkThumbprintUrn(saved.publicJwk as any)
  const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair
  const privateJwk = await crypto.subtle.exportKey("jwk", pair.privateKey)
  const publicJwk = await crypto.subtle.exportKey("jwk", pair.publicKey)
  await deps.store.write(HOST_SERVICE, "host-key", JSON.stringify({ privateJwk, publicJwk }))
  return jwkThumbprintUrn(publicJwk as any)
}

export const loadRegistration = (deps: Deps = defaultDeps()) => readJson<Registration>(deps, SESSION_SERVICE, "registration")
export const loadSession = (deps: Deps = defaultDeps()) => readJson<Session>(deps, SESSION_SERVICE, "session")
export const saveSession = (session: Session, deps: Deps = defaultDeps()) =>
  deps.store.write(SESSION_SERVICE, "session", JSON.stringify(session))
const saveRegistration = (reg: Registration, deps: Deps) => deps.store.write(SESSION_SERVICE, "registration", JSON.stringify(reg))

// ─── Sign in ──────────────────────────────────────────────────────────────────

type Loopback = { redirectUri: string; next: Promise<URL>; stop: () => void }

/** Listen on 127.0.0.1:1455 (or a free port if it is busy) before the browser opens. */
export function startLoopback(port = LOOPBACK_PORT): Loopback {
  let resolve!: (u: URL) => void
  const next = new Promise<URL>((r) => (resolve = r))
  const handler = (req: Request) => {
    const url = new URL(req.url)
    if (url.pathname !== CALLBACK_PATH) return new Response("Not found", { status: 404 })
    resolve(url)
    const failed = url.searchParams.has("error")
    return new Response(
      `<!doctype html><meta charset="utf-8"><title>Gizzi Code</title><body style="font:16px system-ui;padding:40px">` +
        (failed ? "<p>Sign-in did not finish. You can close this tab.</p>" : "<p>Signed in to ChatGPT. You can close this tab and go back to the terminal.</p>") +
        "</body>",
      { headers: { "content-type": "text/html; charset=utf-8" } },
    )
  }
  let server: ReturnType<typeof Bun.serve>
  try {
    server = Bun.serve({ hostname: "127.0.0.1", port, fetch: handler })
  } catch {
    server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: handler })
  }
  return { redirectUri: `http://127.0.0.1:${server.port}${CALLBACK_PATH}`, next, stop: () => server.stop(true) }
}

export type SignInOptions = {
  /** Called with the authorize URL once the listener is up. */
  onAuthorizeUrl: (url: string) => void | Promise<void>
  /** Open the system browser (default true). */
  openBrowser?: boolean
  timeoutMs?: number
  signal?: AbortSignal
}

export async function signIn(opts: SignInOptions, deps: Deps = defaultDeps()): Promise<Session> {
  const host = await hostId(deps)
  const reg = await loadRegistration(deps)
  const clientId = reg?.ext_agent_host_id === host && reg.client_id ? reg.client_id : DYNAMIC_CLIENT
  const loop = startLoopback()
  try {
    const state = randomToken()
    const nonce = randomToken()
    const pkce = await pkcePair()
    const url = authorizeUrl({
      clientId,
      hostId: host,
      redirectUri: loop.redirectUri,
      state,
      nonce,
      challenge: pkce.challenge,
      idTokenHint: reg?.id_token,
      loginHint: reg?.email,
    })
    await opts.onAuthorizeUrl(url)
    if (opts.openBrowser !== false) {
      const { default: open } = await import("open")
      await open(url).catch(() => undefined)
    }
    const timeout = opts.timeoutMs ?? SIGN_IN_TIMEOUT_MS
    const callback = await new Promise<URL>((resolve, reject) => {
      const timer = setTimeout(() => reject(new ChatgptAuthError("Sign-in timed out. Run the command again.", "timeout")), timeout)
      opts.signal?.addEventListener("abort", () => reject(new ChatgptAuthError("Sign-in was stopped.", "aborted")), { once: true })
      loop.next.then(
        (u) => (clearTimeout(timer), resolve(u)),
        (e) => (clearTimeout(timer), reject(e)),
      )
    })
    const result = readCallback(callback, { state, clientId })
    const tokens = await tokenCall(deps, exchangeForm({ clientId: result.clientId, code: result.code, verifier: pkce.verifier, redirectUri: loop.redirectUri }))
    if (!tokens.id_token) throw new ChatgptAuthError("ChatGPT sign-in sent no ID token.", "no_id_token")
    if (!tokens.refresh_token) throw new ChatgptAuthError("ChatGPT sign-in sent no refresh token.", "no_refresh_token")
    const claims = await verifyIdToken(deps, tokens.id_token, { clientId: result.clientId, nonce })
    const scopes = tokens.scope ? tokens.scope.split(/\s+/).filter(Boolean) : (result.scope ?? [])
    if (!hasDirectScope(scopes))
      throw new ChatgptAuthError("ChatGPT did not allow plan use for this sign-in (missing chatgpt.tokens.use.direct).", "scope_missing")
    const now = deps.now()
    const session: Session = {
      email: typeof claims.email === "string" ? claims.email : undefined,
      issuer: claims.iss,
      subject: String(claims.sub),
      client_id: result.clientId,
      ext_agent_host_id: host,
      id_token: tokens.id_token,
      access_token: tokens.access_token,
      refresh_token: tokens.refresh_token,
      token_type: tokens.token_type ?? "Bearer",
      expires_at: now + (tokens.expires_in ?? 3600) * 1000,
      scopes,
      saved_at: new Date(now).toISOString(),
      plan: planFromClaims(claims),
    }
    await saveRegistration({ client_id: result.clientId, ext_agent_host_id: host, email: session.email, subject: session.subject, id_token: tokens.id_token }, deps)
    await saveSession(session, deps)
    return session
  } finally {
    loop.stop()
  }
}

// ─── Access token (refresh) ───────────────────────────────────────────────────

let inflight: Promise<Session> | undefined

async function withLock<T>(deps: Deps, fn: () => Promise<T>): Promise<T> {
  fs.mkdirSync(deps.lockDir, { recursive: true, mode: 0o700 })
  const lock = path.join(deps.lockDir, "chatgpt-refresh.lock")
  const started = Date.now()
  for (;;) {
    try {
      fs.writeFileSync(lock, String(process.pid), { flag: "wx", mode: 0o600 })
      break
    } catch {
      try {
        if (Date.now() - fs.statSync(lock).mtimeMs > 30_000) fs.rmSync(lock, { force: true })
      } catch {}
      if (Date.now() - started > 35_000) throw new ChatgptAuthError("Another Gizzi is refreshing the ChatGPT sign-in. Try again.", "lock")
      await new Promise((r) => setTimeout(r, 150))
    }
  }
  try {
    return await fn()
  } finally {
    fs.rmSync(lock, { force: true })
  }
}

/** Refresh now, serialized; terminal refresh errors clear the tokens and ask for a new sign-in. */
export async function refreshSession(deps: Deps = defaultDeps()): Promise<Session> {
  return withLock(deps, async () => {
    // Another process may have refreshed while this one waited.
    const current = await loadSession(deps)
    if (!current) throw new ChatgptAuthError("Not signed in to ChatGPT. Run `gizzi connect chatgpt`.", "signed_out")
    if (current.expires_at - deps.now() > REFRESH_MARGIN_MS) return current
    try {
      const tokens = await tokenCall(deps, refreshForm(current))
      const next = applyRefresh(current, tokens, deps.now())
      await saveSession(next, deps)
      return next
    } catch (e) {
      if (e instanceof ChatgptAuthError && TERMINAL_REFRESH.has(e.code)) {
        await deps.store.remove(SESSION_SERVICE, "session").catch(() => undefined)
        throw new ChatgptAuthError("Your ChatGPT sign-in expired. Run `gizzi connect chatgpt` to sign in again.", "login_expired")
      }
      throw e
    }
  })
}

/** A usable session: refreshed when its access token is within two minutes of expiry. */
export async function usableSession(deps: Deps = defaultDeps()): Promise<Session> {
  const session = await loadSession(deps)
  if (!session) throw new ChatgptAuthError("Not signed in to ChatGPT. Run `gizzi connect chatgpt`.", "signed_out")
  if (session.expires_at - deps.now() > REFRESH_MARGIN_MS) return session
  inflight ??= refreshSession(deps).finally(() => (inflight = undefined))
  return inflight
}

// ─── Sign out ─────────────────────────────────────────────────────────────────

/** Revoke the refresh token, then delete local tokens. `revoked:false` = remote revoke not confirmed. */
export async function signOut(deps: Deps = defaultDeps()): Promise<{ revoked: boolean; wasSignedIn: boolean }> {
  const session = await loadSession(deps)
  if (!session) return { revoked: true, wasSignedIn: false }
  let revoked = false
  try {
    const doc = await discovery(deps)
    const endpoint: string | undefined = doc.revocation_endpoint
    if (endpoint) {
      for (let attempt = 0; attempt < 3 && !revoked; attempt++) {
        try {
          const res = await deps.fetch(endpoint, {
            method: "POST",
            headers: { "content-type": "application/x-www-form-urlencoded" },
            body: new URLSearchParams({ token: session.refresh_token, token_type_hint: "refresh_token", client_id: session.client_id }).toString(),
            signal: AbortSignal.timeout(15_000),
          })
          if (res.ok) revoked = true
          else if (res.status < 500) break
        } catch {}
        if (!revoked && attempt < 2) await new Promise((r) => setTimeout(r, 500 * 2 ** attempt))
      }
    }
  } catch {}
  await deps.store.remove(SESSION_SERVICE, "session").catch(() => undefined)
  return { revoked, wasSignedIn: true }
}
