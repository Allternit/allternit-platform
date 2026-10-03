import * as http from 'node:http';
import { afterEach, describe, expect, it } from 'vitest';
import { PHONE_GATEWAY_TOKEN_HEADER, startPhoneGateway } from './phone-gateway.js';
import { PhoneToolRunner, type AdbCall } from './phone-tools.js';

const closers: Array<() => void> = [];
afterEach(() => closers.splice(0).forEach((c) => c()));

async function boot() {
  const adb = (async () => ({ stdout: '', stderr: '', code: 0 })) as unknown as AdbCall;
  adb.binary = async () => Buffer.alloc(0);
  const runner = new PhoneToolRunner({
    adb,
    gate: { request: async () => 'denied' },
    rates: { load: () => [], save: () => {} },
    artemis: () => null,
    onlineSerials: () => ['a:1'],
  });
  const gw = await startPhoneGateway({ token: 'secret-token', runner });
  closers.push(gw.close);
  return gw.url;
}

describe('phone gateway', () => {
  it('rejects requests without the token', async () => {
    const url = await boot();
    expect((await fetch(`${url}/phone/tools`)).status).toBe(403);
    expect((await fetch(`${url}/phone/tools`, { headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'nope' } })).status).toBe(403);
  });

  it('lists the tools the spec requires', async () => {
    const url = await boot();
    const body = (await (await fetch(`${url}/phone/tools`, { headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'secret-token' } })).json()) as { tools: Array<{ name: string }> };
    expect(body.tools.map((t) => t.name)).toEqual(
      expect.arrayContaining(['phone.screenshot', 'phone.tap', 'phone.type', 'phone.swipe', 'phone.open_app', 'phone.task', 'phone.sms.send', 'phone.call.dial']),
    );
  });

  it('runs a tool call, and the approval gate still applies through the gateway', async () => {
    const url = await boot();
    const post = (body: unknown) =>
      fetch(`${url}/phone/call`, { method: 'POST', headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'secret-token' }, body: JSON.stringify(body) });
    expect(await (await post({ name: 'phone.tap', arguments: { x: 1, y: 2 } })).json()).toEqual({ ok: true });
    expect(await (await post({ name: 'phone.sms.send', arguments: { to: '+15550102030', text: 'hi', subscription_id: 1 } })).json()).toMatchObject({ ok: false, error: 'approval_denied' });
    expect((await fetch(`${url}/phone/call`, { method: 'POST', headers: { [PHONE_GATEWAY_TOKEN_HEADER]: 'secret-token' }, body: '{' })).status).toBe(400);
  });

  it('is loopback only', async () => {
    const url = await boot();
    expect(new URL(url).hostname).toBe('127.0.0.1');
    void http;
  });
});
