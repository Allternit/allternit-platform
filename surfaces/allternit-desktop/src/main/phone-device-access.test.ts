import { describe, expect, it } from 'vitest';
import { AppPhoneBackend, deviceRpc, type AppPhoneNode, type AppRpcResult } from './phone-app-store.js';
import type { ApprovalDecision } from './phone-tools.js';

const SERIAL = 'app:5b1f0c3e-8d2a-4c11-9a7e-0f6b2d4c9e21';

function backend(rpc: AppRpcResult, decision: ApprovalDecision) {
  const calls: string[] = [];
  const node: AppPhoneNode = {
    listPhones: async () => [],
    rpc: async (_id, method) => (calls.push(method), rpc),
    openViewer: async () => { throw new Error('no viewer'); },
    decidePair: async () => ({ ok: true }),
  };
  const apps = new AppPhoneBackend({
    node,
    streams: { start: async () => undefined, stop: async () => undefined, status: () => null, subscribe: () => () => undefined, consumerCount: () => 0 } as never,
    decodeStill: async () => { throw new Error('no decode'); },
    gate: { request: async () => decision },
  });
  return { apps, calls };
}

describe('Device Access tools', () => {
  it('validates arguments before anything leaves the Desktop', () => {
    expect(deviceRpc('phone.calendar.list', { from: '2026-11-01T00:00:00Z', to: '2027-03-01T00:00:00Z' })).toMatchObject({ ok: false, error: 'invalid_args' });
    expect(deviceRpc('phone.calendar.create', { title: 'x', start: 'nope' })).toMatchObject({ ok: false });
    expect(deviceRpc('phone.contacts.search', { query: ' ' })).toMatchObject({ ok: false });
    const ok = deviceRpc('phone.calendar.create', { title: 'Lunch', start: '2026-11-02T12:00:00Z' });
    expect(ok).toMatchObject({ method: 'device.calendar.create', write: true });
    expect(deviceRpc('phone.contacts.search', { query: 'ann' })).toMatchObject({ method: 'device.contacts.search', write: false });
  });

  it('asks the person before a write and never sends it when declined', async () => {
    const { apps, calls } = backend({ ok: true, result: { created: true } }, 'denied');
    const out = await apps.call('phone.reminders.create', SERIAL, { title: 'Call mum' });
    expect(out).toMatchObject({ ok: false, error: 'approval_denied' });
    expect(calls).toEqual([]);
  });

  it('reads need no approval and a switched-off type is a clear tool error', async () => {
    const off = backend({ ok: false, status: 200, code: 'device_access_off', message: '' }, 'denied');
    expect(await off.apps.call('phone.calendar.list', SERIAL, { from: '2026-11-01T00:00:00Z', to: '2026-11-08T00:00:00Z' })).toMatchObject({ ok: false, error: 'device_access_off' });
    const closed = backend({ ok: false, status: 409, code: 'app_not_open', message: '' }, 'denied');
    expect(await closed.apps.call('phone.contacts.search', SERIAL, { query: 'ann' })).toMatchObject({ ok: false, error: 'app_not_open' });
  });
});
