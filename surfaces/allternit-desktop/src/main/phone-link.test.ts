import { describe, expect, it } from 'vitest';
import { installedVendorApps, parseAdbDevices, parseWlanIp, PhoneLinkManager } from './phone-link.js';
import type { AdbCall, AdbResult } from './phone-tools.js';
import type { PhoneLinkStatus } from './phone-link-types.js';

function fakeAdb(script: (args: string[]) => string): AdbCall {
  const call = (async (args: string[]): Promise<AdbResult> => ({ stdout: script(args), stderr: '', code: 0 })) as AdbCall;
  call.binary = async () => Buffer.alloc(0);
  return call;
}

function waitFor(manager: PhoneLinkManager, state: PhoneLinkStatus['state']): Promise<PhoneLinkStatus> {
  return new Promise((resolve) => {
    if (manager.status().state === state) return resolve(manager.status());
    manager.on('status', (s: PhoneLinkStatus) => s.state === state && resolve(s));
  });
}

const instant = () => Promise.resolve();

describe('parsers', () => {
  it('reads adb devices -l', () => {
    const out = 'List of devices attached\nR5CT1 device usb:1-1 product:x model:SM_S918U device:y\nZZ9 unauthorized usb:1-2\n';
    expect(parseAdbDevices(out)).toEqual([
      { serial: 'R5CT1', state: 'device', model: 'SM S918U' },
      { serial: 'ZZ9', state: 'unauthorized', model: undefined },
    ]);
  });

  it('finds installed vendor apps only', () => {
    const apps = installedVendorApps('package:com.openai.chatgpt\npackage:com.android.chrome\npackage:ai.x.grok\n');
    expect(apps).toEqual([
      { vendor: 'chatgpt', signedIn: null },
      { vendor: 'grok', signedIn: null },
    ]);
  });

  it('reads the wlan0 address', () => {
    expect(parseWlanIp('3: wlan0: <UP>\n    inet 192.168.1.42/24 brd 192.168.1.255 scope global wlan0')).toBe('192.168.1.42');
    expect(parseWlanIp('')).toBeNull();
  });
});

describe('PhoneLinkManager', () => {
  it('walks Android from USB to Wi-Fi', async () => {
    let authorized = false;
    const adb = fakeAdb((args) => {
      const cmd = args.join(' ');
      if (cmd === 'devices -l') {
        const line = authorized ? 'R5CT1 device usb:1 model:Pixel_8' : 'R5CT1 unauthorized usb:1';
        authorized = true;
        return `List of devices attached\n${line}\n`;
      }
      if (cmd.includes('ro.build.version.release')) return '15\n';
      if (cmd.includes('pm list packages')) return 'package:com.openai.chatgpt\n';
      if (cmd.includes('wlan0')) return 'inet 10.0.0.7/24';
      if (cmd.startsWith('connect')) return 'connected to 10.0.0.7:5555';
      return '';
    });
    const manager = new PhoneLinkManager({ adb: async () => adb, sleep: instant });
    const seen: string[] = [];
    manager.on('status', (s: PhoneLinkStatus) => seen.push(s.state));
    manager.start('android');
    const ready = await waitFor(manager, 'ready_wifi');
    expect(ready).toMatchObject({
      platform: 'android',
      deviceName: 'Pixel 8',
      osVersion: 'Android 15',
      detail: '10.0.0.7:5555',
      apps: [{ vendor: 'chatgpt', signedIn: null }],
    });
    expect(seen).toContain('usb_debugging');
    expect(seen).toContain('pairing');
  });

  it('reports no Wi-Fi as a retryable error', async () => {
    const adb = fakeAdb((args) => (args.join(' ') === 'devices -l' ? 'List of devices attached\nA device usb:1\n' : ''));
    const manager = new PhoneLinkManager({ adb: async () => adb, sleep: instant });
    manager.start('android');
    const status = await waitFor(manager, 'error');
    expect(status.error).toMatchObject({ code: 'no_wifi', retryable: true });
  });

  it('cancel returns to idle and ignores the old run', async () => {
    const adb = fakeAdb(() => 'List of devices attached\n');
    const manager = new PhoneLinkManager({ adb: async () => adb, sleep: () => new Promise((r) => setTimeout(r, 5)) });
    manager.start('android');
    await waitFor(manager, 'usb_debugging');
    expect(manager.cancel().state).toBe('idle');
    await new Promise((r) => setTimeout(r, 20));
    expect(manager.status().state).toBe('idle');
  });

  it('iPhone is not supported off macOS', () => {
    const manager = new PhoneLinkManager({ platform: 'win32' });
    expect(manager.start('ios').state).toBe('not_supported');
  });
});
