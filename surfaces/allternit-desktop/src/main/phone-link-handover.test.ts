import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SecretStore } from './ictrl-client.js';
import type { IosLinkRecord, IosLinkStore } from './phone-link-ios.js';
import type { AndroidLinkRecord, AndroidLinkStore } from './phone-link-android.js';
import {
  HANDOVER_API_PATH,
  HandoverError,
  exportPairing,
  handOver,
  handoverKey,
  handoverTargets,
  importHandover,
  openHandover,
  sealHandover,
  type HandoverKey,
  type HandoverClientDeps,
  type HandoverStores,
} from './phone-link-handover.js';
import { handedOverHealth, PhoneLinkHealthMonitor } from './phone-link-health.js';
import { PHONE_GATEWAY_TOKEN_HEADER, startPhoneGateway } from './phone-gateway.js';
import { PhoneToolRunner, type AdbCall } from './phone-tools.js';
import { linkedPhoneSource } from './phone-link-backend.js';

function memSecrets(): SecretStore & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return { map, get: (k) => map.get(k) ?? null, set: (k, v) => void map.set(k, v), delete: (k) => void map.delete(k) };
}
function memIos(): IosLinkStore {
  const m = new Map<string, IosLinkRecord>();
  return { load: (u) => m.get(u) ?? null, all: () => [...m.values()], save: (r) => void m.set(r.udid, r) };
}
function memAndroid(): AndroidLinkStore {
  const m = new Map<string, AndroidLinkRecord>();
  return { load: (d) => m.get(d) ?? null, all: () => [...m.values()], save: (r) => void m.set(r.deviceId, r) };
}
function stores(): HandoverStores & { secrets: ReturnType<typeof memSecrets> } {
  return { iosStore: memIos(), androidStore: memAndroid(), secrets: memSecrets() };
}

const IPHONE: IosLinkRecord = {
  udid: '00008110-AAAA',
  deviceId: 'ios-dev-1',
  deviceName: 'Eoj’s iPhone',
  runnerBundleId: 'com.allternit.ictrl.xctrunner',
  hostBundleId: 'com.allternit.ictrl',
  xctestConfig: 'cfg',
  testToRun: 'test',
  expiresAt: '2026-10-20T00:00:00.000Z',
  meshHost: '100.64.0.9:8200',
  lanHost: '192.168.1.20:8200',
  linkedAt: '2026-10-17T00:00:00.000Z',
};
const MOTO: AndroidLinkRecord = { deviceId: 'moto-1', serial: 'ZY22', deviceName: 'moto g play', meshHost: '100.64.0.7:8200', pairedAt: '2026-10-16T00:00:00.000Z' };
const KEY = JSON.stringify({ tok: 't'.repeat(16), key: 'a'.repeat(64), addresses: ['192.168.1.20:8200'] });

function mac(): HandoverStores & { secrets: ReturnType<typeof memSecrets> } {
  const s = stores();
  s.iosStore.save(IPHONE);
  s.androidStore.save(MOTO);
  s.secrets.set(`phone-link.ictrl.${IPHONE.udid}`, KEY);
  s.secrets.set(`phone-link.ictrl.android:${MOTO.deviceId}`, KEY.replace('a', 'b'));
  return s;
}

const FROM = { name: 'Eoj’s MacBook Pro', at: '2026-10-17T10:00:00.000Z' };

describe('pairing handover: seal, open, import', () => {
  it('moves an iPhone pairing (record + key) to the receiving computer, re-encrypted by its own store', () => {
    const giver = mac();
    const receiver = stores();
    const key = handoverKey(receiver.secrets, 'user_1');
    const sealed = sealHandover(exportPairing(giver, 'ios-dev-1', FROM), key, 'user_1');
    // Nothing secret is readable on the wire.
    expect(JSON.stringify(sealed)).not.toContain('a'.repeat(64));
    expect(JSON.stringify(sealed)).not.toContain(IPHONE.udid);
    const got = importHandover(receiver, openHandover(sealed, receiver.secrets, 'user_1'), () => new Date('2026-10-17T11:00:00.000Z'));
    expect(got).toEqual({ deviceId: 'ios-dev-1', platform: 'ios', name: 'Eoj’s iPhone' });
    expect(receiver.secrets.get(`phone-link.ictrl.${IPHONE.udid}`)).toBe(KEY);
    expect(receiver.iosStore.load(IPHONE.udid)).toMatchObject({ meshHost: '100.64.0.9:8200', handedOverFrom: { name: 'Eoj’s MacBook Pro', at: '2026-10-17T11:00:00.000Z' } });
    // The Mac keeps its own pairing.
    expect(giver.secrets.get(`phone-link.ictrl.${IPHONE.udid}`)).toBe(KEY);
  });

  it("never brings the other computer's Call audio settings (its sound devices aren't here)", () => {
    const giver = mac();
    giver.iosStore.save({ ...IPHONE, callBridge: { enabled: true, botId: 'b', inputDeviceId: 'i', inputLabel: 'USB', outputDeviceId: 'o', outputLabel: 'USB' } });
    const receiver = stores();
    const sealed = sealHandover(exportPairing(giver, 'ios-dev-1', FROM), handoverKey(receiver.secrets, 'user_1'), 'user_1');
    importHandover(receiver, openHandover(sealed, receiver.secrets, 'user_1'));
    expect(receiver.iosStore.load(IPHONE.udid)?.callBridge).toBeUndefined();
  });

  it('moves an Android pairing too', () => {
    const receiver = stores();
    const sealed = sealHandover(exportPairing(mac(), 'moto-1', FROM), handoverKey(receiver.secrets, null), 'user_1');
    expect(importHandover(receiver, openHandover(sealed, receiver.secrets, null))).toMatchObject({ deviceId: 'moto-1', platform: 'android' });
    expect(receiver.secrets.get('phone-link.ictrl.android:moto-1')).toContain('b');
  });

  it('keeps one key pair per computer across calls', () => {
    const s = stores();
    expect(handoverKey(s.secrets, null).keyId).toBe(handoverKey(s.secrets, null).keyId);
  });

  it('refuses to seal for a computer signed in to another account', () => {
    const key = handoverKey(stores().secrets, 'user_2');
    expect(() => sealHandover(exportPairing(mac(), 'ios-dev-1', FROM), key, 'user_1')).toThrow(HandoverError);
  });

  it('refuses to open a box from another owner, for another computer, or changed on the way', () => {
    const receiver = stores();
    const key = handoverKey(receiver.secrets, null);
    const sealed = sealHandover(exportPairing(mac(), 'ios-dev-1', FROM), key, 'user_1');
    expect(() => openHandover(sealed, receiver.secrets, 'user_2')).toThrow(/different Allternit account/);
    expect(() => openHandover(sealed, stores().secrets, null)).toThrow(/different computer/);
    // Rewriting the owner in the clear fails the AAD check.
    expect(() => openHandover({ ...sealed, owner: 'user_2' }, receiver.secrets, null)).toThrow(/could not be opened/);
    const ct = Buffer.from(sealed.ct, 'base64');
    ct[0] ^= 1;
    expect(() => openHandover({ ...sealed, ct: ct.toString('base64') }, receiver.secrets, null)).toThrow(/could not be opened/);
  });

  it('says so when the phone is not linked here or has no key', () => {
    expect(() => exportPairing(stores(), 'nope', FROM)).toThrow(/isn’t linked/);
    const s = mac();
    s.secrets.delete(`phone-link.ictrl.${IPHONE.udid}`);
    expect(() => exportPairing(s, 'ios-dev-1', FROM)).toThrow(/no key/);
  });
});

describe('pairing handover: the Mac side calls', () => {
  function client(over: Partial<HandoverClientDeps> & { receiver?: HandoverStores } = {}) {
    const receiver = over.receiver ?? stores();
    const posted: unknown[] = [];
    const fetchImpl = vi.fn(async (url: string, init?: RequestInit) => {
      expect(url).toBe(`http://api${HANDOVER_API_PATH}`);
      if (!init?.method) {
        return new Response(JSON.stringify({ computer: { id: 'cc_1', name: 'Sessions computer', running: true }, ready: true, key: handoverKey(receiver.secrets, 'user_1') }));
      }
      const body = JSON.parse(String(init.body)) as { computer_id: string; sealed: Parameters<typeof openHandover>[0] };
      posted.push(body);
      const got = importHandover(receiver, openHandover(body.sealed, receiver.secrets, 'user_1'));
      return new Response(JSON.stringify({ ok: true, ...got }));
    });
    const deps: HandoverClientDeps = {
      apiUrl: () => 'http://api',
      headers: () => ({ 'x-allternit-user-id': 'user_1' }),
      owner: () => 'user_1',
      stores: mac(),
      computerName: () => 'Eoj’s MacBook Pro',
      confirm: vi.fn(async () => true),
      fetchImpl: fetchImpl as unknown as typeof fetch,
      ...over,
    };
    return { deps, receiver, posted, fetchImpl };
  }

  it('lists the bound Sessions computer as the one target', async () => {
    const { deps } = client();
    expect(await handoverTargets(deps)).toEqual([{ computerId: 'cc_1', name: 'Sessions computer', ready: true }]);
  });

  it('lists nothing when no Sessions computer is bound, and a not-ready target with its reason', async () => {
    const none = client({ fetchImpl: (async () => new Response(JSON.stringify({ computer: null, ready: false }))) as unknown as typeof fetch });
    expect(await handoverTargets(none.deps)).toEqual([]);
    const off = client({
      fetchImpl: (async () => new Response(JSON.stringify({ computer: { id: 'cc_1', name: 'Sessions computer' }, ready: false, reason: 'Sessions computer is off.' }))) as unknown as typeof fetch,
    });
    expect(await handoverTargets(off.deps)).toEqual([{ computerId: 'cc_1', name: 'Sessions computer', ready: false, reason: 'Sessions computer is off.' }]);
  });

  it('asks on the Mac first, then seals and sends', async () => {
    const { deps, receiver, posted } = client();
    const result = await handOver(deps, { deviceId: 'ios-dev-1', computerId: 'cc_1' });
    expect(deps.confirm).toHaveBeenCalledWith({ name: 'Eoj’s iPhone', platform: 'ios' }, 'Sessions computer');
    expect(result).toEqual({ ok: true, deviceId: 'ios-dev-1', computerName: 'Sessions computer' });
    expect(posted).toHaveLength(1);
    expect(receiver.iosStore.load(IPHONE.udid)?.handedOverFrom?.name).toBe('Eoj’s MacBook Pro');
  });

  it('sends nothing when the person cancels, is signed out, or names another computer', async () => {
    const cancelled = client({ confirm: async () => false });
    expect(await handOver(cancelled.deps, { deviceId: 'ios-dev-1', computerId: 'cc_1' })).toMatchObject({ ok: false, cancelled: true });
    expect(cancelled.posted).toHaveLength(0);
    const signedOut = client({ owner: () => null });
    expect(await handOver(signedOut.deps, { deviceId: 'ios-dev-1', computerId: 'cc_1' })).toMatchObject({ ok: false, error: 'signed_out' });
    const other = client();
    expect(await handOver(other.deps, { deviceId: 'ios-dev-1', computerId: 'cc_someone_else' })).toMatchObject({ ok: false, error: 'not_your_sessions_computer' });
    expect(other.posted).toHaveLength(0);
  });
});

describe('pairing handover: the receiving phone gateway', () => {
  const closers: Array<() => void> = [];
  afterEach(() => closers.splice(0).forEach((c) => c()));

  it('serves its key and imports a sealed pairing, behind the gateway token', async () => {
    const receiver = stores();
    const adb = (async () => ({ stdout: '', stderr: '', code: 0 })) as unknown as AdbCall;
    adb.binary = async () => Buffer.alloc(0);
    const runner = new PhoneToolRunner({ adb, gate: { request: async () => 'denied' }, rates: { load: () => [], save: () => {} }, artemis: () => null, onlineSerials: () => [] });
    const gw = await startPhoneGateway({
      token: 'tkn',
      runner,
      handover: {
        key: () => handoverKey(receiver.secrets, 'user_1'),
        receive: async (sealed) => importHandover(receiver, openHandover(sealed, receiver.secrets, 'user_1')),
      },
    });
    closers.push(gw.close);
    const h = { [PHONE_GATEWAY_TOKEN_HEADER]: 'tkn', 'content-type': 'application/json' };
    expect((await fetch(`${gw.url}/phone/handover/key`)).status).toBe(403);
    const key = (await (await fetch(`${gw.url}/phone/handover/key`, { headers: h })).json()) as HandoverKey;
    const sealed = sealHandover(exportPairing(mac(), 'moto-1', FROM), key, 'user_1');
    const res = await fetch(`${gw.url}/phone/handover`, { method: 'POST', headers: h, body: JSON.stringify({ sealed }) });
    expect(await res.json()).toMatchObject({ ok: true, deviceId: 'moto-1', platform: 'android' });
    const bad = await fetch(`${gw.url}/phone/handover`, { method: 'POST', headers: h, body: JSON.stringify({ sealed: { ...sealed, keyId: 'x' } }) });
    expect(bad.status).toBe(400);
    expect(await bad.json()).toMatchObject({ ok: false, error: 'wrong_key' });
  });
});

describe('health for a handed-over iPhone names the Mac (Eoj: show it so it is understood)', () => {
  const phone = { platform: 'ios' as const, expiresAt: '2026-10-20T00:00:00.000Z', handedOverFrom: FROM };
  const base = { checkedAt: '2026-10-19T00:00:00.000Z' };

  it('signature renewal and remount point at the Mac and say why this computer cannot', () => {
    const sig = handedOverHealth({ ...base, state: 'SIGNATURE_RENEWAL_REQUIRED', recovery: 'manual', action: 'x' }, phone);
    expect(sig.recovery).toBe('manual');
    expect(sig.action).toBe('Your iPhone helper\'s signature runs out on October 20, 2026. Plug your iPhone into Eoj’s MacBook Pro and open Allternit there to renew it.');
    expect(sig.detail).toContain('can\'t renew the signature: that needs Eoj’s MacBook Pro and a cable');
    const remount = handedOverHealth({ ...base, state: 'DEVELOPER_IMAGE_REMOUNT_REQUIRED', recovery: 'manual', action: 'x' }, phone);
    expect(remount.action).toBe('Plug your iPhone into Eoj’s MacBook Pro and open Allternit there to remount its developer image.');
    const offline = handedOverHealth({ ...base, state: 'TRANSPORT_OFFLINE', recovery: 'automatic' }, phone);
    expect(offline.detail).toContain('If the iPhone restarted');
  });

  it('leaves phones linked on this computer, and healthy phones, as they are', () => {
    const own = { ...base, state: 'SIGNATURE_RENEWAL_REQUIRED' as const, recovery: 'manual' as const, action: 'own' };
    expect(handedOverHealth(own, { platform: 'ios', expiresAt: phone.expiresAt })).toBe(own);
    const ok = { ...base, state: 'HEALTHY' as const, recovery: 'automatic' as const };
    expect(handedOverHealth(ok, phone)).toBe(ok);
  });

  it('the health monitor applies it from the link record', async () => {
    const s = stores();
    s.iosStore.save({ ...IPHONE, handedOverFrom: FROM, expiresAt: '2026-10-01T00:00:00.000Z' });
    const monitor = new PhoneLinkHealthMonitor({
      phones: linkedPhoneSource(s),
      probe: async (p) => ({ deviceId: p.deviceId, platform: p.platform, whoami: null, call: 'failed', expiresAt: p.expiresAt }),
      now: () => Date.parse('2026-10-17T00:00:00.000Z'),
    });
    const h = await monitor.check('ios-dev-1');
    expect(h.state).toBe('SIGNATURE_RENEWAL_REQUIRED');
    expect(h.action).toContain('Plug your iPhone into Eoj’s MacBook Pro');
  });
});
