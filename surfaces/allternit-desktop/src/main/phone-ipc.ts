/** IPC for the Settings → Computers → Phones UI. All channels are trusted-sender guarded by the caller. */

import { BrowserWindow, type IpcMainInvokeEvent } from 'electron';
import { phoneService } from './phone-service.js';
import type { PhoneDeviceManager } from './phone-device-manager.js';
import type { UsbAgentManager } from './phone-usb-agent.js';

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

let usbWired = false;
function wireUsb(usb: UsbAgentManager): void {
  if (usbWired) return;
  usbWired = true;
  usb.on('devices', () => broadcast('phone:usb-devices', usb.list()));
}

/** The USB Agent lane runs on macOS and Linux (UxPlay and the serial setup are Unix-only). */
const USB_SUPPORTED = process.platform === 'darwin' || process.platform === 'linux';

async function usbSnapshot() {
  if (!USB_SUPPORTED) return { supported: false as const };
  try {
    const usb = await phoneService.ensureUsbReady();
    wireUsb(usb);
    return {
      supported: true as const,
      ready: true as const,
      uxplay: usb.airplayAvailable(),
      firmwareVersion: (() => {
        try {
          return phoneService.usbFirmware().version;
        } catch {
          return null;
        }
      })(),
      airplayPin: usb.airplayPin(),
      boards: usb.list(),
    };
  } catch (error) {
    return { supported: true as const, ready: false as const, error: error instanceof Error ? error.message : String(error), boards: [] };
  }
}

async function usb(): Promise<UsbAgentManager> {
  const m = await phoneService.ensureUsbReady();
  wireUsb(m);
  return m;
}

const errorResult = (error: unknown) => ({ ok: false as const, message: error instanceof Error ? error.message : String(error) });

async function snapshot() {
  const art = phoneService.getArtemis()?.status() ?? null;
  // Opening Settings shouldn't download adb for someone with no Android phone;
  // pairing (phone:pair-*) brings the lane up when they add one.
  if (!phoneService.androidInUse()) {
    return { ready: true as const, phones: [], pairing: { phase: 'idle' as const }, scrcpy: phoneService.scrcpyInstalled(), artemis: art };
  }
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
  handle('phone:home', (_e, serial: unknown) => phoneService.tryHome(String(serial)));

  // iPhone over an Allternit USB Agent
  handle('phone:usb-status', () => usbSnapshot());
  handle('phone:usb-ports', async () => (USB_SUPPORTED ? (await usb()).serialPorts() : []));
  handle('phone:usb-identify', async (_e, port: unknown) => {
    try {
      return { ok: true as const, ...(await (await usb()).identify(String(port))) };
    } catch (error) {
      return errorResult(error);
    }
  });
  handle('phone:usb-setup', async (_e, port: unknown, ssid: unknown, password: unknown, name: unknown) => {
    try {
      const board = await (await usb()).setUp(String(port), { ssid: String(ssid ?? ''), password: String(password ?? '') }, String(name ?? ''));
      return { ok: true as const, board };
    } catch (error) {
      return errorResult(error);
    }
  });
  // The firmware image for the setup screen to flash (esptool-js over Web Serial, Espressif boards only).
  handle('phone:usb-firmware', () => {
    try {
      const fw = phoneService.usbFirmware();
      return { ok: true as const, version: fw.version, sha256: fw.sha256, offset: fw.offset, data: fw.data };
    } catch (error) {
      return errorResult(error);
    }
  });
  handle('phone:usb-calibrate', async (_e, boardId: unknown) => (await usb()).calibrate(String(boardId)));
  handle('phone:usb-stop', async (_e, boardId: unknown) => (await usb()).stop(String(boardId)));
  handle('phone:usb-forget', async (_e, boardId: unknown) => (await usb()).forget(String(boardId)));

  handle('phone:artemis-install', async () => {
    await phoneService.ensureReady();
    return phoneService.getArtemis()?.install();
  });
}
