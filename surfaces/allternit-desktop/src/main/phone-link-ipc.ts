/** IPC for Phone Link setup (phone-link.ts). All channels are trusted-sender guarded by the caller. */

import { app, BrowserWindow, type IpcMainInvokeEvent } from 'electron';
import log from 'electron-log';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { IctrlClient, ictrlStoreKey, type SecretStore } from './ictrl-client.js';
import { PhoneLinkManager } from './phone-link.js';
import { fileAndroidLinkStore, type AndroidDriverDeps } from './phone-link-android.js';
import { fileLinkStore, type IosDriverDeps } from './phone-link-ios.js';
import type { PhoneLinkPlatform } from './phone-link-types.js';
import { createGoIosExec, createGoIosSpawn, ensureDdi, ensureGoIos, GoIos } from './phone-goios.js';
import { ensureRunnerArtifacts, pickSigner } from './phone-ios-runner.js';
import { ensureAndroidHelper } from './phone-android-helper.js';
import { readPairRecord, startLoopbackForward } from './phone-usbmux.js';
import { phoneService } from './phone-service.js';
import { meshManager } from './mesh-manager.js';
import { deleteSecret, readSecret, writeSecret } from './secure-store.js';

const secrets: SecretStore = { get: readSecret, set: writeSecret, delete: deleteSecret };

/**
 * Mesh credentials for Phone Link setup, built on MeshManager
 * (phone-link-mesh.ts). Rejects when Desktop isn't signed in to an Allternit
 * account; the drivers treat that as non-fatal. The single-use auth key is
 * passed straight to the phone and never logged.
 */
const meshDep = {
  enroll: async () => {
    const enrollment = await meshManager.enrollForGizzi();
    if (!enrollment) throw new Error('Desktop is not signed in to an Allternit account.');
    return enrollment;
  },
};

/** GET /schema on an iCTRL runner; null when it doesn't answer or has none. */
async function fetchRunnerSchema(host: string): Promise<unknown | null> {
  try {
    const res = await fetch(`http://${host}/schema`, { signal: AbortSignal.timeout(5000) });
    if (res.status !== 200) return null;
    return await res.json();
  } catch {
    return null;
  }
}

async function iosDeps(): Promise<IosDriverDeps> {
  const dataDir = path.join(app.getPath('userData'), 'phone');
  const bin = await ensureGoIos(dataDir);
  // go-ios keeps its tunnel identity (selfIdentity.plist) here; default would be the cwd.
  const tunnelDir = path.join(dataDir, 'go-ios', 'tunnel-identity');
  fs.mkdirSync(tunnelDir, { recursive: true, mode: 0o700 });
  const goios = new GoIos(createGoIosExec(bin), createGoIosSpawn(bin, tunnelDir), tunnelDir);
  return {
    goios,
    artifacts: () => ensureRunnerArtifacts(dataDir),
    signer: pickSigner(app.isPackaged),
    ictrl: (udid, host) => new IctrlClient({ host, secrets, storeKey: ictrlStoreKey(udid), logger: (m) => log.info(m) }),
    forward: (udid, port) => startLoopbackForward(udid, port),
    ddiDir: async () => {
      const dir = path.join(dataDir, 'ddi');
      await ensureDdi(dir);
      return dir;
    },
    readPairRecord: (udid) => readPairRecord(udid),
    store: fileLinkStore(path.join(dataDir, 'phone-link-ios.json')),
    mesh: meshDep,
    fetchSchema: fetchRunnerSchema,
    workDir: path.join(dataDir, 'ictrl-work'),
    logger: (m) => log.info(m),
  };
}

async function androidDeps(): Promise<AndroidDriverDeps> {
  const dataDir = path.join(app.getPath('userData'), 'phone');
  return {
    adb: () => phoneService.ensureAdbCall(),
    helper: (dir) => ensureAndroidHelper(dir),
    dataDir,
    ictrl: (storeKey, host) => new IctrlClient({ host, secrets, storeKey, logger: (m) => log.info(m) }),
    store: fileAndroidLinkStore(path.join(dataDir, 'phone-link-android.json')),
    mesh: meshDep,
    logger: (m) => log.info(m),
  };
}

type Guarded = (channel: string, fn: (event: IpcMainInvokeEvent, ...args: any[]) => unknown) => void;

export function registerPhoneLinkIpc(handleGuarded: Guarded): PhoneLinkManager {
  const manager = new PhoneLinkManager({
    android: androidDeps,
    ios: iosDeps,
    logger: (message) => log.info(message),
  });
  manager.on('status', (status) => {
    // A phone just finished setup: make sure bots get the phone tools now.
    if (status.state === 'ready_wifi') void phoneService.startForLinkedPhones();
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) win.webContents.send('phone-link:status-changed', status);
    }
  });
  handleGuarded('phone-link:status', () => manager.status());
  handleGuarded('phone-link:devices', () => phoneService.linkedDevices());
  handleGuarded('phone-link:handover-targets', () => phoneService.handoverTargets());
  handleGuarded('phone-link:handover', (_event, opts?: { deviceId?: unknown; computerId?: unknown }) => {
    if (typeof opts?.deviceId !== 'string' || typeof opts?.computerId !== 'string') throw new Error('deviceId and computerId are required');
    return phoneService.handover({ deviceId: opts.deviceId, computerId: opts.computerId });
  });
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
