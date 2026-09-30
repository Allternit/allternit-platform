import { createSign, generateKeyPairSync } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import {
  SIWC_ISSUER,
  SIWC_PLAN_SCOPE,
  SIWC_SCOPES,
  buildAuthorizeUrl,
  createSiwcManager,
  planUsageGranted,
  redactAuthorizeUrl,
  validateIdToken,
  type SiwcHost,
} from './siwc.js';

const ISSUED = 'oaiapp_test123';
const { publicKey, privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'k1', alg: 'RS256', use: 'sig' };

function idToken(claims: Record<string, unknown>, opts: { key?: typeof privateKey } = {}): string {
  const enc = (o: unknown) => Buffer.from(JSON.stringify(o)).toString('base64url');
  const head = enc({ alg: 'RS256', kid: 'k1', typ: 'JWT' });
  const body = enc({ iss: SIWC_ISSUER, aud: ISSUED, exp: Math.floor(Date.now() / 1000) + 3600, sub: 'user-1', email: 'a@example.com', ...claims });
  const sig = createSign('RSA-SHA256').update(`${head}.${body}`).sign(opts.key ?? privateKey).toString('base64url');
  return `${head}.${body}.${sig}`;
}

interface Fake {
  host: SiwcHost;
  secrets: Map<string, string>;
  flag: { on: boolean };
  clock: { t: number };
  opened: string[];
  calls: Array<{ url: string; form: Record<string, string> }>;
  tokenResponses: Array<() => Response | Promise<Response>>;
  revokeStatus: { code: number };
  /** What the "browser" does after the authorize URL opens. */
  browser: { mode: 'approve' | 'deny' | 'badstate'; scope?: string; sub?: string; nonceOverride?: string; clientId?: string | null };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } }) as unknown as Response;
}

function tokenBody(over: Record<string, unknown> = {}, nonce = 'n'): Record<string, unknown> {
  return {
    access_token: 'at-1',
    refresh_token: 'rt-1',
    id_token: idToken({ nonce }),
    token_type: 'Bearer',
    expires_in: 3600,
    scope: SIWC_SCOPES.join(' '),
    ...over,
  };
}

function setup(): Fake {
  const secrets = new Map<string, string>();
  const f: Fake = {
    secrets,
    flag: { on: true },
    clock: { t: Date.now() },
    opened: [],
    calls: [],
    tokenResponses: [],
    revokeStatus: { code: 200 },
    browser: { mode: 'approve' },
    host: undefined as unknown as SiwcHost,
  };
  const fakeFetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const url = String(input);
    const form = init?.body ? Object.fromEntries(new URLSearchParams(String(init.body))) : {};
    f.calls.push({ url, form });
    if (url.endsWith('/.well-known/openid-configuration')) {
      return json({ jwks_uri: `${SIWC_ISSUER}/jwks`, revocation_endpoint: `${SIWC_ISSUER}/revoke` });
    }
    if (url.endsWith('/jwks')) return json({ keys: [jwk] });
    if (url.endsWith('/revoke')) return new Response('', { status: f.revokeStatus.code }) as unknown as Response;
    if (url.endsWith('/oauth/token')) {
      const next = f.tokenResponses.shift();
      if (!next) throw new Error('no token response queued');
      return next();
    }
    if (url.endsWith('/v1/models')) {
      return json({ models: [
        { slug: 'gpt-x', display_name: 'GPT X', visibility: 'list' },
        { slug: 'hidden', display_name: 'Hidden', visibility: 'hide' },
      ] });
    }
    throw new Error(`unexpected fetch ${url}`);
  }) as typeof fetch;

  f.host = {
    flagEnabled: () => f.flag.on,
    readSecret: (k) => secrets.get(k) ?? null,
    writeSecret: (k, v) => void secrets.set(k, v),
    deleteSecret: (k) => void secrets.delete(k),
    fetch: fakeFetch,
    now: () => f.clock.t,
    retryDelayMs: 0,
    openExternal: (url) => {
      f.opened.push(url);
      const u = new URL(url);
      const redirect = u.searchParams.get('redirect_uri')!;
      const state = u.searchParams.get('state')!;
      const nonce = u.searchParams.get('nonce')!;
      // The "browser" returns to the loopback callback shortly after.
      setTimeout(() => {
        const cb = new URL(redirect);
        if (f.browser.mode === 'deny') {
          cb.searchParams.set('error', 'access_denied');
          cb.searchParams.set('state', state);
        } else if (f.browser.mode === 'badstate') {
          cb.searchParams.set('code', 'code-x');
          cb.searchParams.set('state', 'wrong');
        } else {
          cb.searchParams.set('code', 'code-1');
          cb.searchParams.set('state', state);
          const cid = f.browser.clientId === undefined ? ISSUED : f.browser.clientId;
          if (cid) cb.searchParams.set('client_id', cid);
          f.tokenResponses.push(() => json(tokenBody(
            { scope: f.browser.scope ?? SIWC_SCOPES.join(' '), id_token: idToken({ nonce: f.browser.nonceOverride ?? nonce, ...(f.browser.sub ? { sub: f.browser.sub } : {}) }) },
          )));
        }
        void fetch(cb).catch(() => undefined);
        if (f.browser.mode === 'badstate') {
          // A wrong-state callback is ignored; the real one then completes the flow.
          setTimeout(() => {
            const real = new URL(redirect);
            real.searchParams.set('code', 'code-1');
            real.searchParams.set('state', state);
            real.searchParams.set('client_id', ISSUED);
            f.tokenResponses.push(() => json(tokenBody({ id_token: idToken({ nonce }) })));
            void fetch(real).catch(() => undefined);
          }, 30);
        }
      }, 10);
    },
  };
  return f;
}

async function signedIn(f: Fake) {
  const m = createSiwcManager(f.host);
  await m.signIn();
  return m;
}

describe('flag gate', () => {
  it('does nothing while the flag is off', async () => {
    const f = setup();
    f.flag.on = false;
    const m = createSiwcManager(f.host);
    expect(m.status()).toEqual({ enabled: false, state: 'disabled' });
    await expect(m.signIn()).rejects.toMatchObject({ code: 'flag_off' });
    expect(await m.accessToken()).toBeNull();
    expect(f.opened).toEqual([]);
    expect(f.calls).toEqual([]);
    expect(f.secrets.size).toBe(0);
  });

  it('stops handing out tokens the moment the flag turns off', async () => {
    const f = setup();
    const m = await signedIn(f);
    expect((await m.accessToken())?.token).toBe('at-1');
    f.flag.on = false;
    expect(await m.accessToken()).toBeNull();
    expect(m.status().state).toBe('disabled');
  });
});

describe('authorize URL', () => {
  it('carries the documented parameters for first-time registration', () => {
    const url = new URL(buildAuthorizeUrl({
      clientId: 'dynamic_agent_client', hostId: 'urn:uuid:h', redirectUri: 'http://127.0.0.1:1455/auth/callback',
      state: 's', nonce: 'n', challenge: 'c', agentNameHint: 'Allternit',
    }));
    const q = url.searchParams;
    expect(url.origin + url.pathname).toBe('https://auth.openai.com/api/accounts/authorize');
    expect(q.get('client_id')).toBe('dynamic_agent_client');
    expect(q.get('agent_name_hint')).toBe('Allternit');
    expect(q.get('ext_agent_host_id')).toBe('urn:uuid:h');
    expect(q.get('response_type')).toBe('code');
    expect(q.get('scope')).toBe('openid profile email offline_access resource.invoke chatgpt.tokens.use.direct');
    expect(q.get('resource')).toBe('https://api.openai.com/v1');
    expect(q.get('code_challenge_method')).toBe('S256');
    expect(q.get('prompt')).toBeNull();
  });

  it('redacts id_token_hint for logs', () => {
    expect(redactAuthorizeUrl('https://x/a?id_token_hint=SECRET.JWT.PART&state=s')).toBe('https://x/a?id_token_hint=[redacted]&state=s');
  });
});

describe('validateIdToken', () => {
  const args = { jwks: [jwk], clientId: ISSUED, nonce: 'n', nowSeconds: Math.floor(Date.now() / 1000) };
  it('accepts a good token', () => {
    expect(validateIdToken(idToken({ nonce: 'n' }), args).sub).toBe('user-1');
  });
  it('rejects a wrong nonce, audience, issuer, expiry and signature', () => {
    expect(() => validateIdToken(idToken({ nonce: 'x' }), args)).toThrow(/nonce/);
    expect(() => validateIdToken(idToken({ nonce: 'n', aud: 'other' }), args)).toThrow(/audience/);
    expect(() => validateIdToken(idToken({ nonce: 'n', iss: 'https://evil' }), args)).toThrow(/issuer/);
    expect(() => validateIdToken(idToken({ nonce: 'n', exp: 1 }), args)).toThrow(/expired/);
    const other = generateKeyPairSync('rsa', { modulusLength: 2048 }).privateKey;
    expect(() => validateIdToken(idToken({ nonce: 'n' }, { key: other }), args)).toThrow(/signature/);
  });
  it('checks the plan-usage scope separately from identity', () => {
    expect(planUsageGranted(['openid', 'email'])).toBe(false);
    expect(planUsageGranted(['openid', SIWC_PLAN_SCOPE])).toBe(true);
  });
});

describe('sign in', () => {
  it('registers, exchanges with the issued client id, and stores credentials', async () => {
    const f = setup();
    const m = createSiwcManager(f.host);
    const s = await m.signIn();
    expect(s).toMatchObject({ enabled: true, state: 'signed_in', email: 'a@example.com' });

    const auth = new URL(f.opened[0]);
    expect(auth.searchParams.get('client_id')).toBe('dynamic_agent_client');
    expect(auth.searchParams.get('agent_name_hint')).toBe('Allternit');
    expect(auth.searchParams.get('redirect_uri')).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/auth\/callback$/);

    const exchange = f.calls.find((c) => c.url.endsWith('/oauth/token'))!;
    expect(exchange.form).toMatchObject({ grant_type: 'authorization_code', client_id: ISSUED, code: 'code-1', resource: 'https://api.openai.com/v1' });
    expect(exchange.form.code_verifier.length).toBeGreaterThanOrEqual(43);
    expect(exchange.form.client_secret).toBeUndefined();

    // Host id persisted (urn:uuid) and the credential record kept per client id.
    expect(f.secrets.get('siwc-host-id')).toMatch(/^urn:uuid:[0-9a-f-]{36}$/);
    const store = JSON.parse(f.secrets.get('siwc-credentials')!);
    expect(store.accounts[ISSUED]).toMatchObject({ client_id: ISSUED, subject: 'user-1', issuer: SIWC_ISSUER });
    expect(store.accounts[ISSUED].ext_agent_host_id).toBe(f.secrets.get('siwc-host-id'));
    // Status never exposes tokens.
    expect(JSON.stringify(s)).not.toContain('at-1');
  });

  it('marks plan usage disabled when the scope was not granted, and never serves a token', async () => {
    const f = setup();
    f.browser.scope = 'openid profile email';
    const m = createSiwcManager(f.host);
    expect((await m.signIn()).state).toBe('plan_usage_disabled');
    expect(await m.accessToken()).toBeNull();
  });

  it('stops without exchanging a code when consent is declined', async () => {
    const f = setup();
    f.browser.mode = 'deny';
    const m = createSiwcManager(f.host);
    await expect(m.signIn()).rejects.toMatchObject({ code: 'access_denied' });
    expect(f.calls.some((c) => c.url.endsWith('/oauth/token'))).toBe(false);
    expect(m.status().state).toBe('signed_out');
  });

  it('ignores a callback with the wrong state and finishes on the real one', async () => {
    const f = setup();
    f.browser.mode = 'badstate';
    const m = createSiwcManager(f.host);
    expect((await m.signIn()).state).toBe('signed_in');
    expect(f.calls.filter((c) => c.url.endsWith('/oauth/token'))).toHaveLength(1);
  });

  it('rejects an ID token whose nonce does not match, storing nothing', async () => {
    const f = setup();
    f.browser.nonceOverride = 'attacker';
    const m = createSiwcManager(f.host);
    await expect(m.signIn()).rejects.toMatchObject({ code: 'id_token_invalid' });
    expect(f.secrets.has('siwc-credentials')).toBe(false);
    expect(m.status().state).toBe('error');
  });

  it('treats a registration callback without an issued client id as incomplete', async () => {
    const f = setup();
    f.browser.clientId = null;
    const m = createSiwcManager(f.host);
    await expect(m.signIn()).rejects.toMatchObject({ code: 'registration_incomplete' });
  });
});

describe('refresh', () => {
  it('refreshes near expiry with the issued client id, keeps the grant, and rotates the token', async () => {
    const f = setup();
    const m = await signedIn(f);
    f.clock.t += 3600_000 - 30_000; // inside the 60s skew
    f.tokenResponses.push(() => json({ access_token: 'at-2', refresh_token: 'rt-2', token_type: 'Bearer', expires_in: 3600 }));
    const t = await m.accessToken();
    expect(t?.token).toBe('at-2');
    const refresh = f.calls.filter((c) => c.url.endsWith('/oauth/token')).at(-1)!;
    expect(refresh.form).toEqual({ grant_type: 'refresh_token', client_id: ISSUED, refresh_token: 'rt-1', resource: 'https://api.openai.com/v1' });
    const rec = JSON.parse(f.secrets.get('siwc-credentials')!).accounts[ISSUED];
    expect(rec.tokens).toMatchObject({ access_token: 'at-2', refresh_token: 'rt-2' });
    // Scopes survive a refresh response that omits `scope`.
    expect(rec.tokens.scopes).toContain(SIWC_PLAN_SCOPE);
  });

  it('serializes concurrent refreshes into one request', async () => {
    const f = setup();
    const m = await signedIn(f);
    f.clock.t += 3600_000;
    let n = 0;
    f.tokenResponses.push(() => { n++; return json({ access_token: 'at-2', refresh_token: 'rt-2', expires_in: 3600 }); });
    const [a, b, c] = await Promise.all([m.accessToken(), m.accessToken(), m.accessToken()]);
    expect([a?.token, b?.token, c?.token]).toEqual(['at-2', 'at-2', 'at-2']);
    expect(n).toBe(1);
  });

  it('does not refresh while the token is fresh', async () => {
    const f = setup();
    const m = await signedIn(f);
    const before = f.calls.length;
    expect((await m.accessToken())?.token).toBe('at-1');
    expect(f.calls.length).toBe(before);
  });

  it('clears unusable tokens on invalid_grant but keeps the client mapping', async () => {
    const f = setup();
    const m = await signedIn(f);
    f.clock.t += 3600_000;
    f.tokenResponses.push(() => json({ error: 'refresh_token_reused' }, 400));
    expect(await m.accessToken()).toBeNull();
    expect(m.status()).toMatchObject({ state: 'needs_reauth' });
    const rec = JSON.parse(f.secrets.get('siwc-credentials')!).accounts[ISSUED];
    expect(rec.tokens).toBeUndefined();
    expect(rec.client_id).toBe(ISSUED);
  });

  it('keeps credentials through a temporary failure', async () => {
    const f = setup();
    const m = await signedIn(f);
    f.clock.t += 3600_000;
    f.tokenResponses.push(() => { throw new Error('offline'); });
    expect(await m.accessToken()).toBeNull();
    const rec = JSON.parse(f.secrets.get('siwc-credentials')!).accounts[ISSUED];
    expect(rec.tokens.refresh_token).toBe('rt-1');
  });
});

describe('sign out', () => {
  it('revokes the refresh token, clears tokens, and keeps client + host id', async () => {
    const f = setup();
    const m = await signedIn(f);
    const hostId = f.secrets.get('siwc-host-id');
    const out = await m.signOut();
    const revoke = f.calls.find((c) => c.url.endsWith('/revoke'))!;
    expect(revoke.form).toEqual({ token: 'rt-1', token_type_hint: 'refresh_token', client_id: ISSUED });
    expect(out).toMatchObject({ state: 'signed_out', revocationConfirmed: true });
    const rec = JSON.parse(f.secrets.get('siwc-credentials')!).accounts[ISSUED];
    expect(rec.tokens).toBeUndefined();
    expect(f.secrets.get('siwc-host-id')).toBe(hostId);
    expect(await m.accessToken()).toBeNull();
  });

  it('still clears locally and says so when revocation is not confirmed', async () => {
    const f = setup();
    const m = await signedIn(f);
    f.revokeStatus.code = 503;
    const out = await m.signOut();
    expect(out.revocationConfirmed).toBe(false);
    expect(out.detail).toMatch(/did not confirm/);
    expect(JSON.parse(f.secrets.get('siwc-credentials')!).accounts[ISSUED].tokens).toBeUndefined();
  });

  it('signs back in with the saved client id and host id (no new registration)', async () => {
    const f = setup();
    const m = await signedIn(f);
    await m.signOut();
    await m.signIn();
    const again = new URL(f.opened[1]);
    expect(again.searchParams.get('client_id')).toBe(ISSUED);
    expect(again.searchParams.get('agent_name_hint')).toBeNull();
    expect(again.searchParams.get('login_hint')).toBe('a@example.com');
    expect(again.searchParams.get('ext_agent_host_id')).toBe(f.secrets.get('siwc-host-id'));
  });

  it('rejects a returning sign-in as a different identity without replacing credentials', async () => {
    const f = setup();
    const m = await signedIn(f);
    f.browser.sub = 'someone-else';
    await expect(m.signIn()).rejects.toMatchObject({ code: 'identity_mismatch' });
    expect(JSON.parse(f.secrets.get('siwc-credentials')!).accounts[ISSUED].subject).toBe('user-1');
  });

  it('asks for consent again only when the user enables plan usage', async () => {
    const f = setup();
    f.browser.scope = 'openid profile email';
    const m = createSiwcManager(f.host);
    await m.signIn();
    f.browser.scope = undefined;
    await m.signIn({ enablePlanUsage: true });
    expect(new URL(f.opened[1]).searchParams.get('prompt')).toBe('consent');
    expect(new URL(f.opened[0]).searchParams.get('prompt')).toBeNull();
    expect(m.status().state).toBe('signed_in');
  });
});

describe('models', () => {
  it('lists only visible models with the access token', async () => {
    const f = setup();
    const m = await signedIn(f);
    expect(await m.models()).toEqual([{ slug: 'gpt-x', display_name: 'GPT X' }]);
  });
});
