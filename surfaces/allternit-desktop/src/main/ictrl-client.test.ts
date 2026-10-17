import { afterEach, describe, expect, it } from 'vitest';
import { randomBytes } from 'node:crypto';
import {
  IctrlClient,
  IctrlError,
  ictrlStoreKey,
  keystreamXor,
  openResponse,
  sealRequest,
  envelopeKeys,
  hmac,
  tokenIdFor,
} from './ictrl-client.js';
import { FakeIctrlServer, memorySecrets, type FakeIctrlOptions } from './ictrl-client.testkit.js';

let server: FakeIctrlServer | null = null;
afterEach(async () => {
  await server?.close();
  server = null;
});

async function setup(opts: FakeIctrlOptions = {}, clientNow?: () => number) {
  server = new FakeIctrlServer({ approve: 'once', handlers: { device_info: () => ({ device_name: 'Test iPhone', system_version: '26.6.2' }) }, ...opts });
  const host = await server.listen();
  const secrets = memorySecrets();
  const client = new IctrlClient({ host, secrets, storeKey: ictrlStoreKey('UDID-1'), now: clientNow });
  return { host, secrets, client, server };
}

describe('envelope primitives', () => {
  it('keystream XOR is its own inverse and spans blocks', () => {
    const key = randomBytes(32);
    const nonce = randomBytes(16);
    const data = randomBytes(100);
    expect(keystreamXor(key, nonce, keystreamXor(key, nonce, data))).toEqual(data);
    expect(keystreamXor(key, nonce, Buffer.alloc(0))).toEqual(Buffer.alloc(0));
  });

  it('matches the Swift construction: tag over v1||tok||be64(ts)||nonce||ct', () => {
    const key = Buffer.alloc(32, 7);
    const nonce = Buffer.alloc(16, 1);
    const env = sealRequest(key, '{"a":1}', 1_700_000_000, nonce);
    const k = envelopeKeys(key, nonce);
    const ct = Buffer.from(env.ct, 'base64');
    const ts = Buffer.alloc(8);
    ts.writeBigUInt64BE(1_700_000_000n);
    const expected = hmac(k.mac, Buffer.concat([Buffer.from('v1'), Buffer.from(tokenIdFor(key), 'hex'), ts, nonce, ct]));
    expect(env.tag).toBe(expected.toString('hex'));
    expect(keystreamXor(k.encReq, nonce, ct).toString()).toBe('{"a":1}');
  });

  it('rejects a reply whose tag was altered', () => {
    const key = randomBytes(32);
    const nonce = randomBytes(16);
    const k = envelopeKeys(key, nonce);
    const ct = keystreamXor(k.encResp, nonce, Buffer.from('{}'));
    const tag = hmac(k.mac, Buffer.concat([Buffer.from('resp'), nonce, ct]));
    expect(openResponse(key, nonce, { ct: ct.toString('base64'), tag: tag.toString('hex') })).toBe('{}');
    tag[3] ^= 0xff;
    expect(() => openResponse(key, nonce, { ct: ct.toString('base64'), tag: tag.toString('hex') })).toThrow(IctrlError);
  });
});

describe('IctrlClient against a fake runner', () => {
  it('pairs over X25519, stores the key only in the secret store, and calls', async () => {
    const { client, secrets, server } = await setup();
    expect(client.hasKey()).toBe(false);
    await expect(client.call('device_info')).rejects.toMatchObject({ code: 'no_key' });
    const record = await client.pair('Allternit Desktop', { deviceId: 'dev-1' });
    expect(record.tok).toMatch(/^[0-9a-f]{8}$/);
    expect(server.sessions.has(record.tok)).toBe(true);
    expect(server.sessions.get(record.tok)!.toString('hex')).toBe(record.key);
    expect(JSON.parse(secrets.map.get(ictrlStoreKey('UDID-1'))!).key).toBe(record.key);
    expect(await client.call('device_info')).toEqual({ device_name: 'Test iPhone', system_version: '26.6.2' });
  });

  it('the one-shot auto approval is consumed by the first pairing', async () => {
    const { client, host } = await setup();
    await client.pair('first');
    const other = new IctrlClient({ host, secrets: memorySecrets(), storeKey: 'x' });
    await expect(other.pair('second')).rejects.toMatchObject({ code: 'pair_denied', extra: { status: 403 } });
  });

  it('turns JSON-RPC errors into rpc_error', async () => {
    const { client } = await setup();
    await client.pair('a');
    await expect(client.call('nope')).rejects.toMatchObject({ code: 'rpc_error', extra: { rpcCode: -32601 } });
  });

  it('rejects a replayed nonce', async () => {
    const fixed = Buffer.alloc(16, 9);
    const { host, client } = await setup();
    await client.pair('a');
    const secrets = memorySecrets();
    secrets.set('k', JSON.stringify(client.record()));
    const replaying = new IctrlClient({ host, secrets, storeKey: 'k', random: () => fixed });
    await replaying.call('device_info');
    await expect(replaying.call('device_info')).rejects.toMatchObject({ code: 'nonce_replayed', extra: { status: 401 } });
  });

  it('corrects for a skewed clock once, from server_ts', async () => {
    const realNow = Date.now();
    const { client, server } = await setup({ now: () => realNow }, () => realNow + 10 * 60_000);
    await client.pair('a');
    await expect(client.call('device_info')).resolves.toMatchObject({ device_name: 'Test iPhone' });
    // first attempt was refused for skew, the retry went through
    expect(server.calls.filter((c) => c.method === 'device_info')).toHaveLength(1);
  });

  it('reports ts_skew when the skew is not fixable by one retry', async () => {
    let serverNow = Date.now();
    const { client } = await setup({ now: () => (serverNow += 10 * 60_000) });
    await client.pair('a');
    await expect(client.call('device_info')).rejects.toMatchObject({ code: 'ts_skew' });
  });

  it('a key the phone no longer knows asks for re-pairing', async () => {
    const { client, server } = await setup();
    const record = await client.pair('a');
    server.sessions.delete(record.tok);
    const error = await client.call('device_info').catch((e: IctrlError) => e);
    expect(error).toBeInstanceOf(IctrlError);
    expect((error as IctrlError).needsRepair).toBe(true);
  });

  it('discards a reply with a bad MAC', async () => {
    const { client } = await setup({ tamperResponses: true });
    await client.pair('a');
    await expect(client.call('device_info')).rejects.toMatchObject({ code: 'bad_response_mac' });
  });

  it('withHost keeps the key and remembers the address that answered', async () => {
    const { host, secrets } = await setup();
    const usb = new IctrlClient({ host: '127.0.0.1:1', secrets, storeKey: ictrlStoreKey('UDID-1') });
    const paired = await new IctrlClient({ host, secrets, storeKey: ictrlStoreKey('UDID-1') }).pair('a');
    secrets.set(ictrlStoreKey('UDID-1'), JSON.stringify({ ...paired, addresses: ['127.0.0.1:1'] }));
    const lan = usb.withHost(host);
    await lan.call('device_info');
    expect(lan.record()!.addresses).toEqual([host, '127.0.0.1:1']);
  });

  it('config turns on remote access', async () => {
    const { client, server } = await setup();
    await client.pair('a');
    const result = await client.call<{ address: string }>('config', { allow_remote_access: true });
    expect(server.remoteAccess).toBe(true);
    expect(result.address).toBe('192.168.1.20:8200');
  });

  it('unreachable host is a typed error', async () => {
    const client = new IctrlClient({ host: '127.0.0.1:1', secrets: memorySecrets(), storeKey: 'k', timeoutMs: 1000 });
    await expect(client.whoami()).rejects.toMatchObject({ code: 'unreachable' });
  });
});
