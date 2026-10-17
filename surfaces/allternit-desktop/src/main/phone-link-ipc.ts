/** IPC for Phone Link setup (phone-link.ts). All channels are trusted-sender guarded by the caller. */

import { BrowserWindow, type IpcMainInvokeEvent } from 'electron';
import log from 'electron-log';
import { PhoneLinkManager } from './phone-link.js';
import type { PhoneLinkPlatform } from './phone-link-types.js';
import { phoneService } from './phone-service.js';

type Guarded = (channel: string, fn: (event: IpcMainInvokeEvent, ...args: any[]) => unknown) => void;

export function registerPhoneLinkIpc(handleGuarded: Guarded): PhoneLinkManager {
  const manager = new PhoneLinkManager({
    adb: () => phoneService.ensureAdbCall(),
    logger: (message) => log.info(message),
  });
  manager.on('status', (status) => {
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) win.webContents.send('phone-link:status-changed', status);
    }
  });
  handleGuarded('phone-link:status', () => manager.status());
  handleGuarded('phone-link:start', (_event, opts?: { platform?: PhoneLinkPlatform }) => {
    const platform = opts?.platform;
    if (platform !== 'ios' && platform !== 'android') throw new Error('platform must be ios or android');
    return manager.start(platform);
  });
  handleGuarded('phone-link:cancel', () => manager.cancel());
  handleGuarded('phone-link:continue', () => manager.continue());
  handleGuarded('phone-link:retry', () => manager.retry());
  handleGuarded('phone-link:submit-apple-id', (_event, creds?: { appleId?: unknown; password?: unknown }) => {
    if (typeof creds?.appleId !== 'string' || typeof creds?.password !== 'string') throw new Error('appleId and password are required');
    return manager.submitAppleId({ appleId: creds.appleId, password: creds.password });
  });
  handleGuarded('phone-link:submit-two-factor', (_event, code?: unknown) => {
    if (typeof code !== 'string' || !/^\d{6}$/.test(code)) throw new Error('code must be 6 digits');
    return manager.submitTwoFactor(code);
  });
  return manager;
}
