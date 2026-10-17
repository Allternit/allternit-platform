import { describe, expect, it, vi } from 'vitest';

// This file covers the flow with the mesh switch OFF (phone-link-mesh.test.ts
// covers it ON). The switch is read when the module loads, so set it first.
vi.hoisted(() => {
  process.env.ALLTERNIT_PHONE_LINK_MESH = '0';
});
import { PhoneLinkManager } from './phone-link.js';
import type { PhoneLinkStatus } from './phone-link-types.js';
import type { AdbCall, AdbResult } from './phone-tools.js';
import { IctrlError, ictrlStoreKey } from './ictrl-client.js';
import {
  accessibilityBound,
  parsePackageVersionCode,
  parseProvisionDeviceId,
  reconnectAndroid,
  type AndroidDriverDeps,
  type AndroidLinkRecord,
} from './phone-link-android.js';
import { AndroidHelperUnavailableError, type AndroidHelper } from './phone-android-helper.js';

const instant = () => Promise.resolve();
const PKG = 'com.allternit.phonelink';
const SERVICE = `${PKG}/.PhoneLinkAccessibilityService`;
const DEVICE_ID = 'dev-android-1';

interface World {
  phonePresent: boolean;
  unauthorizedFirst: boolean;
  installedVersionCode: number | null;
  installShouldFail: boolean;
  existingAccessibility: string;
  accessibilityBinds: boolean;
  provisionOutput: string;
  whoamiId: string | null;
  wifiIp: string | null;
  lanHost: string | null;
  wifiDeviceId: string | null;
  listApps: string[];
  helper: AndroidHelper | Error;
  ran: string[];
  accessibilityPut: string[];
  forwardsRemoved: string[];
  saved: AndroidLinkRecord[];
  paired: { storeKey: string; clientName: string; meta?: { deviceId?: string; deviceName?: string } }[];
  calls: { storeKey: string; host: string; method: string }[];
  storeKeys: string[];
}

function world(over: Partial<World> = {}): World {
  return {
    phonePresent: true,
    unauthorizedFirst: false,
    installedVersionCode: null,
    installShouldFail: false,
    existingAccessibility: 'null',
    accessibilityBinds: true,
    provisionOutput: `Broadcast completed: result=0, data="provisioned device_id=${DEVICE_ID} name='Pixel 8' remote=true port=8200 armed=true"`,
    whoamiId: DEVICE_ID,
    wifiIp: '10.0.0.7',
    lanHost: '10.0.0.7:8200',
    wifiDeviceId: DEVICE_ID,
    listApps: ['com.openai.chatgpt', 'com.android.chrome'],
    helper: { apk: '/helpers/phone-link.apk', manifest: { version: '1.0.0', versionCode: 7, apk: { file: 'p.apk', sha256: 'a'.repeat(64), size: 1 } } },
    ran: [],
    accessibilityPut: [],
    forwardsRemoved: [],
    saved: [],
    paired: [],
    calls: [],
    storeKeys: [],
    ...over,
  };
}

function fakeAdb(w: World): AdbCall {
  const call = (async (args: string[]): Promise<AdbResult> => {
    const cmd = args.join(' ');
    const inner = cmd.startsWith('shell ') ? cmd.slice('shell '.length) : cmd;
    w.ran.push(cmd);
    const out = (stdout: string, code = 0): AdbResult => ({ stdout, stderr: '', code });
    if (cmd === 'devices -l') {
      if (!w.phonePresent) return out('List of devices attached\n');
      if (w.unauthorizedFirst) {
        w.unauthorizedFirst = false;
        return out('List of devices attached\nR5CT1 unauthorized usb:1\n');
      }
      return out('List of devices attached\nR5CT1 device usb:1 model:Pixel_8\n');
    }
    if (inner.includes('ro.product.model')) return out('Pixel 8\n');
    if (inner.includes('ro.build.version.release')) return out('15\n');
    if (inner.startsWith('dumpsys package')) {
      return out(w.installedVersionCode === null ? '' : `Packages:\n  Package [${PKG}] (abc):\n    versionCode=${w.installedVersionCode} minSdk=26\n`);
    }
    if (cmd.startsWith('install -r -g')) return w.installShouldFail ? out('Failure [INSTALL_FAILED_USER_RESTRICTED]', 1) : out('Success\n');
    if (inner === 'settings get secure enabled_accessibility_services') return out(`${w.existingAccessibility}\n`);
    if (inner.startsWith('settings put secure enabled_accessibility_services')) {
      w.accessibilityPut.push(inner.slice('settings put secure enabled_accessibility_services '.length));
      return out('');
    }
    if (inner.startsWith('am broadcast')) return out(w.provisionOutput);
    if (inner.startsWith('dumpsys accessibility')) return out(
        w.accessibilityBinds
          ? // Real Android 14 output (moto g play, 2026-10-17): the full class name, bound by label.
            '     Bound services:{Service[label=Allternit Phone Link, feedbackType[FEEDBACK_GENERIC], capabilities=161]}\n     Enabled services:{{com.allternit.phonelink/com.allternit.phonelink.PhoneLinkAccessibilityService}}\n'
          : '     Bound services:{}\n     Enabled services:{{com.allternit.phonelink/com.allternit.phonelink.PhoneLinkAccessibilityService}}\n',
      );
    if (cmd === 'forward tcp:0 tcp:8200') return out('52123\n');
    if (cmd.startsWith('forward --remove')) {
      w.forwardsRemoved.push(cmd.slice('forward --remove tcp:'.length));
      return out('');
    }
    if (inner.includes('wlan0')) return out(w.wifiIp ? `inet ${w.wifiIp}/24 brd 10.0.0.255 scope global wlan0` : '');
    return out('');
  }) as AdbCall;
  call.binary = async () => Buffer.alloc(0);
  return call;
}

function fakeIctrl(w: World): AndroidDriverDeps['ictrl'] {
  const make = (storeKey: string, host: string) => ({
    host,
    hasKey: () => true,
    forget: () => {},
    whoami: async () => (w.whoamiId && host.startsWith('127.0.0.1') ? { device_id: w.whoamiId } : null),
    pair: async (clientName: string, meta?: { deviceId?: string; deviceName?: string }) => {
      w.paired.push({ storeKey, clientName, meta });
    },
    call: async <T,>(method: string): Promise<T> => {
      w.calls.push({ storeKey, host, method });
      if (method === 'device_info') {
        if (w.wifiDeviceId === null || host !== w.lanHost) throw new IctrlError('unreachable', 'unreachable');
        return { device_id: w.wifiDeviceId } as T;
      }
      if (method === 'list_apps') return { count: w.listApps.length, apps: w.listApps.map((pkg) => ({ bundle_id: pkg, package: pkg, name: pkg, system: false })) } as T;
      return {} as T;
    },
    withHost: (h: string) => make(storeKey, h),
  });
  return (storeKey, host) => {
    w.storeKeys.push(storeKey);
    return make(storeKey, host);
  };
}

function deps(w: World, extra: Partial<AndroidDriverDeps> = {}): AndroidDriverDeps {
  const records = new Map<string, AndroidLinkRecord>();
  return {
    adb: async () => fakeAdb(w),
    helper: async () => {
      if (w.helper instanceof Error) throw w.helper;
      return w.helper;
    },
    dataDir: '/data/phone',
    ictrl: fakeIctrl(w),
    store: {
      load: (id) => records.get(id) ?? null,
      all: () => [...records.values()],
      save: (r) => {
        records.set(r.deviceId, r);
        w.saved.push(r);
      },
    },
    clientName: 'Allternit Desktop (test)',
    ...extra,
  };
}

function waitFor(manager: PhoneLinkManager, state: PhoneLinkStatus['state']): Promise<PhoneLinkStatus> {
  return new Promise((resolve) => {
    if (manager.status().state === state) return resolve(manager.status());
    const on = (s: PhoneLinkStatus) => {
      if (s.state !== state) return;
      manager.off('status', on);
      resolve(s);
    };
    manager.on('status', on);
  });
}

function start(w: World, extra?: Partial<AndroidDriverDeps>): PhoneLinkManager {
  const manager = new PhoneLinkManager({ sleep: instant, android: async () => deps(w, extra) });
  manager.start('android');
  return manager;
}

describe('parsers', () => {
  it('reads the installed versionCode', () => {
    expect(parsePackageVersionCode(`Packages:\n  Package [${PKG}]:\n    versionCode=42 minSdk=26`)).toBe(42);
    expect(parsePackageVersionCode('')).toBeNull();
  });

  it('parses the PROVISION broadcast data', () => {
    expect(parseProvisionDeviceId(`Broadcast completed: result=0, data="provisioned device_id=${DEVICE_ID} name='Pixel 8' remote=true port=8200 armed=true"`)).toBe(DEVICE_ID);
    expect(parseProvisionDeviceId('Broadcast completed: result=0')).toBeNull();
  });
});

describe('Android Phone Link', () => {
  it('walks USB debugging → install → provision → pair → Wi-Fi', async () => {
    const w = world({ unauthorizedFirst: true });
    const manager = start(w);
    const seen: string[] = [];
    manager.on('status', (s: PhoneLinkStatus) => seen[seen.length - 1] !== s.state && seen.push(s.state));
    const ready = await waitFor(manager, 'ready_wifi');

    expect(seen).toEqual(['usb_debugging', 'installing', 'pairing', 'ready_wifi']);
    expect(ready).toMatchObject({
      platform: 'android',
      deviceName: 'Pixel 8',
      osVersion: 'Android 15',
      detail: '10.0.0.7:8200',
      apps: [{ vendor: 'chatgpt', signedIn: null }],
    });
    // The phone had no app at all → installed; provisioned with the service appended to the empty list.
    expect(w.ran.some((c) => c.startsWith('install -r -g /helpers/phone-link.apk'))).toBe(true);
    expect(w.accessibilityPut).toEqual([SERVICE]);
    // The provisioning broadcast carries the device name quoted for the phone shell.
    const broadcast = w.ran.find((c) => c.includes('am broadcast'));
    expect(broadcast).toContain(`-a ${PKG}.PROVISION`);
    expect(broadcast).toContain(`--es device_name 'Pixel 8'`);
    // Paired under the android:<device id> key, then verified and listed over Wi-Fi with that key.
    expect(w.storeKeys[0]).toBe(ictrlStoreKey(`android:${DEVICE_ID}`));
    expect(w.paired).toEqual([{ storeKey: ictrlStoreKey(`android:${DEVICE_ID}`), clientName: 'Allternit Desktop (test)', meta: { deviceId: DEVICE_ID, deviceName: 'Pixel 8' } }]);
    expect(w.calls.filter((c) => c.host === '10.0.0.7:8200').map((c) => c.method)).toEqual(['device_info', 'list_apps']);
    // The cable forward is torn down again.
    expect(w.forwardsRemoved).toEqual(['52123']);
    expect(w.saved.at(-1)).toMatchObject({
      deviceId: DEVICE_ID,
      serial: 'R5CT1',
      deviceName: 'Pixel 8',
      osVersion: 'Android 15',
      lanHost: '10.0.0.7:8200',
      helperVersion: '1.0.0',
      apps: [{ vendor: 'chatgpt', signedIn: null }],
    });
  });

  it('skips the install when the phone already has this version, upgrades when older', async () => {
    const same = world({ installedVersionCode: 7 });
    await waitFor(start(same), 'ready_wifi');
    expect(same.ran.some((c) => c.startsWith('install -r -g'))).toBe(false);

    const newer = world({ installedVersionCode: 9 });
    await waitFor(start(newer), 'ready_wifi');
    expect(newer.ran.some((c) => c.startsWith('install -r -g'))).toBe(false);

    const older = world({ installedVersionCode: 3 });
    await waitFor(start(older), 'ready_wifi');
    expect(older.ran.some((c) => c.startsWith('install -r -g'))).toBe(true);
  });

  it('keeps other accessibility services and does not duplicate its own', async () => {
    const existing = world({ existingAccessibility: 'com.foo/.A:com.bar/.B' });
    await waitFor(start(existing), 'ready_wifi');
    expect(existing.accessibilityPut).toEqual([`com.foo/.A:com.bar/.B:${SERVICE}`]);

    const present = world({ existingAccessibility: `com.foo/.A:${SERVICE}` });
    await waitFor(start(present), 'ready_wifi');
    expect(present.accessibilityPut).toEqual([`com.foo/.A:${SERVICE}`]);
  });

  it('re-running setup re-provisions and pairs again over the stored key', async () => {
    const w = world({ installedVersionCode: 7 });
    const manager = start(w);
    await waitFor(manager, 'ready_wifi');
    manager.retry();
    await waitFor(manager, 'ready_wifi');
    expect(w.ran.some((c) => c.startsWith('install -r -g'))).toBe(false);
    expect(w.paired).toHaveLength(2);
    expect(w.paired[1].storeKey).toBe(w.paired[0].storeKey);
    expect(w.saved).toHaveLength(2);
  });

  it('mesh dep present but the switch off: no enrollment, no mesh broadcast, unchanged flow', async () => {
    const w = world();
    let enrolls = 0;
    const d = deps(w, {
      mesh: {
        enroll: async () => {
          enrolls++;
          return { controlUrl: 'https://ctl.example:8444', authKey: 'secret-key' };
        },
      },
    });
    const manager = new PhoneLinkManager({ sleep: instant, android: async () => d });
    manager.start('android');
    const ready = await waitFor(manager, 'ready_wifi');
    expect(ready.detail).toBe('10.0.0.7:8200');
    expect(enrolls).toBe(0);
    expect(w.ran.some((c) => c.includes('mesh_control_url'))).toBe(false);
    expect(w.saved.at(-1)).not.toHaveProperty('meshHost');
    expect(w.saved.at(-1)).not.toHaveProperty('meshPending');
  });

  it('reports install failures as retryable', async () => {
    const w = world({ installShouldFail: true });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'install_failed', retryable: true });
    expect(status.instruction).toBe("Couldn't install Allternit Phone Link on your phone. Unlock it and try again.");
    expect(w.forwardsRemoved).toEqual([]);
  });

  it('reports an unpublished helper as not retryable', async () => {
    const w = world({ helper: new AndroidHelperUnavailableError('not yet') });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'helper_unavailable', retryable: false });
    expect(status.instruction).toBe("This version of Allternit can't set up Android phones yet. Update Allternit, then try again.");
  });

  it('asks for the accessibility service when it never binds, and removes the forward on the way out', async () => {
    const w = world({ accessibilityBinds: false });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'accessibility_blocked', retryable: true });
    expect(status.instruction).toBe('On your phone, open Settings › Accessibility › Allternit Phone Link and turn it on, then try again.');
    expect(w.forwardsRemoved).toEqual([]);
  });

  it('fails pairing when the helper does not answer the cable forward', async () => {
    const w = world({ whoamiId: null });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'pair_failed', retryable: true });
    expect(w.forwardsRemoved).toEqual(['52123']);
  });

  it('fails pairing when the helper reports another device id', async () => {
    const w = world({ whoamiId: 'someone-else' });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'pair_failed', retryable: true });
    expect(w.paired).toHaveLength(0);
  });

  it('reports no Wi-Fi as a retryable error', async () => {
    const w = world({ wifiIp: null });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'no_wifi', retryable: true });
    expect(w.forwardsRemoved).toEqual(['52123']);
  });

  it('reports wifi_unreachable when the app does not answer over Wi-Fi', async () => {
    const w = world({ wifiDeviceId: null });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'wifi_unreachable', retryable: true });
    expect(status.instruction).toBe('Your phone and this computer need to be on the same Wi-Fi. Connect both, then try again.');
    expect(w.forwardsRemoved).toEqual(['52123']);
  });

  it('never trusts a Wi-Fi host that answers with another device id', async () => {
    const w = world({ wifiDeviceId: 'impostor' });
    const status = await waitFor(start(w), 'error');
    expect(status.error).toMatchObject({ code: 'wifi_unreachable', retryable: true });
  });

  it('cancel while waiting for the phone goes idle', async () => {
    const w = world({ phonePresent: false });
    const manager = new PhoneLinkManager({ sleep: () => new Promise((r) => setTimeout(r, 5)), android: async () => deps(w) });
    manager.start('android');
    await waitFor(manager, 'usb_debugging');
    expect(manager.cancel().state).toBe('idle');
    await new Promise((r) => setTimeout(r, 20));
    expect(manager.status().state).toBe('idle');
  });

  it('cancel mid-pairing removes the forward and goes idle', async () => {
    const w = world({ whoamiId: null });
    let attempts = 0;
    const d = deps(w, {
      ictrl: ((storeKey: string, host: string) => {
        const base = fakeIctrl(w)(storeKey, host);
        return {
          ...base,
          whoami: async () => {
            attempts++;
            if (attempts === 3) manager.cancel();
            return null;
          },
        };
      }) as AndroidDriverDeps['ictrl'],
    });
    const manager = new PhoneLinkManager({ sleep: instant, android: async () => d });
    manager.start('android');
    await new Promise((r) => setTimeout(r, 10));
    expect(manager.status().state).toBe('idle');
    expect(w.forwardsRemoved).toEqual(['52123']);
  });

  it('reconnectAndroid finds the phone by device id and saves the new address', async () => {
    // The app now lives at 10.0.0.11 after a reboot moved it (see discovery).
    const w = world({ lanHost: '10.0.0.11:8200' });
    const d = deps(w, {
      discovery: {
        lanInterfaces: () => [{ address: '10.0.0.5', netmask: '255.255.255.0' }],
        fetchWhoami: async (host) => (host === '10.0.0.11:8200' ? { device_id: DEVICE_ID } : null),
      },
    });
    d.store.save({
      deviceId: DEVICE_ID,
      serial: 'R5CT1',
      lanHost: '10.0.0.238:8200',
      pairedAt: new Date().toISOString(),
    });
    // The stale host must not be trusted even though "someone" answers there.
    const host = await reconnectAndroid(d, DEVICE_ID);
    expect(host).toBe('10.0.0.11:8200');
    expect(d.store.load(DEVICE_ID)?.lanHost).toBe('10.0.0.11:8200');
  });

  it('reconnectAndroid keeps the saved host when it still answers', async () => {
    const w = world();
    const d = deps(w, {
      discovery: {
        lanInterfaces: () => [],
        fetchWhoami: async () => ({ device_id: 'impostor' }),
      },
    });
    d.store.save({ deviceId: DEVICE_ID, serial: 'R5CT1', lanHost: '10.0.0.7:8200', pairedAt: new Date().toISOString() });
    expect(await reconnectAndroid(d, DEVICE_ID)).toBe('10.0.0.7:8200');
  });
});

describe('accessibilityBound', () => {
  const enabled = '     Enabled services:{{com.allternit.phonelink/com.allternit.phonelink.PhoneLinkAccessibilityService}}';
  it('needs the full class name enabled and the service bound by label', () => {
    expect(accessibilityBound(`     Bound services:{Service[label=Allternit Phone Link, capabilities=161]}\n${enabled}\n`)).toBe(true);
  });
  it('is false while enabled but not yet bound', () => {
    expect(accessibilityBound(`     Bound services:{}\n${enabled}\n`)).toBe(false);
  });
  it('does not accept the short component form, which dumpsys never prints', () => {
    expect(accessibilityBound('     Bound services:{Service[label=Allternit Phone Link]}\n     Enabled services:{{com.allternit.phonelink/.PhoneLinkAccessibilityService}}\n')).toBe(false);
  });
});
