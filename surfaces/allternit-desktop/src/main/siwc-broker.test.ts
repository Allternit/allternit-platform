import { afterEach, describe, expect, it } from 'vitest';
import { SIWC_BROKER_TOKEN_ENV, SIWC_BROKER_URL_ENV, startSiwcBroker, type SiwcBroker } from './siwc-broker.js';
import type { SiwcManager } from './siwc.js';

function fakeManager(over: Partial<SiwcManager> = {}): SiwcManager {
  return {
    status: () => ({ enabled: true, state: 'signed_in' }),
    signIn: async () => ({ enabled: true, state: 'signed_in' }),
    cancelSignIn: () => undefined,
    signOut: async () => ({ enabled: true, state: 'signed_out', revocationConfirmed: true }),
    accessToken: async () => ({ token: 'at-1', expiresAt: 123, email: 'a@example.com' }),
    models: async () => [{ slug: 'gpt-x', display_name: 'GPT X' }],
    onStatusChange: () => () => undefined,
    ...over,
  };
}

let broker: SiwcBroker | undefined;
afterEach(() => broker?.close());

async function call(path: string, token?: string, method = 'GET') {
  const url = broker!.env()[SIWC_BROKER_URL_ENV];
  const headers: Record<string, string> = token ? { Authorization: `Bearer ${token}` } : {};
  const res = await fetch(`${url}${path}`, { method, headers });
  return { status: res.status, body: (await res.json()) as any };
}

describe('siwc broker', () => {
  it('serves the access token only to a caller with the launch secret', async () => {
    broker = await startSiwcBroker(fakeManager());
    const secret = broker.env()[SIWC_BROKER_TOKEN_ENV];
    expect((await call('/v1/token')).status).toBe(401);
    expect((await call('/v1/token', 'wrong')).status).toBe(401);
    const ok = await call('/v1/token', secret);
    expect(ok).toEqual({ status: 200, body: { access_token: 'at-1', expires_at: 123, email: 'a@example.com' } });
    expect((await call('/v1/token', secret, 'POST')).status).toBe(405);
  });

  it('never exposes refresh or ID tokens', async () => {
    broker = await startSiwcBroker(fakeManager());
    const { body } = await call('/v1/token', broker.env()[SIWC_BROKER_TOKEN_ENV]);
    expect(Object.keys(body).sort()).toEqual(['access_token', 'email', 'expires_at']);
  });

  it('answers 409 with the state when SIWC is off or signed out', async () => {
    broker = await startSiwcBroker(fakeManager({
      accessToken: async () => null,
      status: () => ({ enabled: false, state: 'disabled' }),
    }));
    const r = await call('/v1/token', broker.env()[SIWC_BROKER_TOKEN_ENV]);
    expect(r).toEqual({ status: 409, body: { error: 'not_available', state: 'disabled' } });
  });

  it('lists models for the signed-in account', async () => {
    broker = await startSiwcBroker(fakeManager());
    const r = await call('/v1/models', broker.env()[SIWC_BROKER_TOKEN_ENV]);
    expect(r.body.models).toEqual([{ slug: 'gpt-x', display_name: 'GPT X' }]);
  });
});
