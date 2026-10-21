/** IPC for the Settings → Computers → Phones UI. All channels are trusted-sender guarded by the caller. */

import { BrowserWindow, type IpcMainInvokeEvent } from 'electron';
import { phoneService } from './phone-service.js';
import type { PhoneDeviceManager } from './phone-device-manager.js';

type Guarded = (channel: string, fn: (event: IpcMainInvokeEvent, ...args: any[]) => unknown) => void;

function broadcast(channel: string, payload: unknown): void {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) win.webContents.send(channel, payload);
  }
}

let wired = false;
function wire(manager: PhoneDeviceManager): void {
  if (wired) return;
  wired = true;
  manager.on('devices', () => broadcast('phone:devices', manager.list()));
  manager.on('pairing', (status) => broadcast('phone:pairing', status));
}

async function snapshot() {
  const art = phoneService.getArtemis()?.status() ?? null;
  let manager: PhoneDeviceManager | null = null;
  try {
    manager = await phoneService.ensureReady();
    wire(manager);
    return { ready: true as const, phones: manager.list(), pairing: manager.pairingStatus(), scrcpy: manager.scrcpyAvailable(), artemis: art };
  } catch (error) {
    return { ready: false as const, error: error instanceof Error ? error.message : String(error), phones: [], artemis: art };
  }
}

export function registerPhoneIpc(handle: Guarded): void {
  handle('phone:status', () => snapshot());
  handle('phone:pair-qr-start', async () => {
    const manager = await phoneService.ensureReady();
    wire(manager);
    return { payload: manager.startQrPairing().payload };
  });
  handle('phone:pair-code', async (_e, hostPort: unknown, code: unknown) => {
    const manager = await phoneService.ensureReady();
    wire(manager);
    return manager.pairWithCode(String(hostPort ?? ''), String(code ?? ''));
  });
  handle('phone:pair-cancel', async () => (await phoneService.ensureReady()).cancelPairing());
  handle('phone:forget', async (_e, id: unknown) => (await phoneService.ensureReady()).forget(String(id)));
  handle('phone:scrcpy-start', async (_e, serial: unknown) => (await phoneService.ensureReady()).startScrcpy(String(serial)));
  handle('phone:scrcpy-stop', async (_e, serial: unknown) => (await phoneService.ensureReady()).stopScrcpy(String(serial)));
  handle('phone:try-open-app', (_e, serial: unknown, app: unknown) => phoneService.tryOpenApp(String(serial), String(app)));
  handle('phone:screenshot', (_e, serial: unknown) => phoneService.tryScreenshot(String(serial)));
  handle('phone:artemis-install', async () => {
    await phoneService.ensureReady();
    return phoneService.getArtemis()?.install();
  });
}
