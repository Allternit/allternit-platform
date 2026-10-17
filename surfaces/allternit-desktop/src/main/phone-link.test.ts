import { describe, expect, it } from 'vitest';
import { PhoneLinkManager } from './phone-link.js';
import { installedVendorApps, parseAdbDevices, parseWlanIp } from './phone-link-android.js';
import type { PhoneLinkStatus } from './phone-link-types.js';

function waitFor(manager: PhoneLinkManager, state: PhoneLinkStatus['state']): Promise<PhoneLinkStatus> {
  return new Promise((resolve) => {
    if (manager.status().state === state) return resolve(manager.status());
    manager.on('status', (s: PhoneLinkStatus) => s.state === state && resolve(s));
  });
}

describe('parsers', () => {
  it('reads adb devices -l', () => {
    const out = 'List of devices attached\nR5CT1 device usb:1-1 product:x model:SM_S918U device:y\nZZ9 unauthorized usb:1-2\n';
    expect(parseAdbDevices(out)).toEqual([
      { serial: 'R5CT1', state: 'device', model: 'SM S918U' },
      { serial: 'ZZ9', state: 'unauthorized', model: undefined },
    ]);
  });

  it('finds installed vendor apps only', () => {
    const apps = installedVendorApps('package:com.openai.chatgpt\npackage:com.android.chrome\npackage:ai.x.grok\npackage:org.telegram.messenger\n');
    expect(apps).toEqual([
      { vendor: 'chatgpt', signedIn: null, name: 'ChatGPT', category: 'AI chat', provider: 'chatgpt' },
      { vendor: 'grok', signedIn: null, name: 'Grok', category: 'AI chat' },
      { vendor: 'telegram', signedIn: null, name: 'Telegram', category: 'Communication' },
    ]);
  });

  it('reads the wlan0 address', () => {
    expect(parseWlanIp('3: wlan0: <UP>\n    inet 192.168.1.42/24 brd 192.168.1.255 scope global wlan0')).toBe('192.168.1.42');
    expect(parseWlanIp('')).toBeNull();
  });
});

describe('PhoneLinkManager', () => {
  it('iPhone is not supported off macOS', () => {
    const manager = new PhoneLinkManager({ platform: 'win32' });
    expect(manager.start('ios').state).toBe('not_supported');
  });
});
