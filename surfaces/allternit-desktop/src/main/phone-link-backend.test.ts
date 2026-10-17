import { describe, expect, it, vi } from 'vitest';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { IctrlError } from './ictrl-client.js';
import { fileLinkStore } from './phone-link-ios.js';
import { jpegSize, linkedPhoneSource, PhoneLinkBackend, probeLinkedPhone, LINK_SERIAL_PREFIX } from './phone-link-backend.js';
import { PhoneLinkHealthMonitor, type LinkedPhone } from './phone-link-health.js';
import type { IctrlLike } from './phone-link-ios.js';

/** Minimal but well-formed JPEG with the given pixel size (SOI + SOF0 + EOI). */
function jpeg(width: number, height: number): Buffer {
  const buf = Buffer.alloc(23);
  buf[0] = 0xff;
  buf[1] = 0xd8;
  buf[2] = 0xff;
  buf[3] = 0xc0;
  buf.writeUInt16BE(17, 4); // SOF length: 8 + 3 components × 3
  buf[6] = 8; // precision
  buf.writeUInt16BE(height, 7);
  buf.writeUInt16BE(width, 9);
  buf[11] = 3;
  buf[21] = 0xff;
  buf[22] = 0xd9;
  return buf;
}

type MethodHandler = (params: Record<string, unknown>) => unknown;

interface FakeClient extends IctrlLike {
  calls: { method: string; params: Record<string, unknown> }[];
}

function fakeClient(handlers: Record<string, MethodHandler>, whoami: { device_id: string } | Error | null = { device_id: 'dev-1' }): FakeClient {
  const calls: FakeClient['calls'] = [];
  const client: FakeClient = {
    calls,
    host: '192.168.1.20:8200',
    hasKey: () => true,
    forget: () => {},
    whoami: async () => {
      if (whoami instanceof Error) throw whoami;
      return whoami;
    },
    pair: async () => ({}),
    withHost: () => client,
    call: async <T = Record<string, unknown>>(method: string, params: Record<string, unknown> = {}): Promise<T> => {
      calls.push({ method, params });
      const handler = handlers[method];
      if (!handler) throw new IctrlError('rpc_error', `no handler for ${method}`);
      return handler(params) as T;
    },
  };
  return client;
}

const iphone: LinkedPhone = { deviceId: 'dev-1', platform: 'ios', name: 'iPhone 15', lanHost: '192.168.1.20:8200', storeKey: 'k1' };
const android: LinkedPhone = { deviceId: 'dev-2', platform: 'android', name: 'moto g play', lanHost: '192.168.1.21:8200', storeKey: 'k2' };

function backend(client: IctrlLike, phones: LinkedPhone[] = [iphone], extra: Partial<ConstructorParameters<typeof PhoneLinkBackend>[0]> = {}) {
  return new PhoneLinkBackend({ phones: () => phones, client: () => client, ...extra });
}

describe('jpegSize', () => {
  it('reads width and height from the SOF marker', () => {
    expect(jpegSize(jpeg(1179, 2556))).toEqual({ width: 1179, height: 2556 });
    expect(jpegSize(jpeg(1080, 2400))).toEqual({ width: 1080, height: 2400 });
  });

  it('refuses non-JPEG bytes', () => {
    expect(jpegSize(Buffer.from('not a jpeg at all'))).toBeNull();
    expect(jpegSize(Buffer.from([0x89, 0x50]))).toBeNull();
  });
});

describe('PhoneLinkBackend lane mapping', () => {
  it('iPhone screenshot: returns JPEG pixels, learns the point scale, reports the tap space', async () => {
    const client = fakeClient({
      screenshot: () => ({ image: jpeg(1179, 2556).toString('base64'), width: 393, height: 852, coordinate_space: 'points' }),
    });
    const b = backend(client);
    const res = await b.call('phone.screenshot', 'link:dev-1', {});
    expect(res).toMatchObject({ ok: true, mimeType: 'image/jpeg', width: 1179, height: 2556, coordinate_space: 'pixels' });
    expect(client.calls[0]).toEqual({ method: 'screenshot', params: { format: 'jpeg' } });
    // The image comes back verbatim.
    expect((res as unknown as { dataBase64: string }).dataBase64).toBe(jpeg(1179, 2556).toString('base64'));
  });

  it('scales iPhone taps from image pixels to points', async () => {
    const client = fakeClient({
      screenshot: () => ({ image: jpeg(1179, 2556).toString('base64'), width: 393, height: 852, coordinate_space: 'points' }),
      tap: () => ({}),
      swipe: () => ({}),
    });
    const b = backend(client);
    await b.call('phone.screenshot', 'link:dev-1', {});
    expect(await b.call('phone.tap', 'link:dev-1', { x: 600, y: 1200 })).toEqual({ ok: true });
    expect(client.calls.at(-1)).toEqual({ method: 'tap', params: { x: 200, y: 400 } });

    expect(await b.call('phone.swipe', 'link:dev-1', { x1: 0, y1: 2556, x2: 1179, y2: 0, durationMs: 50 })).toEqual({ ok: true });
    expect(client.calls.at(-1)).toEqual({ method: 'swipe', params: { x1: 0, y1: 852, x2: 393, y2: 0, duration_ms: 50 } });
  });

  it('Android screenshots are pixels 1:1 and taps pass through', async () => {
    const client = fakeClient({
      screenshot: () => ({ image: jpeg(1080, 2400).toString('base64'), width: 1080, height: 2400, coordinate_space: 'pixels' }),
      tap: () => ({}),
    });
    const b = backend(client, [android]);
    expect(await b.call('phone.screenshot', 'link:dev-2', {})).toMatchObject({ ok: true, width: 1080, height: 2400, coordinate_space: 'pixels' });
    expect(await b.call('phone.tap', 'link:dev-2', { x: 540, y: 1200 })).toEqual({ ok: true });
    expect(client.calls.at(-1)).toEqual({ method: 'tap', params: { x: 540, y: 1200 } });
  });

  it('validates tap coordinates and typed text before calling the phone', async () => {
    const client = fakeClient({ tap: () => ({}), type: () => ({}) });
    const b = backend(client);
    expect(await b.call('phone.tap', 'link:dev-1', { x: -1, y: 5 })).toMatchObject({ ok: false, error: 'invalid_args' });
    expect(await b.call('phone.tap', 'link:dev-1', { x: 1.5, y: 5 })).toMatchObject({ error: 'invalid_args' });
    expect(await b.call('phone.type', 'link:dev-1', { text: '' })).toMatchObject({ error: 'invalid_args' });
    expect(await b.call('phone.type', 'link:dev-1', { text: 'x'.repeat(501) })).toMatchObject({ error: 'invalid_args' });
    expect(client.calls).toHaveLength(0);
    expect(await b.call('phone.type', 'link:dev-1', { text: 'héllo emoji 🎉' })).toEqual({ ok: true });
    expect(client.calls.at(-1)).toEqual({ method: 'type', params: { text: 'héllo emoji 🎉' } });
  });

  it('home maps to home; back is Android-only with a plain sentence on iOS', async () => {
    const client = fakeClient({ home: () => ({}), back: () => ({}) });
    const b = backend(client);
    expect(await b.call('phone.home', 'link:dev-1', {})).toEqual({ ok: true });
    expect(client.calls.at(-1)?.method).toBe('home');
    expect(await b.call('phone.back', 'link:dev-1', {})).toEqual({
      ok: false,
      error: 'unsupported',
      message: "iPhone has no Back button. Tap the app's back control instead.",
    });
    const droid = backend(client, [android]);
    expect(await droid.call('phone.back', 'link:dev-2', {})).toEqual({ ok: true });
    expect(client.calls.at(-1)?.method).toBe('back');
  });

  it('open_app resolves vendor names per platform and package ids verbatim', async () => {
    const client = fakeClient({ launch_app: () => ({}) });
    const b = backend(client);
    expect(await b.call('phone.open_app', 'link:dev-1', { app: 'chatgpt' })).toMatchObject({ ok: true, app: 'com.openai.chat' });
    expect(client.calls.at(-1)).toEqual({ method: 'launch_app', params: { bundle_id: 'com.openai.chat' } });
    expect(await b.call('phone.open_app', 'link:dev-1', { app: 'com.example.custom' })).toMatchObject({ ok: true, app: 'com.example.custom' });
    expect(await b.call('phone.open_app', 'link:dev-1', { app: 'not-an-app' })).toMatchObject({ ok: false, error: 'unknown_app' });

    const droid = backend(client, [android]);
    expect(await droid.call('phone.open_app', 'link:dev-2', { app: 'ChatGPT' })).toMatchObject({ ok: true, app: 'com.openai.chatgpt' });
    expect(await droid.call('phone.open_app', 'link:dev-2', { app: 'settings' })).toMatchObject({ ok: true, app: 'com.android.settings' });
  });

  it('read_ui / screen_text / find_and_tap map to the helper methods', async () => {
    const client = fakeClient({
      page_source: () => ({ text: 'UIView "Sign in" {{24, 836}, {312, 48}}' }),
      screen_text: () => ({ text: 'Sign in\nEmail' }),
      find_and_tap: () => ({}),
    });
    const b = backend(client);
    expect(await b.call('phone.read_ui', 'link:dev-1', { filter: { label: 'sign' }, visible_only: true, max_elements: 20 })).toEqual({
      ok: true,
      text: 'UIView "Sign in" {{24, 836}, {312, 48}}',
    });
    expect(client.calls.at(-1)).toEqual({
      method: 'page_source',
      params: { format: 'text', filter: { label: 'sign' }, visible_only: true, max_elements: 20 },
    });
    expect(await b.call('phone.screen_text', 'link:dev-1', {})).toEqual({ ok: true, text: 'Sign in\nEmail' });
    expect(client.calls.at(-1)?.method).toBe('screen_text');

    expect(await b.call('phone.find_and_tap', 'link:dev-1', { element_id: 'Sign in' })).toEqual({ ok: true });
    expect(client.calls.at(-1)).toEqual({ method: 'find_and_tap', params: { element_id: 'Sign in' } });
    expect(await b.call('phone.find_and_tap', 'link:dev-1', { element_id: 'Sign in', match: 'continue' })).toEqual({ ok: true });
    expect(client.calls.at(-1)).toEqual({ method: 'find_and_tap', params: { element_id: 'Sign in', match: 'continue' } });

    expect(await b.call('phone.read_ui', 'link:dev-1', { filter: 'nope' })).toMatchObject({ error: 'invalid_args' });
    expect(await b.call('phone.find_and_tap', 'link:dev-1', {})).toMatchObject({ error: 'invalid_args' });
  });

  it('sms, calls, keys and plain-English tasks stay unsupported with a plain sentence', async () => {
    const client = fakeClient({});
    const b = backend(client);
    for (const [name, args, hint] of [
      ['phone.key', { combo: 'enter' }, 'hardware keys'],
      ['phone.task', { task: 'open settings' }, 'Plain-English tasks'],
      ['phone.sms.send', { to: '+15550102030', text: 'hi' }, 'Texts'],
      ['phone.call.dial', { number: '+15550102030' }, 'Calls'],
    ] as const) {
      const res = await b.call(name, 'link:dev-1', args);
      expect(res).toMatchObject({ ok: false, error: 'unsupported' });
      expect(String((res as { message: string }).message)).toContain(hint);
    }
    expect(client.calls).toHaveLength(0);
  });

  it('direction swipe needs a screenshot first, then uses the screen centre', async () => {
    const client = fakeClient({
      screenshot: () => ({ image: jpeg(1179, 2556).toString('base64'), width: 393, height: 852, coordinate_space: 'points' }),
      swipe: () => ({}),
    });
    const b = backend(client);
    expect(await b.call('phone.swipe', 'link:dev-1', { direction: 'up' })).toMatchObject({ ok: false, error: 'adb_failed' });
    await b.call('phone.screenshot', 'link:dev-1', {});
    expect(await b.call('phone.swipe', 'link:dev-1', { direction: 'up' })).toEqual({ ok: true });
    // centre (589, 1278), dy = 852 in image pixels; ÷3 to points.
    expect(client.calls.at(-1)).toEqual({ method: 'swipe', params: { x1: 196, y1: 710, x2: 196, y2: 142, duration_ms: 300 } });
    expect(await b.call('phone.swipe', 'link:dev-1', { direction: 'sideways' })).toMatchObject({ error: 'invalid_args' });
  });
});

describe('PhoneLinkBackend error mapping', () => {
  it('unreachable → device_offline with the address', async () => {
    const b = backend(fakeClient({ tap: () => { throw new IctrlError('unreachable', 'fetch failed'); } }));
    expect(await b.call('phone.tap', 'link:dev-1', { x: 1, y: 1 })).toMatchObject({
      ok: false,
      error: 'device_offline',
      message: expect.stringContaining('192.168.1.20:8200'),
    });
  });

  it('needsRepair → pairing_invalid', async () => {
    const b = backend(fakeClient({ tap: () => { throw new IctrlError('auth_failed', 'key revoked'); } }));
    expect(await b.call('phone.tap', 'link:dev-1', { x: 1, y: 1 })).toEqual({
      ok: false,
      error: 'pairing_invalid',
      message: 'Pair this phone again in Settings › Computers › Phones.',
    });
  });

  it('a failing call with is_locked true → device_locked', async () => {
    const b = backend(
      fakeClient({
        tap: () => { throw new IctrlError('rpc_error', 'screen unavailable'); },
        is_locked: () => ({ locked: true }),
      }),
    );
    expect(await b.call('phone.tap', 'link:dev-1', { x: 1, y: 1 })).toEqual({
      ok: false,
      error: 'device_locked',
      message: 'Unlock your phone and try again.',
    });
  });

  it('a failing call while unlocked → adb_failed with the phone\'s message', async () => {
    const b = backend(
      fakeClient({
        tap: () => { throw new IctrlError('rpc_error', 'screen unavailable'); },
        is_locked: () => ({ locked: false }),
      }),
    );
    expect(await b.call('phone.tap', 'link:dev-1', { x: 1, y: 1 })).toMatchObject({ ok: false, error: 'adb_failed', message: 'screen unavailable' });
  });

  it('unknown serial and missing host are device_offline, no client touched', async () => {
    const client = fakeClient({});
    const b = backend(client);
    expect(await b.call('phone.tap', 'link:nope', { x: 1, y: 1 })).toMatchObject({ error: 'device_offline' });
    const noHost = backend(client, [{ ...iphone, lanHost: undefined }]);
    expect(await noHost.call('phone.tap', 'link:dev-1', { x: 1, y: 1 })).toMatchObject({ ok: false, error: 'device_offline' });
    expect(client.calls).toHaveLength(0);
  });
});

describe('PhoneLinkBackend phones and health', () => {
  it('serials() treats HEALTHY and DEVICE_LOCKED as online, TRANSPORT_OFFLINE as not', async () => {
    const phones = [iphone, android];
    const probe = vi.fn(async (p: LinkedPhone) =>
      p.deviceId === 'dev-1'
        ? { deviceId: p.deviceId, platform: p.platform, whoami: { deviceId: p.deviceId }, call: 'ok' as const }
        : { deviceId: p.deviceId, platform: p.platform, whoami: null, call: 'failed' as const },
    );
    const monitor = new PhoneLinkHealthMonitor({ phones: () => phones, probe, now: () => 1_000_000 });
    const client = fakeClient({});
    const b = backend(client, phones, { health: monitor });

    // Not checked yet: optimistically online.
    expect(b.serials()).toEqual(['link:dev-1', 'link:dev-2']);
    await monitor.refreshStale();
    expect(b.serials()).toEqual(['link:dev-1']);
    expect(b.has('link:dev-1')).toBe(true);
    expect(b.has('link:dev-2')).toBe(false);
  });

  it('linkedDevices() refreshes stale health and returns the phone.devices shape', async () => {
    const probe = vi.fn(async (p: LinkedPhone) => ({ deviceId: p.deviceId, platform: p.platform, whoami: { deviceId: p.deviceId }, call: 'ok' as const }));
    const monitor = new PhoneLinkHealthMonitor({ phones: () => [android], probe, now: () => 1_000_000 });
    const b = backend(fakeClient({}), [android], { health: monitor });
    const devices = await b.linkedDevices();
    expect(probe).toHaveBeenCalledTimes(1);
    expect(devices).toEqual([
      {
        serial: 'link:dev-2',
        deviceId: 'dev-2',
        platform: 'android',
        via: 'phone-link',
        route: 'android-app',
        name: 'moto g play',
        health: { state: 'HEALTHY', checkedAt: expect.any(String), recovery: 'automatic' },
        apps: undefined,
      },
    ]);
  });

  it('linkedDevices() works without a health monitor', async () => {
    const b = backend(fakeClient({}), [iphone]);
    const devices = await b.linkedDevices();
    expect(devices[0]).toMatchObject({ serial: 'link:dev-1', via: 'phone-link', route: 'iphone-app-ictrl', health: { state: 'UNKNOWN' } });
  });
});

describe('linkedPhoneSource', () => {
  it('reads both stores; a missing Android file means no Android phones', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'link-src-'));
    const iosStore = fileLinkStore(path.join(dir, 'phone-link-ios.json'));
    iosStore.save({
      udid: 'udid-1',
      deviceId: 'dev-ios',
      deviceName: 'iPhone 15',
      lanHost: '192.168.1.20:8200',
      expiresAt: '2026-10-20T00:00:00.000Z',
      runnerBundleId: 'com.allternit.phonelink.ictrl',
      hostBundleId: 'com.allternit.phonelink',
      xctestConfig: 'iCTRLUITests.xctest',
      testToRun: 'testStartServer',
      linkedAt: '2026-10-17T00:00:00.000Z',
    });
    // No phone-link-android.json yet.
    let source = linkedPhoneSource({ iosStore, androidFile: path.join(dir, 'phone-link-android.json') });
    expect(source()).toEqual([
      {
        deviceId: 'dev-ios',
        platform: 'ios',
        name: 'iPhone 15',
        lanHost: '192.168.1.20:8200',
        storeKey: 'phone-link.ictrl.udid-1',
        expiresAt: '2026-10-20T00:00:00.000Z',
        apps: undefined,
      },
    ]);

    fs.writeFileSync(
      path.join(dir, 'phone-link-android.json'),
      JSON.stringify({
        'dev-2': {
          deviceId: 'dev-2',
          serial: 'ZY326',
          deviceName: 'moto g play',
          osVersion: 'Android 14',
          lanHost: '192.168.1.21:8200',
          apps: [{ vendor: 'chatgpt', signedIn: null }],
          pairedAt: '2026-10-17T00:00:00.000Z',
          helperVersion: '1.0.0',
        },
      }),
    );
    source = linkedPhoneSource({ iosStore, androidFile: path.join(dir, 'phone-link-android.json') });
    const phones = source();
    expect(phones).toHaveLength(2);
    expect(phones[1]).toEqual({
      deviceId: 'dev-2',
      platform: 'android',
      name: 'moto g play',
      lanHost: '192.168.1.21:8200',
      storeKey: 'phone-link.ictrl.android:dev-2',
      apps: [{ vendor: 'chatgpt', signedIn: null }],
    });
    fs.rmSync(dir, { recursive: true, force: true });
  });
});

describe('probeLinkedPhone', () => {
  it('needs a matching device_id from /whoami', async () => {
    const ok = fakeClient({ device_info: () => ({}) }, { device_id: 'dev-1' });
    expect(await probeLinkedPhone(iphone, () => ok)).toMatchObject({ whoami: { deviceId: 'dev-1' }, call: 'ok' });

    const other = fakeClient({}, { device_id: 'someone-else' });
    expect(await probeLinkedPhone(iphone, () => other)).toMatchObject({ whoami: null, call: 'failed' });

    const silent = fakeClient({}, new IctrlError('unreachable', 'no answer'));
    expect(await probeLinkedPhone(iphone, () => silent)).toMatchObject({ whoami: null, call: 'failed' });
  });

  it('distinguishes locked, healthy, and needs-repair from the sealed calls', async () => {
    const locked = fakeClient({ device_info: () => ({}), is_locked: () => ({ locked: true }) });
    expect(await probeLinkedPhone(iphone, () => locked)).toMatchObject({ call: 'locked' });

    const broken = fakeClient({ device_info: () => { throw new IctrlError('no_key', 'gone'); } });
    expect(await probeLinkedPhone(iphone, () => broken)).toMatchObject({ call: 'needs_repair' });

    const deadHelper = fakeClient({ device_info: () => { throw new IctrlError('rpc_error', 'busy'); } });
    expect(await probeLinkedPhone(iphone, () => deadHelper)).toMatchObject({ call: 'failed' });
  });

  it('uses the injected host resolver', async () => {
    const client = fakeClient({ device_info: () => ({}) });
    expect(await probeLinkedPhone({ ...iphone, lanHost: undefined }, () => client, async () => null)).toMatchObject({ whoami: null });
    const hostUsed: string[] = [];
    await probeLinkedPhone(iphone, (_p, host) => (hostUsed.push(host), client));
    expect(hostUsed).toEqual(['192.168.1.20:8200']);
  });
});

describe('serial format', () => {
  it('link:<deviceId>', () => {
    expect(LINK_SERIAL_PREFIX + 'dev-1').toBe('link:dev-1');
  });
});
