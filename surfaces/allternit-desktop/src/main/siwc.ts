/**
 * Sign in with ChatGPT (SIWC) — ChatGPT plan usage for Allternit Desktop.
 *
 * Implements OpenAI's documented open-source flow
 * (developers.openai.com/siwc/token-sharing-open-source): dynamic client
 * registration (`dynamic_agent_client` + `ext_agent_host_id`), Authorization
 * Code + PKCE through the system browser with a 127.0.0.1 loopback callback,
 * ID-token validation against OpenAI's JWKS, a `chatgpt.tokens.use.direct`
 * scope check, serialized rotating-token refresh, and refresh-token
 * revocation on sign out.
 *
 * Invariants:
 * - Feature-flagged (`feature.siwc`, default OFF). Every entry point checks
 *   the flag live, so turning it off stops all use at once.
 * - Desktop main process only. Tokens never cross IPC or reach a renderer:
 *   `status()` returns account metadata, `accessToken()` is for the local
 *   token broker (siwc-broker.ts) that feeds gizzi-code.
 * - Scopes are identity + ChatGPT plan usage only. It grants no access to the
 *   user's ChatGPT conversations, and nothing here reads them.
 * - Credentials live in the injected secret store (Electron safeStorage in
 *   production) and are never logged.
 *
 * Electron-free: the host injects storage, browser launch, fetch and time so
 * the whole flow is testable under plain Node with a fake OpenAI.
 */

import * as http from 'node:http';
import type { AddressInfo } from 'node:net';
import { createPublicKey, randomBytes, randomUUID, verify as cryptoVerify, type JsonWebKey } from 'node:crypto';
import { constantTimeEqual, generatePkce, generateState } from './mini-app-oauth-broker.js';

// ─── Documented constants ─────────────────────────────────────────────────────

export const SIWC_FLAG = 'feature.siwc';
export const SIWC_ISSUER = 'https://auth.openai.com';
export const SIWC_AUTHORIZE_URL = `${SIWC_ISSUER}/api/accounts/authorize`;
export const SIWC_TOKEN_URL = `${SIWC_ISSUER}/api/accounts/oauth/token`;
export const SIWC_DISCOVERY_URL = `${SIWC_ISSUER}/.well-known/openid-configuration`;
export const SIWC_RESOURCE = 'https://api.openai.com/v1';
export const SIWC_DYNAMIC_CLIENT_ID = 'dynamic_agent_client';
export const SIWC_AGENT_NAME = 'Allternit';
export const SIWC_PLAN_SCOPE = 'chatgpt.tokens.use.direct';
export const SIWC_SCOPES = [
  'openid', 'profile', 'email',
  'offline_access', 'resource.invoke', SIWC_PLAN_SCOPE,
];
export const SIWC_CALLBACK_PATH = '/auth/callback';
const PREFERRED_PORT = 1455;
const FLOW_TIMEOUT_MS = 5 * 60_000;
/** Refresh this long before the access token expires. */
const REFRESH_SKEW_MS = 60_000;
const CLOCK_LEEWAY_S = 60;
const HOST_ID_SECRET = 'siwc-host-id';
const CREDENTIALS_SECRET = 'siwc-credentials';

/** Refresh errors that mean the refresh token is unusable (re-run OAuth). */
const UNUSABLE_REFRESH_ERRORS = new Set([
  'invalid_grant', 'invalid_refresh_token', 'token_expired',
  'refresh_token_expired', 'refresh_token_invalidated', 'refresh_token_reused',
]);

// ─── Types ────────────────────────────────────────────────────────────────────

export interface SiwcHost {
  /** Live feature-flag read (`feature.siwc`). */
  flagEnabled(): boolean;
  readSecret(key: string): string | null;
  writeSecret(key: string, value: string): void;
  deleteSecret(key: string): void;
  openExternal(url: string): void | Promise<void>;
  fetch?: typeof fetch;
  now?: () => number;
  /** Backoff between revocation retries (tests pass 0). */
  retryDelayMs?: number;
  logger?: (message: string) => void;
}

export type SiwcState =
  | 'disabled'
  | 'signed_out'
  | 'signing_in'
  | 'signed_in'
  /** Signed in, but the user has not granted ChatGPT plan usage. */
  | 'plan_usage_disabled'
  /** The saved session is no longer usable; sign in again. */
  | 'needs_reauth'
  | 'error';

export interface SiwcStatus {
  enabled: boolean;
  state: SiwcState;
  email?: string;
  /** ISO time the current access token expires. */
  expiresAt?: string;
  detail?: string;
}

interface Registration {
  client_id: string;
  ext_agent_host_id: string;
  issuer: string;
  subject: string;
  email?: string;
}

interface Tokens {
  id_token: string;
  access_token: string;
  refresh_token?: string;
  token_type: string;
  expires_in: number;
  scopes: string[];
  saved_at: string;
  /** Epoch ms the access token expires. */
  expires_at: number;
}

/** One protected credential record per issued client id + verified identity. */
interface CredentialRecord extends Registration {
  tokens?: Tokens;
}

interface Store {
  version: 1;
  active?: string;
  accounts: Record<string, CredentialRecord>;
}

export class SiwcError extends Error {
  constructor(message: string, readonly code: string) {
    super(message);
    this.name = 'SiwcError';
  }
}

// ─── Pure helpers (test targets) ──────────────────────────────────────────────

export function hostIdFor(uuid: string): string {
  return `urn:uuid:${uuid}`;
}

export interface AuthorizeParams {
  clientId: string;
  hostId: string;
  redirectUri: string;
  state: string;
  nonce: string;
  challenge: string;
  /** Only for first-time registration. */
  agentNameHint?: string;
  idTokenHint?: string;
  loginHint?: string;
  forceConsent?: boolean;
}

export function buildAuthorizeUrl(p: AuthorizeParams): string {
  const url = new URL(SIWC_AUTHORIZE_URL);
  const q = url.searchParams;
  q.set('client_id', p.clientId);
  if (p.agentNameHint) q.set('agent_name_hint', p.agentNameHint);
  q.set('ext_agent_host_id', p.hostId);
  if (p.idTokenHint) q.set('id_token_hint', p.idTokenHint);
  if (p.loginHint) q.set('login_hint', p.loginHint);
  q.set('response_type', 'code');
  q.set('redirect_uri', p.redirectUri);
  q.set('scope', SIWC_SCOPES.join(' '));
  q.set('resource', SIWC_RESOURCE);
  q.set('state', p.state);
  q.set('nonce', p.nonce);
  q.set('code_challenge_method', 'S256');
  q.set('code_challenge', p.challenge);
  // Only when the user explicitly asks to (re)enable plan usage.
  if (p.forceConsent) q.set('prompt', 'consent');
  return url.toString();
}

/** Redact `id_token_hint` before a URL goes anywhere near a log. */
export function redactAuthorizeUrl(url: string): string {
  return url.replace(/([?&]id_token_hint=)[^&]*/g, '$1[redacted]');
}

function b64urlJson(part: string): Record<string, unknown> {
  return JSON.parse(Buffer.from(part, 'base64url').toString('utf8')) as Record<string, unknown>;
}

interface Jwk extends JsonWebKey { kid?: string; alg?: string; use?: string }

export interface IdTokenClaims {
  sub: string;
  email?: string;
  aud: string | string[];
  iss: string;
  exp: number;
  nonce?: string;
}

/**
 * Verify an ID token: signature against the published JWKS (RS256/ES256),
 * issuer, audience == the issued client id, expiry, and the attempt's nonce.
 */
export function validateIdToken(
  idToken: string,
  opts: { jwks: Jwk[]; clientId: string; nonce: string; nowSeconds: number },
): IdTokenClaims {
  const parts = idToken.split('.');
  if (parts.length !== 3) throw new SiwcError('ID token is malformed', 'id_token_invalid');
  let header: Record<string, unknown>;
  let claims: IdTokenClaims;
  try {
    header = b64urlJson(parts[0]);
    claims = b64urlJson(parts[1]) as unknown as IdTokenClaims;
  } catch {
    throw new SiwcError('ID token is malformed', 'id_token_invalid');
  }
  const alg = header.alg;
  if (alg !== 'RS256' && alg !== 'ES256') throw new SiwcError('ID token algorithm is not accepted', 'id_token_invalid');
  const candidates = opts.jwks.filter((k) => (header.kid ? k.kid === header.kid : true));
  const signed = Buffer.from(`${parts[0]}.${parts[1]}`);
  const signature = Buffer.from(parts[2], 'base64url');
  const ok = candidates.some((jwk) => {
    try {
      const key = createPublicKey({ key: jwk, format: 'jwk' });
      return alg === 'RS256'
        ? cryptoVerify('RSA-SHA256', signed, key, signature)
        : cryptoVerify('SHA256', signed, { key, dsaEncoding: 'ieee-p1363' }, signature);
    } catch {
      return false;
    }
  });
  if (!ok) throw new SiwcError('ID token signature could not be verified', 'id_token_invalid');
  if (claims.iss !== SIWC_ISSUER) throw new SiwcError('ID token issuer is wrong', 'id_token_invalid');
  const aud = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
  if (!aud.includes(opts.clientId)) throw new SiwcError('ID token audience is wrong', 'id_token_invalid');
  if (typeof claims.exp !== 'number' || claims.exp + CLOCK_LEEWAY_S < opts.nowSeconds) {
    throw new SiwcError('ID token has expired', 'id_token_invalid');
  }
  if (!claims.nonce || !constantTimeEqual(claims.nonce, opts.nonce)) {
    throw new SiwcError('ID token nonce does not match', 'id_token_invalid');
  }
  if (!claims.sub) throw new SiwcError('ID token has no subject', 'id_token_invalid');
  return claims;
}

export function planUsageGranted(scopes: string[]): boolean {
  return scopes.includes(SIWC_PLAN_SCOPE);
}

// ─── Manager ──────────────────────────────────────────────────────────────────

interface PendingFlow {
  state: string;
  nonce: string;
  verifier: string;
  redirectUri: string;
  /** The saved client id for a returning sign-in; absent for first-time registration. */
  savedClientId?: string;
  server: http.Server;
  timer: NodeJS.Timeout;
}

export interface SiwcManager {
  status(): SiwcStatus;
  /** Start (or resume) sign-in in the system browser. Resolves when it ends. */
  signIn(opts?: { enablePlanUsage?: boolean }): Promise<SiwcStatus>;
  cancelSignIn(): void;
  /** Revoke the renewable session and clear local tokens; keeps the client mapping and host id. */
  signOut(): Promise<SiwcStatus & { revocationConfirmed: boolean }>;
  /** A usable access token (refreshing if due), or null. Broker use only. */
  accessToken(): Promise<{ token: string; expiresAt: number; email?: string } | null>;
  /** Live model catalog for the signed-in account (`GET /v1/models`, visibility=list). */
  models(): Promise<Array<{ slug: string; display_name: string }>>;
  onStatusChange(listener: (status: SiwcStatus) => void): () => void;
}

export function createSiwcManager(host: SiwcHost): SiwcManager {
  const doFetch: typeof fetch = host.fetch ?? ((...a) => fetch(...a));
  const now = host.now ?? Date.now;
  const log = (m: string) => host.logger?.(`[siwc] ${m}`);
  const listeners = new Set<(s: SiwcStatus) => void>();

  let pending: PendingFlow | null = null;
  let refreshing: Promise<Tokens | null> | null = null;
  let transient: { state: SiwcState; detail: string } | null = null;
  let discovery: { revocation_endpoint?: string; jwks_uri: string } | null = null;
  let jwksCache: Jwk[] | null = null;

  // ── storage ────────────────────────────────────────────────────────────────

  function load(): Store {
    const raw = host.readSecret(CREDENTIALS_SECRET);
    if (!raw) return { version: 1, accounts: {} };
    try {
      const parsed = JSON.parse(raw) as Store;
      if (parsed?.version === 1 && parsed.accounts && typeof parsed.accounts === 'object') return parsed;
    } catch { /* fall through */ }
    return { version: 1, accounts: {} };
  }

  function save(store: Store): void {
    host.writeSecret(CREDENTIALS_SECRET, JSON.stringify(store));
  }

  function activeRecord(store = load()): CredentialRecord | undefined {
    return store.active ? store.accounts[store.active] : undefined;
  }

  /** Chosen and persisted before the first sign-in; never derived from the user. */
  function hostId(): string {
    let id = host.readSecret(HOST_ID_SECRET);
    if (!id) {
      id = hostIdFor(randomUUID());
      host.writeSecret(HOST_ID_SECRET, id);
    }
    return id;
  }

  // ── status ─────────────────────────────────────────────────────────────────

  function status(): SiwcStatus {
    if (!host.flagEnabled()) return { enabled: false, state: 'disabled' };
    if (pending) return { enabled: true, state: 'signing_in' };
    const rec = activeRecord();
    const base = { enabled: true, ...(rec?.email ? { email: rec.email } : {}) };
    if (transient) return { ...base, state: transient.state, detail: transient.detail };
    if (!rec?.tokens) return { ...base, state: 'signed_out' };
    const common = { ...base, expiresAt: new Date(rec.tokens.expires_at).toISOString() };
    if (!planUsageGranted(rec.tokens.scopes)) {
      return { ...common, state: 'plan_usage_disabled', detail: 'Signed in, but ChatGPT plan usage is not enabled.' };
    }
    return { ...common, state: 'signed_in' };
  }

  function emit(): void {
    const s = status();
    for (const l of listeners) {
      try { l(s); } catch { /* a listener never breaks the flow */ }
    }
  }

  // ── OpenID discovery + JWKS ────────────────────────────────────────────────

  async function getDiscovery() {
    if (discovery) return discovery;
    const res = await doFetch(SIWC_DISCOVERY_URL);
    if (!res.ok) throw new SiwcError(`OpenAI discovery failed (${res.status})`, 'discovery_failed');
    const doc = (await res.json()) as { revocation_endpoint?: string; jwks_uri?: string };
    if (!doc.jwks_uri) throw new SiwcError('OpenAI discovery has no jwks_uri', 'discovery_failed');
    discovery = { revocation_endpoint: doc.revocation_endpoint, jwks_uri: doc.jwks_uri };
    return discovery;
  }

  async function getJwks(refetch = false): Promise<Jwk[]> {
    if (jwksCache && !refetch) return jwksCache;
    const { jwks_uri } = await getDiscovery();
    const res = await doFetch(jwks_uri);
    if (!res.ok) throw new SiwcError(`OpenAI JWKS fetch failed (${res.status})`, 'discovery_failed');
    jwksCache = ((await res.json()) as { keys: Jwk[] }).keys ?? [];
    return jwksCache;
  }

  async function validateWithJwks(idToken: string, clientId: string, nonce: string): Promise<IdTokenClaims> {
    const args = { clientId, nonce, nowSeconds: Math.floor(now() / 1000) };
    try {
      return validateIdToken(idToken, { ...args, jwks: await getJwks() });
    } catch (err) {
      // A rotated signing key: refetch the JWKS once before giving up.
      if (err instanceof SiwcError && err.code === 'id_token_invalid') {
        return validateIdToken(idToken, { ...args, jwks: await getJwks(true) });
      }
      throw err;
    }
  }

  // ── token endpoint ─────────────────────────────────────────────────────────

  async function postToken(form: Record<string, string>): Promise<Record<string, any>> {
    const res = await doFetch(SIWC_TOKEN_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded', Accept: 'application/json' },
      body: new URLSearchParams(form).toString(),
    });
    let body: Record<string, any> = {};
    try { body = (await res.json()) as Record<string, any>; } catch { /* non-JSON error body */ }
    if (!res.ok) {
      const code = typeof body.error === 'string' ? body.error : `http_${res.status}`;
      throw new SiwcError(`Token request failed (${res.status} ${code})`, code);
    }
    return body;
  }

  function toTokens(body: Record<string, any>, previous?: Tokens): Tokens {
    if (typeof body.access_token !== 'string' || !body.access_token) {
      throw new SiwcError('Token response has no access token', 'token_response_invalid');
    }
    const expiresIn = Number(body.expires_in ?? 3600);
    return {
      id_token: typeof body.id_token === 'string' ? body.id_token : previous?.id_token ?? '',
      access_token: body.access_token,
      refresh_token: typeof body.refresh_token === 'string' ? body.refresh_token : previous?.refresh_token,
      token_type: typeof body.token_type === 'string' ? body.token_type : 'Bearer',
      expires_in: expiresIn,
      scopes: typeof body.scope === 'string' ? body.scope.split(/\s+/).filter(Boolean).sort() : previous?.scopes ?? [],
      saved_at: new Date(now()).toISOString(),
      expires_at: now() + expiresIn * 1000,
    };
  }

  // ── sign in ────────────────────────────────────────────────────────────────

  async function completeExchange(flow: PendingFlow, code: string, clientId: string, isNew: boolean): Promise<void> {
    const body = await postToken({
      grant_type: 'authorization_code',
      client_id: clientId,
      code,
      code_verifier: flow.verifier,
      redirect_uri: flow.redirectUri,
      resource: SIWC_RESOURCE,
    });
    const tokens = toTokens(body);
    if (!tokens.id_token) throw new SiwcError('Token response has no ID token', 'id_token_invalid');
    const claims = await validateWithJwks(tokens.id_token, clientId, flow.nonce);
    const store = load();
    const existing = store.accounts[clientId];
    // A returning sign-in must be the same verified identity: never replace
    // one account's credentials with another's.
    if (!isNew && existing && existing.subject !== claims.sub) {
      throw new SiwcError('Signed in as a different ChatGPT account; the saved account was not changed.', 'identity_mismatch');
    }
    store.accounts[clientId] = {
      client_id: clientId,
      ext_agent_host_id: hostId(),
      issuer: SIWC_ISSUER,
      subject: claims.sub,
      email: claims.email ?? existing?.email,
      tokens,
    };
    store.active = clientId;
    save(store);
    transient = null;
  }

  function signIn(opts: { enablePlanUsage?: boolean } = {}): Promise<SiwcStatus> {
    if (!host.flagEnabled()) return Promise.reject(new SiwcError('Sign in with ChatGPT is not enabled.', 'flag_off'));
    if (pending) return Promise.reject(new SiwcError('A sign-in is already in progress.', 'in_progress'));

    return new Promise<SiwcStatus>((resolve, reject) => {
      const saved = activeRecord();
      const flowBase = { ...generatePkce(), state: generateState(), nonce: randomBytes(24).toString('base64url') };
      const server = http.createServer();
      const clean = () => {
        if (pending?.server === server) {
          clearTimeout(pending.timer);
          pending = null;
        }
        server.closeAllConnections?.();
        server.close();
      };
      const fail = (error: SiwcError) => {
        clean();
        transient = error.code === 'access_denied' ? null : { state: 'error', detail: error.message };
        emit();
        reject(error);
      };

      server.on('request', (req, res) => {
        const flow = pending;
        const url = new URL(req.url ?? '/', 'http://127.0.0.1');
        if (!flow || url.pathname !== SIWC_CALLBACK_PATH) {
          res.writeHead(404).end();
          return;
        }
        const page = (msg: string) => {
          res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
          res.end(`<!doctype html><meta charset="utf-8"><title>Allternit</title><body style="font:15px system-ui;padding:48px;background:#fff;color:#111">${msg}</body>`);
        };
        if (!constantTimeEqual(url.searchParams.get('state') ?? '', flow.state)) {
          page('This sign-in link is not valid. You can close this tab.');
          return; // not our attempt; keep waiting for the real callback
        }
        const oauthError = url.searchParams.get('error');
        if (oauthError) {
          page('Sign-in was not completed. You can close this tab and return to Allternit.');
          fail(new SiwcError(
            oauthError === 'access_denied' ? 'Sign-in was declined.' : `Sign-in failed (${oauthError}).`,
            oauthError,
          ));
          return;
        }
        const code = url.searchParams.get('code');
        const returnedClient = url.searchParams.get('client_id');
        const isNew = !flow.savedClientId;
        if (!code) {
          page('Sign-in did not return a code. You can close this tab.');
          fail(new SiwcError('Sign-in did not return an authorization code.', 'no_code'));
          return;
        }
        // New registration must return the issued client id; a returning
        // sign-in must not come back with a different one.
        if (isNew && (!returnedClient || returnedClient === SIWC_DYNAMIC_CLIENT_ID)) {
          page('Registration did not complete. You can close this tab.');
          fail(new SiwcError('Registration did not return a client id.', 'registration_incomplete'));
          return;
        }
        if (!isNew && returnedClient && returnedClient !== flow.savedClientId) {
          page('Sign-in did not match the saved account. You can close this tab.');
          fail(new SiwcError('Sign-in returned a different client id; rejected.', 'client_mismatch'));
          return;
        }
        const clientId = isNew ? returnedClient! : flow.savedClientId!;
        completeExchange(flow, code, clientId, isNew).then(
          () => {
            page('Signed in. You can close this tab and return to Allternit.');
            clean();
            emit();
            resolve(status());
          },
          (err) => {
            page('Sign-in could not be finished. You can close this tab.');
            fail(err instanceof SiwcError ? err : new SiwcError(String(err?.message ?? err), 'exchange_failed'));
          },
        );
      });

      const listen = (port: number) => new Promise<void>((ok, bad) => {
        server.once('error', bad);
        server.listen(port, '127.0.0.1', () => { server.off('error', bad); ok(); });
      });

      (async () => {
        const id = hostId(); // persisted before the first sign-in
        try {
          await listen(PREFERRED_PORT);
        } catch {
          await listen(0); // later sign-ins may use another port; only the port varies
        }
        const port = (server.address() as AddressInfo).port;
        const redirectUri = `http://127.0.0.1:${port}${SIWC_CALLBACK_PATH}`;
        const authorize = buildAuthorizeUrl({
          clientId: saved?.client_id ?? SIWC_DYNAMIC_CLIENT_ID,
          hostId: id,
          redirectUri,
          state: flowBase.state,
          nonce: flowBase.nonce,
          challenge: flowBase.challenge,
          agentNameHint: saved ? undefined : SIWC_AGENT_NAME,
          // Reauthorization hints: the retained ID token identifies the
          // account; the saved email pre-selects it.
          idTokenHint: saved?.tokens?.id_token || undefined,
          loginHint: saved?.email,
          forceConsent: opts.enablePlanUsage === true && Boolean(saved),
        });
        pending = {
          state: flowBase.state,
          nonce: flowBase.nonce,
          verifier: flowBase.verifier,
          redirectUri,
          savedClientId: saved?.client_id,
          server,
          timer: setTimeout(() => fail(new SiwcError('Sign-in timed out.', 'timeout')), FLOW_TIMEOUT_MS),
        };
        transient = null;
        emit();
        log(`opening browser for ${saved ? 'returning' : 'first-time'} sign-in`);
        await host.openExternal(authorize);
      })().catch((err) => fail(err instanceof SiwcError ? err : new SiwcError(String(err?.message ?? err), 'start_failed')));
    });
  }

  function cancelSignIn(): void {
    if (!pending) return;
    const { server, timer } = pending;
    clearTimeout(timer);
    pending = null;
    server.closeAllConnections?.();
    server.close();
    transient = null;
    emit();
  }

  // ── refresh (serialized: the refresh token rotates) ────────────────────────

  function refreshTokens(rec: CredentialRecord): Promise<Tokens | null> {
    if (refreshing) return refreshing;
    const run = async (): Promise<Tokens | null> => {
      const refreshToken = rec.tokens?.refresh_token;
      if (!refreshToken) {
        markNeedsReauth(rec.client_id, 'The saved session has no refresh token.');
        return null;
      }
      try {
        const body = await postToken({
          grant_type: 'refresh_token',
          client_id: rec.client_id, // the issued id, never dynamic_agent_client
          refresh_token: refreshToken,
          resource: SIWC_RESOURCE, // scope omitted: the grant is retained
        });
        const next = toTokens(body, rec.tokens);
        // Replace access token, expiry, scopes and rotating refresh token together.
        const store = load();
        const current = store.accounts[rec.client_id];
        if (current) {
          current.tokens = next;
          save(store);
        }
        if (transient?.state === 'error') transient = null;
        emit();
        return next;
      } catch (err) {
        const code = err instanceof SiwcError ? err.code : 'network';
        if (UNUSABLE_REFRESH_ERRORS.has(code)) {
          markNeedsReauth(rec.client_id, 'Your ChatGPT session ended. Sign in again.');
          return null;
        }
        if (code === 'invalid_client') {
          transient = { state: 'error', detail: 'OpenAI rejected the client registration (invalid_client).' };
          emit();
          return null;
        }
        // Temporary network or infrastructure failure: keep credentials.
        log(`refresh failed transiently (${code})`);
        return null;
      }
    };
    refreshing = run().finally(() => { refreshing = null; });
    return refreshing;
  }

  function markNeedsReauth(clientId: string, detail: string): void {
    const store = load();
    const rec = store.accounts[clientId];
    if (rec) {
      delete rec.tokens; // clear unusable tokens; keep the client mapping
      save(store);
    }
    transient = { state: 'needs_reauth', detail };
    emit();
  }

  async function accessToken() {
    if (!host.flagEnabled()) return null;
    const rec = activeRecord();
    let tokens = rec?.tokens;
    if (!rec || !tokens || !planUsageGranted(tokens.scopes)) return null;
    if (tokens.expires_at - now() <= REFRESH_SKEW_MS) {
      tokens = (await refreshTokens(rec)) ?? undefined;
      if (!tokens || tokens.expires_at <= now()) return null;
    }
    return { token: tokens.access_token, expiresAt: tokens.expires_at, email: rec.email };
  }

  async function models() {
    const t = await accessToken();
    if (!t) throw new SiwcError('Not signed in with ChatGPT plan usage.', 'not_signed_in');
    const res = await doFetch(`${SIWC_RESOURCE}/models`, { headers: { Authorization: `Bearer ${t.token}` } });
    if (!res.ok) throw new SiwcError(`Model list failed (${res.status})`, `http_${res.status}`);
    const body = (await res.json()) as { models?: Array<{ slug: string; display_name: string; visibility?: string }> };
    return (body.models ?? [])
      .filter((m) => m.visibility === 'list')
      .map((m) => ({ slug: m.slug, display_name: m.display_name }));
  }

  // ── sign out ───────────────────────────────────────────────────────────────

  async function revoke(clientId: string, refreshToken: string): Promise<boolean> {
    const delay = host.retryDelayMs ?? 500;
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        const { revocation_endpoint } = await getDiscovery();
        if (!revocation_endpoint) return false;
        const res = await doFetch(revocation_endpoint, {
          method: 'POST',
          headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
          body: new URLSearchParams({ token: refreshToken, token_type_hint: 'refresh_token', client_id: clientId }).toString(),
        });
        if (res.status === 200) return true;
        if (res.status < 500) return false; // a 4xx will not improve by retrying
      } catch { /* network failure: retry with backoff */ }
      if (delay) await new Promise((r) => setTimeout(r, delay * 2 ** attempt));
    }
    return false;
  }

  async function signOut() {
    cancelSignIn();
    const store = load();
    const rec = activeRecord(store);
    let revocationConfirmed = true;
    if (rec?.tokens?.refresh_token) {
      revocationConfirmed = await revoke(rec.client_id, rec.tokens.refresh_token);
    }
    if (rec) {
      // Clear access, refresh and ID tokens; retain the client mapping + host id.
      const fresh = load();
      const current = fresh.accounts[rec.client_id];
      if (current) {
        delete current.tokens;
        save(fresh);
      }
    }
    transient = null;
    log(`signed out (remote revocation ${revocationConfirmed ? 'confirmed' : 'not confirmed'})`);
    emit();
    return {
      ...status(),
      revocationConfirmed,
      ...(revocationConfirmed ? {} : { detail: 'Signed out here. OpenAI did not confirm the disconnect — you can disconnect Allternit in ChatGPT Settings.' }),
    };
  }

  return {
    status,
    signIn,
    cancelSignIn,
    signOut,
    accessToken,
    models,
    onStatusChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
