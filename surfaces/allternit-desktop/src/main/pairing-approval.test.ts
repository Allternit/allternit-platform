import { describe, expect, it, vi } from 'vitest';
import { approvePairing, describePairingRequest, type PairingApprovalDeps } from './pairing-approval.js';

const BASE = 'https://api.example/api/v1/runtime-pairings/code/ABCD-1234';

function deps(overrides: Partial<PairingApprovalDeps> & { routes?: Record<string, Response> } = {}) {
  const calls: Array<{ url: string; init?: RequestInit }> = [];
  const routes = overrides.routes ?? {};
  const d: PairingApprovalDeps = {
    cloudApiBase: 'https://api.example/',
    hostedPairUrl: (c) => `https://platform.example/pair?code=${c}`,
    getClerkToken: async () => 'clerk-jwt',
    fetch: (async (input: string, init?: RequestInit) => {
      calls.push({ url: String(input), init });
      return routes[`${init?.method ?? 'GET'} ${input}`] ?? new Response('{}', { status: 404 });
    }) as PairingApprovalDeps["fetch"],
    confirm: vi.fn(async () => true),
    openBrowser: vi.fn(),
    notify: vi.fn(),
    account: { email: 'me@example.com' },
    ...overrides,
  };
  return { d, calls };
}

const pending = () => new Response(JSON.stringify({ userCode: 'ABCD-1234', name: 'gizzi', hostname: 'mac', status: 'pending' }), { status: 200 });

describe('gizzi login approval in Desktop', () => {
  it('opens the hosted page in the browser when there is no Clerk token', async () => {
    const { d, calls } = deps({ getClerkToken: async () => null });
    expect(await approvePairing('ABCD-1234', d)).toBe('browser');
    expect(d.openBrowser).toHaveBeenCalledWith('https://platform.example/pair?code=ABCD-1234');
    expect(calls).toHaveLength(0);
  });

  it('asks, then approves with the Clerk token', async () => {
    const { d, calls } = deps({ routes: { [`GET ${BASE}`]: pending(), [`POST ${BASE}/approve`]: new Response('{}', { status: 200 }) } });
    expect(await approvePairing('ABCD-1234', d)).toBe('approved');
    expect(d.confirm).toHaveBeenCalledOnce();
    const approve = calls.find((c) => c.url.endsWith('/approve'))!;
    expect(new Headers(approve.init?.headers).get('Authorization')).toBe('Bearer clerk-jwt');
    expect(JSON.parse(String(approve.init?.body)).email).toBe('me@example.com');
    expect(d.openBrowser).not.toHaveBeenCalled();
  });

  it('denies when the person cancels', async () => {
    const { d, calls } = deps({ confirm: async () => false, routes: { [`GET ${BASE}`]: pending(), [`POST ${BASE}/deny`]: new Response('{}') } });
    expect(await approvePairing('ABCD-1234', d)).toBe('cancelled');
    expect(calls.map((c) => c.url)).toEqual([BASE, `${BASE}/deny`]);
  });

  it('falls back to the browser when the token is rejected', async () => {
    const { d } = deps({ routes: { [`GET ${BASE}`]: new Response('', { status: 401 }) } });
    expect(await approvePairing('ABCD-1234', d)).toBe('browser');
    expect(d.confirm).not.toHaveBeenCalled();
  });

  it('reports an expired code without asking', async () => {
    const { d } = deps({ routes: { [`GET ${BASE}`]: new Response('', { status: 410 }) } });
    expect(await approvePairing('ABCD-1234', d)).toBe('failed');
    expect(d.confirm).not.toHaveBeenCalled();
    expect(d.notify).toHaveBeenCalled();
  });

  it('describes the device and code', () => {
    expect(describePairingRequest({ userCode: 'ABCD-1234', name: 'gizzi', hostname: 'mac' })).toContain('Device: gizzi · mac');
  });
});
