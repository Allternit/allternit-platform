import { describe, expect, it, vi } from 'vitest';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { IctrlError } from './ictrl-client.js';
import { fileLinkStore } from './phone-link-ios.js';
import { fileAndroidLinkStore } from './phone-link-android.js';
import { APPS_REFRESH_MS, cachedHostResolver, jpegSize, linkedPhoneSource, PhoneLinkBackend, probeLinkedPhone, rememberLinkedHost, LINK_SERIAL_PREFIX } from './phone-link-backend.js';
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
  // Real shapes (iCTRL 1.98.12 on iOS 26.6.2, Phone Link 0.1.0 on Android 14, 2026-10-17).
  it('asks the iPhone for a scale-1 image so image pixels are tap points', async () => {
    const client = fakeClient({
      screenshot: () => ({ data: jpeg(393, 852).toString('base64'), format: 'jpeg', pixel_width: 393, pixel_height: 852, point_width: 393, point_height: 852, scale: 1 }),
      tap: () => ({ message: 'ok' }),
    });
    const res = await backend(client).call('phone.screenshot', 'link:dev-1', {});
    expect(client.calls[0]).toEqual({ method: 'screenshot', params: { format: 'jpeg', scale: 1 } });
    expect(res).toMatchObject({ ok: true, mimeType: 'image/jpeg', width: 393, height: 852 });
    await backend(client).call('phone.tap', 'link:dev-1', { x: 200, y: 433 });
    expect(client.calls[1]).toEqual({ method: 'tap', params: { x: 200, y: 433 } });
  });

  it('Android screenshots are device pixels and taps pass through unchanged', async () => {
    const client = fakeClient({
      screenshot: () => ({ format: 'jpeg', encoding: 'base64', data: jpeg(720, 1600).toString('base64'), pixel_width: 720, pixel_height: 1600 }),
      tap: () => ({ message: 'ok' }),
    });
    const b = backend(client, [android]);
    expect(await b.call('phone.screenshot', 'link:dev-2', {})).toMatchObject({ ok: true, width: 720, height: 1600 });
    expect(client.calls[0].params).toEqual({ format: 'jpeg' });
    await b.call('phone.tap', 'link:dev-2', { x: 402, y: 1453 });
    expect(client.calls[1].params).toEqual({ x: 402, y: 1453 });
  });

  it('refuses a screenshot with no image data or not a JPEG', async () => {
    expect(await backend(fakeClient({ screenshot: () => ({ format: 'jpeg' }) })).call('phone.screenshot', 'link:dev-1', {})).toMatchObject({ ok: false, error: 'phone_failed' });
    expect(await backend(fakeClient({ screenshot: () => ({ data: Buffer.from('png?').toString('base64') }) })).call('phone.screenshot', 'link:dev-1', {})).toMatchObject({ ok: false, error: 'phone_failed' });
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

  it('direction swipes use the helper\'s swipe; endpoint swipes become a drag in seconds', async () => {
    const client = fakeClient({ swipe: () => ({ message: 'ok' }), drag: () => ({ message: 'ok' }) });
    const b = backend(client);
    expect(await b.call('phone.swipe', 'link:dev-1', { direction: 'up' })).toEqual({ ok: true });
    expect(client.calls[0]).toEqual({ method: 'swipe', params: { direction: 'up' } });
    expect(await b.call('phone.swipe', 'link:dev-1', { x1: 10, y1: 700, x2: 10, y2: 200, durationMs: 400 })).toEqual({ ok: true });
    expect(client.calls[1]).toEqual({ method: 'drag', params: { from_x: 10, from_y: 700, to_x: 10, to_y: 200, duration: 0.4 } });
    expect(await b.call('phone.swipe', 'link:dev-1', { direction: 'sideways' })).toMatchObject({ ok: false, error: 'invalid_args' });
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
      message: 'Pair this phone again in Settings › Subscriptions › Linked phones.',
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

  it('a failing call while unlocked → phone_failed with the phone\'s message', async () => {
    const b = backend(
      fakeClient({
        tap: () => { throw new IctrlError('rpc_error', 'screen unavailable'); },
        is_locked: () => ({ locked: false }),
      }),
    );
    expect(await b.call('phone.tap', 'link:dev-1', { x: 1, y: 1 })).toMatchObject({ ok: false, error: 'phone_failed', message: 'screen unavailable' });
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

describe('PhoneLinkBackend app refresh', () => {
  const healthy = (phones: LinkedPhone[], now: () => number) =>
    new PhoneLinkHealthMonitor({
      phones: () => phones,
      probe: async (p: LinkedPhone) => ({ deviceId: p.deviceId, platform: p.platform, whoami: { deviceId: p.deviceId }, call: 'ok' as const }),
      now,
    });
  const flush = () => new Promise((r) => setTimeout(r, 0));

  it('re-reads an Android phone’s apps in the background and replaces the saved list', async () => {
    let now = 1_000_000;
    const phone = { ...android, apps: [{ vendor: 'chatgpt', signedIn: null }] };
    const client = fakeClient({ list_apps: () => ({ apps: [{ package: 'com.openai.chatgpt' }, { package: 'org.telegram.messenger' }, { package: 'com.android.chrome' }] }) });
    const saveApps = vi.fn();
    const b = backend(client, [phone], { health: healthy([phone], () => now), saveApps, now: () => now });
    await b.linkedDevices();
    await flush();
    expect(saveApps).toHaveBeenCalledWith(phone, [
      { vendor: 'chatgpt', signedIn: null, name: 'ChatGPT', category: 'AI chat', provider: 'chatgpt' },
      { vendor: 'telegram', signedIn: null, name: 'Telegram', category: 'Communication' },
    ]);
    // At most every APPS_REFRESH_MS.
    await b.linkedDevices();
    await flush();
    expect(client.calls.filter((c) => c.method === 'list_apps')).toHaveLength(1);
    now += APPS_REFRESH_MS;
    await b.linkedDevices();
    await flush();
    expect(client.calls.filter((c) => c.method === 'list_apps')).toHaveLength(2);
  });

  it('on iPhone only adds apps: a partial or empty list never removes one', async () => {
    const phone = { ...iphone, apps: [{ vendor: 'claude', signedIn: null, name: 'Claude' }] };
    const saveApps = vi.fn();
    const client = fakeClient({ list_apps: () => ({ apps: [{ bundle_id: 'notion.id' }] }) });
    const b = backend(client, [phone], { health: healthy([phone], () => 1), saveApps, now: () => 1 });
    await b.linkedDevices();
    await flush();
    expect(saveApps.mock.calls[0][1].map((a: { vendor: string }) => a.vendor)).toEqual(['claude', 'notion']);

    const empty = vi.fn();
    const b2 = backend(fakeClient({ list_apps: () => ({ apps: [] }) }), [phone], { health: healthy([phone], () => 1), saveApps: empty, now: () => 1 });
    await b2.linkedDevices();
    await flush();
    expect(empty).not.toHaveBeenCalled();
  });

  it('leaves phones that are not healthy alone', async () => {
    const phone = { ...android };
    const monitor = new PhoneLinkHealthMonitor({
      phones: () => [phone],
      probe: async (p: LinkedPhone) => ({ deviceId: p.deviceId, platform: p.platform, whoami: null, call: 'failed' as const }),
      now: () => 1,
    });
    const client = fakeClient({ list_apps: () => ({ apps: [{ package: 'org.telegram.messenger' }] }) });
    const saveApps = vi.fn();
    await backend(client, [phone], { health: monitor, saveApps, now: () => 1 }).linkedDevices();
    await flush();
    expect(client.calls.some((c) => c.method === 'list_apps')).toBe(false);
    expect(saveApps).not.toHaveBeenCalled();
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
    let source = linkedPhoneSource({ iosStore, androidStore: fileAndroidLinkStore(path.join(dir, 'phone-link-android.json')) });
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
    source = linkedPhoneSource({ iosStore, androidStore: fileAndroidLinkStore(path.join(dir, 'phone-link-android.json')) });
    const phones = source();
    expect(phones).toHaveLength(2);
    expect(phones[1]).toEqual({
      deviceId: 'dev-2',
      platform: 'android',
      name: 'moto g play',
      lanHost: '192.168.1.21:8200',
      storeKey: 'phone-link.ictrl.android:dev-2',
      // A list saved by an older build (vendor id only) is named from the catalog.
      apps: [{ vendor: 'chatgpt', signedIn: null, name: 'ChatGPT', category: 'AI chat', provider: 'chatgpt' }],
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

describe('screen text shapes', () => {
  it('read_ui returns the tree from page_source.source; screen_text lists text with centres', async () => {
    const client = fakeClient({
      page_source: () => ({ format: 'text', source: "StaticText, value: 'Allternit iphone tree works', {{16, 229}, {361, 24}}", truncated: false }),
      // Android's screen_text also carries source: 'tree' — the items are the text.
      screen_text: () => ({ count: 2, source: 'tree', items: [{ label: 'Menu', center_x: 38, center_y: 81 }, { text: 'Log in', center_x: 622, center_y: 139 }] }),
    });
    const b = backend(client);
    expect(await b.call('phone.read_ui', 'link:dev-1', {})).toEqual({ ok: true, text: "StaticText, value: 'Allternit iphone tree works', {{16, 229}, {361, 24}}" });
    expect(await b.call('phone.screen_text', 'link:dev-1', {})).toEqual({ ok: true, text: 'Menu @ (38, 81)\nLog in @ (622, 139)' });
  });
});

describe('cachedHostResolver', () => {
  const discovery = (answers: Record<string, string>) => ({
    lanInterfaces: () => [{ address: '10.0.0.5', netmask: '255.255.255.0' }],
    fetchWhoami: async (host: string) => (answers[host] ? { device_id: answers[host] } : null),
  });

  it('keeps a saved address that still answers with the right device id', async () => {
    const remembered: string[] = [];
    const resolve = cachedHostResolver({ discovery: discovery({ '192.168.1.21:8200': 'dev-2' }), remember: (_p, h) => remembered.push(h) });
    expect(await resolve(android)).toBe('192.168.1.21:8200');
    expect(remembered).toEqual([]);
  });

  it('finds a moved phone by device id and writes the new address back', async () => {
    const remembered: string[] = [];
    const resolve = cachedHostResolver({ discovery: discovery({ '10.0.0.11:8200': 'dev-2' }), remember: (_p, h) => remembered.push(h) });
    expect(await resolve(android)).toBe('10.0.0.11:8200');
    expect(remembered).toEqual(['10.0.0.11:8200']);
  });

  it('caches answers for the ttl, then checks again', async () => {
    let t = 0;
    let probes = 0;
    const d = discovery({ '192.168.1.21:8200': 'dev-2' });
    const resolve = cachedHostResolver({
      discovery: { ...d, fetchWhoami: async (h: string) => (probes++, d.fetchWhoami(h)) },
      remember: () => {},
      ttlMs: 1000,
      now: () => t,
    });
    await resolve(android);
    await resolve(android);
    expect(probes).toBe(1);
    t = 2000;
    await resolve(android);
    expect(probes).toBe(2);
  });

  it('returns null when nothing on the LAN has that device id', async () => {
    const resolve = cachedHostResolver({ discovery: discovery({ '10.0.0.9:8200': 'someone-else' }), remember: () => {} });
    expect(await resolve(android)).toBeNull();
  });
});

describe('cachedHostResolver mesh heal', () => {
  const offLan = {
    lanInterfaces: () => [] as { address: string; netmask: string }[],
    fetchWhoami: async (host: string) => (host === '127.0.0.1:41000' && meshUp ? { device_id: 'dev-2' } : null),
  };
  let meshUp = false;

  it('restarts the phone mesh when its mesh route is dead, then uses the mesh', async () => {
    meshUp = false;
    const heals: string[] = [];
    const resolve = cachedHostResolver({
      discovery: offLan,
      remember: () => {},
      meshProxy: async () => '127.0.0.1:41000',
      healMesh: async (_p, why) => {
        heals.push(why);
        meshUp = true;
        return { meshState: 'running', meshHost: '100.64.0.16:8200' };
      },
    });
    expect(await resolve({ ...android, meshHost: '100.64.0.16:8200' })).toBe('127.0.0.1:41000');
    expect(resolve.lastPath?.('dev-2')).toBe('mesh');
    expect(heals).toEqual(['mesh route failed']);
    expect(resolve.healMesh).toBeDefined();
  });

  it('no heal, no mesh: null as before', async () => {
    meshUp = false;
    const resolve = cachedHostResolver({ discovery: offLan, remember: () => {}, meshProxy: async () => '127.0.0.1:41000', healMesh: async () => null });
    expect(await resolve({ ...android, meshHost: '100.64.0.16:8200' })).toBeNull();
  });
});

describe('helperCall mesh heal', () => {
  function meshResolver(heal: () => Promise<{ meshState: string; meshHost: string | null } | null>) {
    let forgotten = 0;
    const r = Object.assign(async () => '127.0.0.1:41000', {
      lastPath: () => 'mesh' as const,
      forget: () => void forgotten++,
      healMesh: heal,
    });
    return { r, forgotten: () => forgotten };
  }

  it('heals a dead mesh route once and retries the call', async () => {
    let fail = true;
    const client = fakeClient({
      is_locked: () => {
        if (fail) throw new IctrlError('unreachable', 'reset');
        return { locked: false };
      },
    });
    let heals = 0;
    const { r, forgotten } = meshResolver(async () => {
      heals++;
      fail = false;
      return { meshState: 'running', meshHost: '100.64.0.16:8200' };
    });
    const b = backend(client, [android], { resolveHost: r });
    expect(await b.helperCall('link:dev-2', 'is_locked')).toEqual({ locked: false });
    expect(heals).toBe(1);
    expect(forgotten()).toBe(1);
  });

  it('does not heal for app-level errors or LAN routes', async () => {
    let heals = 0;
    const heal = async () => (heals++, { meshState: 'running', meshHost: null });
    const locked = fakeClient({ tap: () => { throw new IctrlError('rpc_error', 'screen_locked'); } });
    await expect(backend(locked, [android], { resolveHost: meshResolver(heal).r }).helperCall('link:dev-2', 'tap')).rejects.toThrow(/screen_locked/);
    const dead = fakeClient({ tap: () => { throw new IctrlError('unreachable', 'reset'); } });
    const lan = Object.assign(async () => '192.168.1.21:8200', { lastPath: () => 'lan' as const, healMesh: heal });
    await expect(backend(dead, [android], { resolveHost: lan }).helperCall('link:dev-2', 'tap')).rejects.toThrow(/reset/);
    expect(heals).toBe(0);
  });

  it('tool calls heal too', async () => {
    let fail = true;
    const client = fakeClient({
      page_source: () => {
        if (fail) throw new IctrlError('unreachable', 'reset');
        return { format: 'text', source: 'Button, label: "OK"', truncated: false };
      },
    });
    const { r } = meshResolver(async () => ((fail = false), { meshState: 'running', meshHost: null }));
    const res = await backend(client, [android], { resolveHost: r }).call('phone.read_ui', 'link:dev-2', {});
    expect(res).toEqual({ ok: true, text: 'Button, label: "OK"' });
  });
});

describe('rememberLinkedHost', () => {
  it('updates the right store for each platform', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pl-remember-'));
    const iosStore = fileLinkStore(path.join(dir, 'ios.json'));
    const androidStore = fileAndroidLinkStore(path.join(dir, 'android.json'));
    iosStore.save({ udid: 'udid-1', deviceId: 'dev-1', lanHost: '10.0.0.90:8200', runnerBundleId: 'r', hostBundleId: 'h', xctestConfig: 'x', testToRun: 't' } as never);
    androidStore.save({ deviceId: 'dev-2', serial: 'ZY', lanHost: '10.0.0.238:8200', pairedAt: '2026-10-17T00:00:00.000Z' });
    const remember = rememberLinkedHost({ iosStore, androidStore });
    remember(iphone, '10.0.0.252:8200');
    remember(android, '10.0.0.11:8200');
    expect(iosStore.load('udid-1')?.lanHost).toBe('10.0.0.252:8200');
    expect(androidStore.load('dev-2')?.lanHost).toBe('10.0.0.11:8200');
  });
});

describe('cachedHostResolver shared scan', () => {
  it('two moved phones are found by one LAN scan', async () => {
    let probes = 0;
    const answers: Record<string, string> = { '10.0.0.11:8200': 'dev-2', '10.0.0.252:8200': 'dev-1' };
    const resolve = cachedHostResolver({
      discovery: {
        lanInterfaces: () => [{ address: '10.0.0.5', netmask: '255.255.255.0' }],
        fetchWhoami: async (h: string) => (probes++, answers[h] ? { device_id: answers[h] } : null),
      },
      remember: () => {},
    });
    const [a, b] = await Promise.all([resolve({ ...iphone, lanHost: '10.0.0.90:8200' }), resolve({ ...android, lanHost: '10.0.0.238:8200' })]);
    expect([a, b]).toEqual(['10.0.0.252:8200', '10.0.0.11:8200']);
    // 2 stale saved hosts + one pass over the /24 (253 hosts, own address skipped).
    expect(probes).toBe(2 + 253);
  });
});

describe('PhoneLinkBackend live-screen lookups', () => {
  it('finds the linked phone and its current address by serial', async () => {
    const b = backend(fakeClient({}), [iphone, android], { resolveHost: Object.assign(async (p: LinkedPhone) => `mesh-${p.deviceId}:8200`, {}) });
    expect(b.linkedPhone('link:dev-1')?.platform).toBe('ios');
    expect(b.linkedPhone('link:nope')).toBeNull();
    expect(await b.hostFor('link:dev-2')).toBe('mesh-dev-2:8200');
    expect(await b.hostFor('emulator-5554')).toBeNull();
  });
});
