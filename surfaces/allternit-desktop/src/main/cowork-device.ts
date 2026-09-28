/**
 * This computer's identity for Settings → Cowork devices, plus the "open
 * links in the built-in browser" preference.
 *
 * The device id is generated once and kept in the app store, so it survives
 * updates and reinstalls of the app bundle. Main injects it on every request
 * to the local API (see onBeforeSendHeaders in unified-main.ts); the renderer
 * reads it over `device:info` to register this computer and to mark it
 * "This computer" in the devices table.
 */

import { randomUUID } from 'node:crypto';
import * as os from 'node:os';

export interface CoworkDeviceInfo {
  id: string;
  name: string;
  platform: string;
  kind: 'desktop';
}

export interface DeviceStore {
  get(key: 'coworkDeviceId'): string | undefined;
  set(key: 'coworkDeviceId', value: string): void;
}

export function platformLabel(platform: NodeJS.Platform = process.platform): string {
  if (platform === 'darwin') return 'macOS';
  if (platform === 'win32') return 'Windows';
  if (platform === 'linux') return 'Linux';
  return platform;
}

/** Host name without the `.local` / domain suffix macOS and routers add. */
export function shortHostname(hostname: string = os.hostname()): string {
  return hostname.replace(/\.(local|lan|home)$/i, '').split('.')[0] || hostname;
}

export function coworkDeviceInfo(store: DeviceStore): CoworkDeviceInfo {
  let id = store.get('coworkDeviceId');
  if (!id) {
    id = `desktop-${randomUUID()}`;
    store.set('coworkDeviceId', id);
  }
  const platform = platformLabel();
  return {
    id,
    name: `Allternit Desktop (${shortHostname()})`,
    platform,
    kind: 'desktop',
  };
}

export function coworkDeviceHeaders(info: CoworkDeviceInfo): Record<string, string> {
  return {
    'X-Allternit-Device-Id': info.id,
    // Header values must be ByteStrings; non-Latin-1 host names are replaced.
    'X-Allternit-Device-Name': info.name.replace(/[^\x20-\x7e]/g, '?'),
    'X-Allternit-Device-Platform': info.platform,
  };
}

/** Links that may open in the app's own browser instead of the system one. */
export function isInAppBrowsableUrl(rawUrl: string): boolean {
  try {
    const { protocol } = new URL(rawUrl);
    return protocol === 'http:' || protocol === 'https:';
  } catch {
    return false;
  }
}
