/**
 * Allternit Desktop — Preload Script
 *
 * Exposes safe, structured APIs to the renderer via contextBridge.
 * The renderer (https://platform.allternit.com or local backend /platform)
 * accesses these as `window.allternit.*`.
 */

import { contextBridge, ipcRenderer, IpcRendererEvent } from 'electron';

// Same-origin gateway (local allternit-api). Main injects the paired device
// token and the local API introspects it against cloudApiUrl.
try {
  const info = ipcRenderer.sendSync('app:get-platform-url');
  const platformUrl =
    info && typeof info === 'object' ? info.platformUrl : info;
  if (typeof platformUrl === 'string' && platformUrl) {
    (window as any).__ALLTERNIT_GATEWAY_URL__ = platformUrl;
  }
  if (info && typeof info === 'object' && typeof info.gatewayUrl === 'string') {
    (window as any).__ALLTERNIT_CLOUD_API_URL__ = info.gatewayUrl;
  }
} catch (e) {
  console.warn('[preload] Could not set __ALLTERNIT_GATEWAY_URL__:', e);
}

// ─── SDK / Backend URL ────────────────────────────────────────────────────────

const sdkAPI = {
  /** Resolves the active backend URL so the renderer can init its API client. */
  getBackendUrl: (): Promise<string> => ipcRenderer.invoke('sdk:get-backend-url'),
};

// ─── Connection / Backend mode ────────────────────────────────────────────────

const connectionAPI = {
  test: (): Promise<{ mode: string; url: string; status: string; lastError?: string }> =>
    ipcRenderer.invoke('connection:test'),
  getBackend: (): Promise<{ mode: 'bundled' | 'remote' | 'development'; url: string }> =>
    ipcRenderer.invoke('connection:get-backend'),
  setBackend: (config: { mode: 'bundled' | 'remote'; remoteUrl?: string }): Promise<void> =>
    ipcRenderer.invoke('connection:set-backend', config),
  onStateChange: (callback: (state: unknown) => void): (() => void) => {
    const handler = (_: IpcRendererEvent, state: unknown) => callback(state);
    ipcRenderer.on('connection:state', handler);
    return () => ipcRenderer.off('connection:state', handler);
  },
};

// ─── Backend / Sidecar lifecycle ──────────────────────────────────────────────

const backendAPI = {
  getStatus: (): Promise<{ installed: boolean; running: boolean; version?: string; url: string }> =>
    ipcRenderer.invoke('backend:get-status'),
  restart: (): Promise<string> => ipcRenderer.invoke('backend:restart'),
  /** Legacy sidecar controls (API server subprocess) */
  sidecar: {
    start: (): Promise<boolean> => ipcRenderer.invoke('sidecar:start'),
    stop: (): Promise<boolean> => ipcRenderer.invoke('sidecar:stop'),
    restart: (): Promise<boolean> => ipcRenderer.invoke('sidecar:restart'),
    getStatus: (): Promise<'stopped' | 'starting' | 'running' | 'error' | 'crashed'> =>
      ipcRenderer.invoke('sidecar:get-status'),
    getApiUrl: (): Promise<string | undefined> => ipcRenderer.invoke('sidecar:get-api-url'),
    onStatusChanged: (handler: (status: string) => void): (() => void) => {
      const listener = (_: IpcRendererEvent, s: string) => handler(s);
      ipcRenderer.on('sidecar:status-changed', listener);
      return () => ipcRenderer.removeListener('sidecar:status-changed', listener);
    },
  },
  onDownloadProgress: (handler: (progress: { stage: string; percent: number }) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, p: { stage: string; percent: number }) => handler(p);
    ipcRenderer.on('backend:download-progress', listener);
    return () => ipcRenderer.removeListener('backend:download-progress', listener);
  },
};

// ─── Engine status (consumer-packaged Cowork P1) ─────────────────────────────
// One aggregate green/yellow/red indicator over the four managed engines.
// Present only in the packaged desktop bridge; absent in browser/cloud.

export interface EngineComponentStatus {
  status: 'pending' | 'up' | 'down';
  detail: string;
}

export interface EnginesStatus {
  api: EngineComponentStatus;
  gizzi: EngineComponentStatus;
  fabricWorker: EngineComponentStatus;
  office: EngineComponentStatus;
}

const enginesAPI = {
  getStatus: (): Promise<EnginesStatus> => ipcRenderer.invoke('engines:get-status'),
  onStatusChange: (handler: (status: EnginesStatus) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, s: EnginesStatus) => handler(s);
    ipcRenderer.on('engines:status', listener);
    return () => ipcRenderer.removeListener('engines:status', listener);
  },
};

// ─── Bonsai local image companion ─────────────────────────────────────────────

export interface BonsaiStatus {
  installed: boolean;
  running: boolean;
  installing: boolean;
  url: string;
  revisions?: { source?: string; model?: string; mlxWheel?: string };
  installDir: string;
  error?: string;
}

const bonsaiAPI = {
  getStatus: (): Promise<BonsaiStatus> => ipcRenderer.invoke('bonsai:get-status'),
  install: (): Promise<void> => ipcRenderer.invoke('bonsai:install'),
  cancelInstall: (): Promise<boolean> => ipcRenderer.invoke('bonsai:cancel-install'),
  start: (): Promise<void> => ipcRenderer.invoke('bonsai:start'),
  stop: (): Promise<boolean> => ipcRenderer.invoke('bonsai:stop'),
  remove: (): Promise<void> => ipcRenderer.invoke('bonsai:remove'),
  onProgress: (handler: (progress: { stage: string; message: string }) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, p: { stage: string; message: string }) => handler(p);
    ipcRenderer.on('bonsai:progress', listener);
    return () => ipcRenderer.removeListener('bonsai:progress', listener);
  },
};

// ─── System One (S1) + Laya ───────────────────────────────────────────────────
// Settings → Models & tiers reads install state from here (Q28/Q29).

export interface SystemOneStatus {
  layaSupported: boolean;
  systemOne: { available: boolean; running: boolean; url: string };
  laya: {
    installed: boolean;
    installing: boolean;
    running: boolean;
    url: string;
    version: string;
    checkpoint: { source: 'pinned' | 'revision' | 'path'; revision?: string; path?: string };
    needsUv: boolean;
    uvHint?: string;
    installDir: string;
  };
  backend: 'laya_bundled' | 'system_one_local';
  apiBackend?: 'laya_bundled' | 'system_one_local' | 'auto';
  shadowDir: string | null;
  shadowState: boolean;
  liveMode: boolean;
  embed: { running: boolean; url: string };
  error?: string;
}

export interface SystemOneProgress {
  stage: 'starting' | 'installing' | 'ready' | 'error' | 'cancelled' | 'needs-uv';
  message: string;
}

const systemOneAPI = {
  getStatus: (): Promise<SystemOneStatus> => ipcRenderer.invoke('system-one:get-status'),
  install: (): Promise<void> => ipcRenderer.invoke('system-one:install'),
  cancelInstall: (): Promise<boolean> => ipcRenderer.invoke('system-one:cancel-install'),
  repair: (): Promise<void> => ipcRenderer.invoke('system-one:repair'),
  remove: (): Promise<void> => ipcRenderer.invoke('system-one:remove'),
  start: (): Promise<void> => ipcRenderer.invoke('system-one:start'),
  stop: (): Promise<boolean> => ipcRenderer.invoke('system-one:stop'),
  setCheckpoint: (checkpoint: { revision?: string; path?: string } | null): Promise<SystemOneStatus['laya']['checkpoint']> =>
    ipcRenderer.invoke('system-one:set-checkpoint', checkpoint),
  setShadowState: (enabled: boolean): Promise<boolean> => ipcRenderer.invoke('system-one:set-shadow-state', enabled),
  setLiveMode: (enabled: boolean): Promise<boolean> => ipcRenderer.invoke('system-one:set-live-mode', enabled),
  onProgress: (handler: (progress: SystemOneProgress) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, p: SystemOneProgress) => handler(p);
    ipcRenderer.on('system-one:progress', listener);
    return () => ipcRenderer.removeListener('system-one:progress', listener);
  },
};

// ─── VM Setup (onboarding wizard) ─────────────────────────────────────────────

type DownloadProgress = {
  stage: 'downloading' | 'verifying' | 'extracting' | 'complete';
  fileName: string;
  bytesDownloaded: number;
  totalBytes: number;
  speed: number;
  eta: number;
};

type InitProgress = {
  stage: 'verifying' | 'booting' | 'connecting' | 'ready';
  message: string;
  progress: number;
};

const vmAPI = {
  checkConnectivity: (): Promise<{ internet: boolean; github: boolean; allternitServices: boolean }> =>
    ipcRenderer.invoke('vm-setup:check-connectivity'),

  downloadImages: (onProgress: (p: DownloadProgress) => void): Promise<boolean> => {
    const listener = (_: IpcRendererEvent, p: unknown) => onProgress(p as DownloadProgress);
    ipcRenderer.on('vm-setup:download-progress', listener);
    return ipcRenderer.invoke('vm-setup:download-images').finally(() => {
      ipcRenderer.removeListener('vm-setup:download-progress', listener);
    });
  },

  initializeVm: (onProgress: (p: InitProgress) => void): Promise<boolean> => {
    const listener = (_: IpcRendererEvent, p: unknown) => onProgress(p as InitProgress);
    ipcRenderer.on('vm-setup:init-progress', listener);
    return ipcRenderer.invoke('vm-setup:initialize-vm').finally(() => {
      ipcRenderer.removeListener('vm-setup:init-progress', listener);
    });
  },

  checkImagesExist: (): Promise<boolean> => ipcRenderer.invoke('vm-setup:check-images-exist'),
  getStatus: (): Promise<'stopped' | 'starting' | 'running' | 'error'> =>
    ipcRenderer.invoke('vm-setup:get-vm-status'),
};

// ─── Window controls ──────────────────────────────────────────────────────────

type WindowBounds = { x: number; y: number; width: number; height: number };
type WindowState = {
  maximized: boolean;
  minimized: boolean;
  fullscreen: boolean;
  focused: boolean;
  bounds: WindowBounds;
};

const windowAPI = {
  minimize: (): Promise<void> => ipcRenderer.invoke('window:minimize'),
  maximize: (): Promise<{ maximized: boolean }> => ipcRenderer.invoke('window:maximize'),
  close: (): Promise<void> => {
    console.warn('[preload] window.close() invoked from renderer', new Error('trace').stack);
    return ipcRenderer.invoke('window:close');
  },
  isMaximized: (): Promise<boolean> => ipcRenderer.invoke('window:is-maximized'),
  fullscreen: (enabled?: boolean): Promise<{ fullscreen: boolean }> =>
    ipcRenderer.invoke('window:fullscreen', enabled),
  setAlwaysOnTop: (enabled: boolean): Promise<void> =>
    ipcRenderer.invoke('window:set-always-on-top', enabled),
  getState: (): Promise<WindowState> => ipcRenderer.invoke('window:get-state'),
  getBounds: (): Promise<WindowBounds> => ipcRenderer.invoke('window:get-bounds'),
  setBounds: (bounds: Partial<WindowBounds>): Promise<void> =>
    ipcRenderer.invoke('window:set-bounds', bounds),
  center: (): Promise<void> => ipcRenderer.invoke('window:center'),
  hide: (): Promise<void> => ipcRenderer.invoke('window:hide'),
  show: (): Promise<void> => ipcRenderer.invoke('window:show'),
  minimizeToTray: (): Promise<void> => ipcRenderer.invoke('window:minimize-to-tray'),
  onEvent: (event: string, handler: (payload: unknown) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, payload: unknown) => handler(payload);
    ipcRenderer.on(`window:event:${event}`, listener);
    return () => ipcRenderer.removeListener(`window:event:${event}`, listener);
  },
};

// ─── Store ────────────────────────────────────────────────────────────────────

const storeAPI = {
  get: <T>(key: string): Promise<T> => ipcRenderer.invoke('store:get', key) as Promise<T>,
  set: <T>(key: string, value: T): Promise<void> => ipcRenderer.invoke('store:set', key, value),
};

// ─── App info ─────────────────────────────────────────────────────────────────

export type UpdateStatus =
  | { state: 'checking' }
  | { state: 'available' }
  | { state: 'up-to-date' }
  | { state: 'downloaded'; version?: string; releaseNotes?: string; updateURL?: string }
  | { state: 'error'; message: string };

const appAPI = {
  getInfo: (): Promise<{
    version: string;
    platform: string;
    isPackaged: boolean;
    manifest: unknown;
    buildInfo?: {
      version?: string;
      buildVersion?: string;
      buildSuffix?: string;
      builtAt?: string;
    } | null;
  }> => ipcRenderer.invoke('app:get-info'),
  isFirstLaunch: (): Promise<boolean> => ipcRenderer.invoke('app:is-first-launch'),
  completeOnboarding: (): Promise<boolean> => ipcRenderer.invoke('app:complete-onboarding'),
  checkForUpdates: (): Promise<{ ok: boolean; reason?: string; message?: string }> =>
    ipcRenderer.invoke('app:check-for-updates'),
  installUpdate: (): Promise<void> => ipcRenderer.invoke('app:install-update'),
  onUpdateStatus: (handler: (status: UpdateStatus) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, status: UpdateStatus) => handler(status);
    ipcRenderer.on('app:update-status', listener);
    return () => ipcRenderer.removeListener('app:update-status', listener);
  },
  /** Record this computer as a trusted Cowork device (main holds the API secret). */
  registerDevice: (): Promise<boolean> => ipcRenderer.invoke('device:register'),
  /** This computer in Settings → Cowork devices (stable id, host name, OS). */
  deviceInfo: ipcRenderer.sendSync('device:info') as {
    id: string;
    name: string;
    platform: string;
    kind: 'desktop';
  } | null,
};

// ─── Links ──────────────────────────────────────────────────────────────────
// Settings → Cowork "Open links in the built-in browser": the renderer mirrors
// the saved preference to main, and main hands external links back here
// instead of the system browser while it's on.

const linksAPI = {
  setOpenInApp: (enabled: boolean): Promise<void> => ipcRenderer.invoke('links:set-open-in-app', enabled),
  onOpenInApp: (callback: (url: string) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, url: string) => callback(url);
    ipcRenderer.on('links:open-in-app', listener);
    return () => ipcRenderer.removeListener('links:open-in-app', listener);
  },
};

// ─── Auth ─────────────────────────────────────────────────────────────────────

const authAPI = {
  startLogin: async (): Promise<void> => {
    ipcRenderer.send('auth:start-login');
  },
  getSession: (): Promise<null | {
    userId: string;
    userEmail: string;
    expiresAt: number;
    runtimeId: string;
    organizationId?: string;
    capabilities: string[];
  }> => ipcRenderer.invoke('auth:get-session'),
  listAccounts: (): Promise<Array<{
    userId: string;
    userEmail: string;
    clientId: string;
    lastSignedInAt: string;
    lastSeenAt: string;
    current: boolean;
    backend?: {
      mode: 'bundled' | 'remote' | 'development';
      remoteUrl?: string;
    };
  }>> => ipcRenderer.invoke('auth:list-accounts'),
  forgetAccount: (userId: string): Promise<void> =>
    ipcRenderer.invoke('auth:forget-account', userId),
  signOut: (): Promise<void> => ipcRenderer.invoke('auth:sign-out'),
  hardSignOut: (): Promise<void> => ipcRenderer.invoke('auth:sign-out'),
  getClerkToken: (): Promise<string | null> => ipcRenderer.invoke('auth:get-clerk-token'),
  onSessionUpdated: (handler: (session: { userId: string; userEmail: string }) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, session: { userId: string; userEmail: string }) => handler(session);
    ipcRenderer.on('auth:session-updated', listener);
    return () => ipcRenderer.removeListener('auth:session-updated', listener);
  },
};

// ─── Device Pairing ───────────────────────────────────────────────────────────
// Approving `gizzi pair` codes in-app. Electron main holds the runtime device
// credential and brokers these calls to the Allternit Cloud API.

export type PairingInfo = {
  pairingId: string;
  userCode: string;
  name: string;
  runtimeType: string;
  hostname?: string;
  platform?: string;
  publicKeyFingerprint: string;
  capabilities: string[];
  status: string;
  expiresAt: string;
};

export type RuntimeDevice = {
  id: string;
  name: string;
  runtimeType: string;
  hostname?: string;
  platform?: string;
  version?: string;
  capabilities: string[];
  publicKeyFingerprint: string;
  status: string;
  lastSeenAt?: string;
  createdAt: string;
  credentialExpiresAt: string;
};

const devicePairingAPI = {
  lookup: (code: string): Promise<PairingInfo> => ipcRenderer.invoke('device-pairing:lookup', code),
  approve: (code: string): Promise<{ status: string; pairingId?: string; runtimeName?: string }> =>
    ipcRenderer.invoke('device-pairing:approve', code),
  deny: (code: string): Promise<{ status: string }> => ipcRenderer.invoke('device-pairing:deny', code),
  listDevices: (): Promise<RuntimeDevice[]> => ipcRenderer.invoke('device-pairing:list'),
};

// ─── Mesh (Allternit tailnet client) ──────────────────────────────────────────
// Reaching gizzi instances registered with 100.x mesh URLs. Electron main
// enrolls against the cloud API with the runtime device credential and runs
// mesh-node sidecars; the renderer only ever sees loopback URLs.

export type MeshState = 'stopped' | 'starting' | 'running' | 'error';

export type MeshStatus = {
  state: MeshState;
  meshIp?: string;
  error?: string;
  proxies: Array<{ target: string; url: string }>;
};

const meshAPI = {
  start: (): Promise<MeshStatus> => ipcRenderer.invoke('mesh:start'),
  stop: (): Promise<MeshStatus> => ipcRenderer.invoke('mesh:stop'),
  status: (): Promise<MeshStatus> => ipcRenderer.invoke('mesh:status'),
  proxyFor: (instanceUrl: string): Promise<string> => ipcRenderer.invoke('mesh:proxy-for', instanceUrl),
};

// ─── Shell ────────────────────────────────────────────────────────────────────

const shellAPI = {
  openExternal: (url: string): Promise<void> => ipcRenderer.invoke('shell:open-external', url),
  openDesign: (): Promise<void> => ipcRenderer.invoke('shell:open-design'),
  openOfficeWindow: (): Promise<void> => ipcRenderer.invoke('shell:open-office-window'),
  openRemoteControl: (runtimeId?: string): Promise<void> => ipcRenderer.invoke('shell:open-remote-control', runtimeId),
  openFabricSession: (runtimeId?: string): Promise<void> => ipcRenderer.invoke('shell:open-fabric-session', runtimeId),
  openHud: (): Promise<void> => ipcRenderer.invoke('shell:open-hud'),
  closeHud: (): Promise<void> => ipcRenderer.invoke('shell:close-hud'),
  toggleHud: (): Promise<void> => ipcRenderer.invoke('shell:toggle-hud'),
  moveHudBy: (delta: { x: number; y: number; width: number; height: number }): Promise<void> =>
    ipcRenderer.invoke('shell:move-hud', delta),
  setHudBounds: (bounds: { x?: number; y?: number; width?: number; height?: number }): Promise<void> =>
    ipcRenderer.invoke('shell:set-hud-bounds', bounds),
  // HUD mode: chrome-free floating composer. These mirror the Hermes Desktop
  // HUD bridge but are namespaced under `window.allternit.shell.hud` and use
  // `shell:*` / `shell:hud:*` IPC channels.
  hud: {
    open: (): Promise<void> => ipcRenderer.invoke('shell:open-hud'),
    close: (): Promise<void> => ipcRenderer.invoke('shell:close-hud'),
    toggle: (): Promise<void> => ipcRenderer.invoke('shell:toggle-hud'),
    setIgnoreMouse: (ignore: boolean): void => ipcRenderer.send('shell:hud:ignore-mouse', ignore),
    moveBy: (delta: { x: number; y: number; width: number; height: number }): Promise<void> =>
      ipcRenderer.invoke('shell:move-hud', delta),
    beginMove: (): Promise<void> => ipcRenderer.invoke('shell:hud:begin-move'),
    endMove: (): Promise<void> => ipcRenderer.invoke('shell:hud:end-move'),
    setBounds: (bounds: { x?: number; y?: number; width?: number; height?: number }): Promise<void> =>
      ipcRenderer.invoke('shell:set-hud-bounds', bounds),
    resetLayout: (): Promise<{ ok: boolean }> => ipcRenderer.invoke('shell:hud:reset-layout'),
    setFrost: (showing: boolean): Promise<{ ok: boolean }> => ipcRenderer.invoke('shell:hud:frost', showing),
    setWorkspaceTransfer: (transferring: boolean): void =>
      ipcRenderer.send('shell:hud:workspace-transfer', transferring),
    reportSession: (sessionId: string | null): void =>
      ipcRenderer.send('shell:hud:session', sessionId),
    onCursor: (callback: (point: { x: number; y: number } | null) => void): (() => void) => {
      const listener = (_: IpcRendererEvent, point: unknown) =>
        callback(point as { x: number; y: number } | null);
      ipcRenderer.on('shell:hud:cursor', listener);
      return () => ipcRenderer.removeListener('shell:hud:cursor', listener);
    },
    onGameOverlay: (callback: (state: unknown) => void): (() => void) => {
      const listener = (_: IpcRendererEvent, state: unknown) => callback(state);
      ipcRenderer.on('shell:hud:game-overlay', listener);
      return () => ipcRenderer.removeListener('shell:hud:game-overlay', listener);
    },
    onGoto: (callback: (sessionId: string) => void): (() => void) => {
      const listener = (_: IpcRendererEvent, sessionId: string) => callback(sessionId);
      ipcRenderer.on('shell:hud:goto', listener);
      return () => ipcRenderer.removeListener('shell:hud:goto', listener);
    },
    onChanged: (callback: (state: { open: boolean; sessionId: string | null }) => void): (() => void) => {
      const listener = (_: IpcRendererEvent, state: unknown) =>
        callback(state as { open: boolean; sessionId: string | null });
      ipcRenderer.on('shell:hud:state', listener);
      return () => ipcRenderer.removeListener('shell:hud:state', listener);
    },
    annotation: {
      open: (): Promise<void> => ipcRenderer.invoke('shell:hud:annotation:open'),
      close: (): Promise<void> => ipcRenderer.invoke('shell:hud:annotation:close'),
      clear: (): void => ipcRenderer.send('shell:hud:annotation:clear'),
      save: (base64Png: string): Promise<{ success: boolean; path?: string; error?: string }> =>
        ipcRenderer.invoke('shell:hud:annotation:save', base64Png),
      onClear: (callback: () => void): (() => void) => {
        const listener = (_: IpcRendererEvent) => callback();
        ipcRenderer.on('shell:hud:annotation:clear', listener);
        return () => ipcRenderer.removeListener('shell:hud:annotation:clear', listener);
      },
      onStateChange: (callback: (state: { open: boolean }) => void): (() => void) => {
        const listener = (_: IpcRendererEvent, state: unknown) => callback(state as { open: boolean });
        ipcRenderer.on('shell:hud:annotation:state', listener);
        return () => ipcRenderer.removeListener('shell:hud:annotation:state', listener);
      },
    },
    windowing: ipcRenderer.sendSync('shell:hud:windowing') as {
      clientPlacement: boolean;
      controlDrag: boolean;
      nativeDrag: boolean;
      workspaceTransfer: boolean;
    },
  },
  openDocs: (artifactId?: string): Promise<void> => ipcRenderer.invoke('shell:open-docs', artifactId),
  openOffice: (target?: string, artifactId?: string): Promise<void> =>
    ipcRenderer.invoke('shell:open-office', target, artifactId),
  openSession: (options: { sessionId: string; workspaceId?: string; title?: string }): Promise<void> =>
    ipcRenderer.invoke('shell:open-session', options),
  openBotComputer: (options: { botId: string; title?: string; sandboxId?: string }): Promise<void> =>
    ipcRenderer.invoke('shell:open-bot-computer', options),
  getOfficeHostStatus: (): Promise<Record<'word' | 'excel' | 'powerpoint', {
    installed: boolean;
    running: boolean;
    bundlePath: string | null;
  }>> => ipcRenderer.invoke('shell:get-office-host-status'),
  showSave: (options: unknown): Promise<unknown> => ipcRenderer.invoke('dialog:show-save', options),
  showOpen: (options: unknown): Promise<unknown> => ipcRenderer.invoke('dialog:show-open', options),
  moveHud: (delta: { dx: number; dy: number }): Promise<void> =>
    ipcRenderer.invoke('shell:move-hud', delta),
  resizeHud: (size: { width?: number; height: number }): Promise<void> =>
    ipcRenderer.invoke('shell:resize-hud', size),
  showHud: (): Promise<void> => ipcRenderer.invoke('shell:show-hud'),
};

// ─── Office programs ─────────────────────────────────────────────────────────
// File-association delivery: the main process sends `office:open-file` with
// { name, bytes } after the main window loads; the platform surface's
// office desktop bridge registers a handler here and routes the bytes via
// its file-handoff store. `office:open-target` is the sibling channel for
// target/artifact opens (app menu, shell:open-office IPC).

const officeAPI = {
  onOpenFile: (
    callback: (payload: { name: string; bytes: Uint8Array }) => void,
  ): (() => void) => {
    const listener = (_event: Electron.IpcRendererEvent, payload: { name: string; bytes: Uint8Array }) =>
      callback(payload);
    ipcRenderer.on('office:open-file', listener);
    return () => {
      ipcRenderer.removeListener('office:open-file', listener);
    };
  },
  onOpenTarget: (
    callback: (payload: { target: string; artifactId?: string | null }) => void,
  ): (() => void) => {
    const listener = (
      _event: Electron.IpcRendererEvent,
      payload: { target: string; artifactId?: string | null },
    ) => callback(payload);
    ipcRenderer.on('office:open-target', listener);
    return () => {
      ipcRenderer.removeListener('office:open-target', listener);
    };
  },
};

const officeAddinsAPI = {
  getStatus: () => ipcRenderer.invoke('office-addins:get-status'),
  install: (product: 'word' | 'excel' | 'powerpoint') => ipcRenderer.invoke('office-addins:install', product),
  repair: (product: 'word' | 'excel' | 'powerpoint') => ipcRenderer.invoke('office-addins:repair', product),
  remove: (product: 'word' | 'excel' | 'powerpoint') => ipcRenderer.invoke('office-addins:remove', product),
};

// ─── Theme ────────────────────────────────────────────────────────────────────

const themeAPI = {
  get: (): Promise<'light' | 'dark'> => ipcRenderer.invoke('theme:get'),
  set: (theme: 'light' | 'dark' | 'system'): Promise<void> =>
    ipcRenderer.invoke('theme:set', theme),
  onChanged: (handler: (dark: boolean) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, dark: boolean) => handler(dark);
    ipcRenderer.on('theme:updated', listener);
    return () => ipcRenderer.removeListener('theme:updated', listener);
  },
};

// ─── Extension Bridge ─────────────────────────────────────────────────────────
// Lets the platform renderer listen for Chrome extension messages relayed via
// the native messaging host and send responses back.

const extensionAPI = {
  getStatus: (): Promise<{ connected: boolean }> =>
    ipcRenderer.invoke('extension:get-status'),
  send: (message: unknown): Promise<boolean> =>
    ipcRenderer.invoke('extension:send', message),
  onMessage: (handler: (message: unknown) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, msg: unknown) => handler(msg);
    ipcRenderer.on('extension:message', listener);
    return () => ipcRenderer.removeListener('extension:message', listener);
  },
  onStatusChange: (handler: (status: { connected: boolean }) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, status: { connected: boolean }) => handler(status);
    ipcRenderer.on('extension:status', listener);
    return () => ipcRenderer.removeListener('extension:status', listener);
  },
};

// ─── Tunnel (Cloudflare Web Access) ──────────────────────────────────────────

type TunnelStatus = 'stopped' | 'starting' | 'running' | 'error';
type TunnelState = { status: TunnelStatus; url?: string; error?: string };

const tunnelAPI = {
  /** Start tunnel + open browser to /connect (full "Enable Web Access" flow). */
  enable: (): Promise<{ success: boolean; url?: string; error?: string }> =>
    ipcRenderer.invoke('tunnel:enable'),
  /** Start tunnel only — returns URL + token without opening a browser tab.
   *  Use this from within the in-app onboarding wizard. */
  start: (): Promise<{ success: boolean; url?: string; token?: string; error?: string }> =>
    ipcRenderer.invoke('tunnel:start'),
  disable: (): Promise<{ success: boolean }> =>
    ipcRenderer.invoke('tunnel:disable'),
  getUrl: (): Promise<string | null> =>
    ipcRenderer.invoke('tunnel:get-url'),
  getState: (): Promise<TunnelState> =>
    ipcRenderer.invoke('tunnel:get-status'),
  onStateChange: (handler: (state: TunnelState) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, state: TunnelState) => handler(state);
    ipcRenderer.on('tunnel:state', listener);
    return () => ipcRenderer.removeListener('tunnel:state', listener);
  },
};

// ─── Permission Guide ─────────────────────────────────────────────────────────

type PermissionPanel = 'accessibility' | 'screen-recording';
type PermissionStatus = {
  accessibility: 'granted' | 'denied' | 'unknown' | 'not-applicable';
  screenRecording: 'granted' | 'denied' | 'unknown' | 'not-applicable';
};

const permissionGuideAPI = {
  check: (): Promise<PermissionStatus> =>
    ipcRenderer.invoke('permission-guide:check'),
  requestCheck: (): Promise<PermissionStatus> =>
    ipcRenderer.invoke('permission-guide:request-check'),
  readyForCheck: (): Promise<PermissionStatus> =>
    ipcRenderer.invoke('permission-guide:ready-for-check'),
  present: (panel: PermissionPanel): Promise<{ success: boolean; alreadyGranted?: boolean; error?: string }> =>
    ipcRenderer.invoke('permission-guide:present', panel),
  dismiss: (): Promise<{ success: boolean; error?: string }> =>
    ipcRenderer.invoke('permission-guide:dismiss'),
  getStatus: (): Promise<{ active: boolean }> =>
    ipcRenderer.invoke('permission-guide:get-status'),
  getDriverStatus: (): Promise<{
    available: boolean; running: boolean; embedded: boolean; executable?: string; socket?: string; error?: string;
  }> => ipcRenderer.invoke('computer-use-driver:get-status'),
  onStatusChanged: (handler: (status: PermissionStatus) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, status: PermissionStatus) => handler(status);
    ipcRenderer.on('permission-guide:status', listener);
    return () => ipcRenderer.removeListener('permission-guide:status', listener);
  },
};

// ─── Phones (Lane 2a: Android over Wi-Fi) ─────────────────────────────────────

const phonesAPI = {
  status: (): Promise<unknown> => ipcRenderer.invoke('phone:status'),
  startQrPairing: (): Promise<{ payload: string }> => ipcRenderer.invoke('phone:pair-qr-start'),
  pairWithCode: (hostPort: string, code: string): Promise<unknown> => ipcRenderer.invoke('phone:pair-code', hostPort, code),
  cancelPairing: (): Promise<void> => ipcRenderer.invoke('phone:pair-cancel'),
  forget: (id: string): Promise<void> => ipcRenderer.invoke('phone:forget', id),
  startScrcpy: (serial: string): Promise<{ ok: boolean; error?: string }> => ipcRenderer.invoke('phone:scrcpy-start', serial),
  stopScrcpy: (serial: string): Promise<void> => ipcRenderer.invoke('phone:scrcpy-stop', serial),
  tryOpenApp: (serial: string, app: string): Promise<{ ok: boolean; message?: string }> => ipcRenderer.invoke('phone:try-open-app', serial, app),
  screenshot: (serial: string): Promise<{ ok: boolean; dataBase64?: string; message?: string }> => ipcRenderer.invoke('phone:screenshot', serial),
  installArtemis: (): Promise<unknown> => ipcRenderer.invoke('phone:artemis-install'),
  onDevices: (handler: (phones: unknown) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, phones: unknown) => handler(phones);
    ipcRenderer.on('phone:devices', listener);
    return () => ipcRenderer.removeListener('phone:devices', listener);
  },
  onPairing: (handler: (status: unknown) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, status: unknown) => handler(status);
    ipcRenderer.on('phone:pairing', listener);
    return () => ipcRenderer.removeListener('phone:pairing', listener);
  },
};

// ─── Feature Flags ────────────────────────────────────────────────────────────

const featureFlagsAPI = {
  get: (key?: string): Promise<unknown> => ipcRenderer.invoke('featureFlags:get', key),
  set: (key: string, value: unknown): Promise<boolean> => ipcRenderer.invoke('featureFlags:set', key, value),
  onChanged: (handler: (key: string, value: unknown) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, data: { key: string; value: unknown }) => handler(data.key, data.value);
    ipcRenderer.on('featureFlags:changed', listener);
    return () => ipcRenderer.removeListener('featureFlags:changed', listener);
  },
};

// ─── Sign in with ChatGPT (feature.siwc) ──────────────────────────────────────
// Status only — tokens live in main and never cross this bridge.

const siwcAPI = {
  status: (): Promise<unknown> => ipcRenderer.invoke('siwc:status'),
  signIn: (opts?: { enablePlanUsage?: boolean }): Promise<unknown> => ipcRenderer.invoke('siwc:signIn', opts),
  cancel: (): Promise<unknown> => ipcRenderer.invoke('siwc:cancel'),
  signOut: (): Promise<unknown> => ipcRenderer.invoke('siwc:signOut'),
  onStatusChanged: (handler: (status: unknown) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, status: unknown) => handler(status);
    ipcRenderer.on('siwc:status-changed', listener);
    return () => ipcRenderer.removeListener('siwc:status-changed', listener);
  },
};

// ─── Persisted State ──────────────────────────────────────────────────────────

const stateAPI = {
  get: (key: string): Promise<unknown> => ipcRenderer.invoke('state:get', key),
  set: (key: string, value: unknown): Promise<boolean> => ipcRenderer.invoke('state:set', key, value),
  patch: (key: string, partial: unknown): Promise<boolean> => ipcRenderer.invoke('state:patch', key, partial),
  onChanged: (handler: (key: string, value: unknown) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, data: { key: string; value: unknown }) => handler(data.key, data.value);
    ipcRenderer.on('state:changed', listener);
    return () => ipcRenderer.removeListener('state:changed', listener);
  },
};

// ─── Find in Page ─────────────────────────────────────────────────────────────

type FindInPageResult = { requestId: number; activeMatchOrdinal: number; matches: number; finalUpdate: boolean };

const findInPageAPI = {
  search: (text: string, options?: { forward?: boolean; matchCase?: boolean }): Promise<void> =>
    ipcRenderer.invoke('window:find-in-page', text, options),
  next: (): Promise<void> => ipcRenderer.invoke('window:find-in-page', undefined, { findNext: true }),
  previous: (): Promise<void> => ipcRenderer.invoke('window:find-in-page', undefined, { forward: false, findNext: true }),
  stop: (keepSelection?: boolean): Promise<void> => ipcRenderer.invoke('window:find-stop', keepSelection),
  onResult: (handler: (result: FindInPageResult) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, result: FindInPageResult) => handler(result);
    ipcRenderer.on('window:find-result', listener);
    return () => ipcRenderer.removeListener('window:find-result', listener);
  },
};

// ─── Locale / i18n ────────────────────────────────────────────────────────────

const localeAPI = {
  get: (): Promise<string> => ipcRenderer.invoke('locale:get'),
  set: (locale: string): Promise<boolean> => ipcRenderer.invoke('locale:set', locale),
  onChanged: (handler: (locale: string) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, locale: string) => handler(locale);
    ipcRenderer.on('locale:changed', listener);
    return () => ipcRenderer.removeListener('locale:changed', listener);
  },
};

// ─── Menu Bar + Startup on Login ──────────────────────────────────────────────

const menuBarAPI = {
  getMode: (): Promise<boolean> => ipcRenderer.invoke('menuBar:getMode'),
  setMode: (enabled: boolean): Promise<boolean> => ipcRenderer.invoke('menuBar:setMode', enabled),
  onModeChanged: (handler: (enabled: boolean) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, enabled: boolean) => handler(enabled);
    ipcRenderer.on('menuBar:modeChanged', listener);
    return () => ipcRenderer.removeListener('menuBar:modeChanged', listener);
  },
};

const startupAPI = {
  getOnLogin: (): Promise<boolean> => ipcRenderer.invoke('startup:getOnLogin'),
  setOnLogin: (enabled: boolean): Promise<boolean> => ipcRenderer.invoke('startup:setOnLogin', enabled),
};

// ─── MCP Host ────────────────────────────────────────────────────────────────

// ─── Research Backend ────────────────────────────────────────────────────────

const researchAPI = {
  getStatus: (): Promise<{ running: boolean; ready: boolean }> =>
    ipcRenderer.invoke('research:get-status'),
  start: (): Promise<boolean> => ipcRenderer.invoke('research:start'),
  stop: (): Promise<void> => ipcRenderer.invoke('research:stop'),
};

const mcpAPI = {
  listServers: (): Promise<unknown[]> => ipcRenderer.invoke('mcp:list-servers'),
  listTools: (serverId?: string): Promise<unknown[]> => ipcRenderer.invoke('mcp:list-tools', serverId),
  callTool: (serverId: string, toolName: string, args: unknown): Promise<{ success: boolean; result?: unknown; error?: string }> =>
    ipcRenderer.invoke('mcp:call-tool', serverId, toolName, args),
  addServer: (id: string, config: unknown): Promise<{ success: boolean; error?: string }> =>
    ipcRenderer.invoke('mcp:add-server', id, config),
  removeServer: (id: string): Promise<{ success: boolean; error?: string }> =>
    ipcRenderer.invoke('mcp:remove-server', id),
  onServerReady: (handler: (data: { serverId: string; tools: unknown[] }) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, data: unknown) => handler(data as { serverId: string; tools: unknown[] });
    ipcRenderer.on('mcp:server-ready', listener);
    return () => ipcRenderer.removeListener('mcp:server-ready', listener);
  },
  onServerError: (handler: (data: { serverId: string; error: string }) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, data: unknown) => handler(data as { serverId: string; error: string });
    ipcRenderer.on('mcp:server-error', listener);
    return () => ipcRenderer.removeListener('mcp:server-error', listener);
  },
};

// ─── Mini-apps (install / start / stop / status) ─────────────────────────────

type MiniAppInstallProgress = { id: string; line: string; type: 'stdout' | 'stderr' | 'info' };
type MiniAppInstallResult  = { success: boolean; error?: string };
type MiniAppStatus         = { managed: boolean; running: boolean; port: number | null };
type MiniAppRuntimeRegistration = { id: string; name: string; version?: string; installCommand?: string; startCommand?: string; stopCommand?: string; healthUrl?: string; permissions?: { network?: string[]; filesystem?: string[]; secrets?: string[]; processes?: boolean }; oauth?: Record<string, unknown> };
type MiniAppReleaseInstallOptions = { registryUrl: string; id: string; version?: string };
type MiniAppReleaseInstallResult = { success: boolean; error?: string; id: string; version?: string; previousVersion?: string; rolledBack?: boolean };
type MiniAppReleaseInstallInfo = { id: string; currentVersion?: string; previousVersion?: string; healthy: boolean; releases: Record<string, { installedAt: string; sha256: string; signature: string; publisherKey: string; healthy: boolean }> };
type MiniAppOAuthProvider = { authorizationUrl: string; tokenUrl: string; revocationUrl?: string; clientId: string; scopes: string[]; additionalAuthParams?: Record<string, string> };
type MiniAppOAuthAccountMetadata = { appId: string; providerId: string; accountId: string; scopes: string[]; expiresAt?: string; createdAt: string; lastRefreshedAt?: string; needsReauth: boolean };
type MiniAppOAuthFlowResult = { flowId: string; success: boolean; error?: string; scopes?: string[]; expiresAt?: string; appId: string; providerId: string; accountId: string };

const miniAppsAPI = {
  install: (id: string): Promise<MiniAppInstallResult> =>
    ipcRenderer.invoke('miniApps:install', id),
  start: (id: string): Promise<MiniAppInstallResult> =>
    ipcRenderer.invoke('miniApps:start', id),
  stop: (id: string): Promise<{ success: boolean }> =>
    ipcRenderer.invoke('miniApps:stop', id),
  getStatus: (id: string): Promise<MiniAppStatus> =>
    ipcRenderer.invoke('miniApps:getStatus', id),
  launchDesktop: (id: string): Promise<MiniAppInstallResult> =>
    ipcRenderer.invoke('miniApps:launchDesktop', id),
  getApproval: (id: string, registration?: MiniAppRuntimeRegistration): Promise<{ approved: boolean; fingerprint?: string; approvedAt?: string }> =>
    ipcRenderer.invoke('miniApps:getApproval', id, registration),
  reviewAndApprove: (registration: MiniAppRuntimeRegistration): Promise<{ success: boolean; approved: boolean; fingerprint?: string; error?: string }> =>
    ipcRenderer.invoke('miniApps:reviewAndApprove', registration),
  revokeApproval: (id: string): Promise<{ success: boolean }> =>
    ipcRenderer.invoke('miniApps:revokeApproval', id),
  setSecret: (id: string, name: string, value: string): Promise<{ success: boolean; error?: string }> =>
    ipcRenderer.invoke('miniApps:setSecret', id, name, value),
  listSecrets: (id: string): Promise<string[]> => ipcRenderer.invoke('miniApps:listSecrets', id),
  deleteSecret: (id: string, name: string): Promise<{ success: boolean }> => ipcRenderer.invoke('miniApps:deleteSecret', id, name),
  removeRuntime: (id: string): Promise<{ success: boolean; error?: string }> => ipcRenderer.invoke('miniApps:removeRuntime', id),
  rollbackRuntime: (id: string): Promise<{ success: boolean; error?: string }> => ipcRenderer.invoke('miniApps:rollbackRuntime', id),
  installRelease: (options: MiniAppReleaseInstallOptions): Promise<MiniAppReleaseInstallResult> =>
    ipcRenderer.invoke('miniApps:installRelease', options),
  rollbackRelease: (id: string, registryUrl?: string): Promise<{ success: boolean; error?: string; currentVersion?: string }> =>
    ipcRenderer.invoke('miniApps:rollbackRelease', id, registryUrl),
  removeRelease: (id: string, registryUrl?: string): Promise<{ success: boolean; error?: string }> =>
    ipcRenderer.invoke('miniApps:removeRelease', id, registryUrl),
  listReleaseInstalls: (): Promise<MiniAppReleaseInstallInfo[]> => ipcRenderer.invoke('miniApps:listReleaseInstalls'),
  getReleaseInstall: (id: string): Promise<MiniAppReleaseInstallInfo | null> => ipcRenderer.invoke('miniApps:getReleaseInstall', id),
  oauthStart: (appId: string, providerId: string, provider: MiniAppOAuthProvider, accountId: string): Promise<{ flowId?: string; error?: string }> =>
    ipcRenderer.invoke('miniApps:oauthStart', appId, providerId, provider, accountId),
  oauthCancel: (flowId: string): Promise<{ success: boolean }> => ipcRenderer.invoke('miniApps:oauthCancel', flowId),
  oauthAccounts: (appId: string): Promise<MiniAppOAuthAccountMetadata[]> => ipcRenderer.invoke('miniApps:oauthAccounts', appId),
  oauthDisconnect: (appId: string, providerId: string, accountId: string): Promise<{ success: boolean; error?: string }> =>
    ipcRenderer.invoke('miniApps:oauthDisconnect', appId, providerId, accountId),
  onOAuthComplete: (handler: (result: MiniAppOAuthFlowResult) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, result: MiniAppOAuthFlowResult) => handler(result);
    ipcRenderer.on('miniApps:oauth-complete', listener);
    return () => ipcRenderer.removeListener('miniApps:oauth-complete', listener);
  },
  onProgress: (handler: (p: MiniAppInstallProgress) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, p: MiniAppInstallProgress) => handler(p);
    ipcRenderer.on('miniApps:install-progress', listener);
    return () => ipcRenderer.removeListener('miniApps:install-progress', listener);
  },
};

export interface HermesRoutingExportResult {
  success: boolean;
  path: string;
  backupPath?: string;
  replaced: boolean;
  error?: string;
}

// ACI → Hermes export bridge: the renderer fetches the generated
// provider_routing YAML from the gateway admin API, the main process splices
// it into ~/.hermes/config.yaml.
const hermesRoutingAPI = {
  export: (routingYaml: string): Promise<HermesRoutingExportResult> =>
    ipcRenderer.invoke('hermesRouting:export', routingYaml),
};

// ─── Voice call-mode dictation ────────────────────────────────────────────────
// macOS-first native dictation bridge. When unavailable, the renderer falls
// back to the browser's Web Speech API through the existing useSTT() hook.

const voiceAPI = {
  isAvailable: (): Promise<boolean> => ipcRenderer.invoke('voice:is-available'),
  startDictation: (): Promise<{ success: boolean; error?: string }> =>
    ipcRenderer.invoke('voice:start-dictation'),
  stopDictation: (): Promise<void> => ipcRenderer.invoke('voice:stop-dictation'),
  transcribe: (wav: ArrayBuffer): Promise<{ text?: string; error?: string }> =>
    ipcRenderer.invoke('voice:transcribe', wav),
  onTranscript: (callback: (event: { text: string; isFinal: boolean }) => void): (() => void) => {
    const handler = (_: IpcRendererEvent, event: { text: string; isFinal: boolean }) => callback(event);
    ipcRenderer.on('voice:transcript', handler);
    return () => ipcRenderer.off('voice:transcript', handler);
  },
};

// ─── Worker Bus (renderer → main → worker round-trip) ────────────────────────

const workerAPI = {
  send: (workerName: string, message: unknown): Promise<{ success: boolean; result?: unknown; error?: string }> =>
    ipcRenderer.invoke('worker:send', workerName, message),
  list: (): Promise<string[]> => ipcRenderer.invoke('worker:list'),
};

// ─── HyperFrames ─────────────────────────────────────────────────────────────

const hyperframesAPI = {
  check: (): Promise<{ available: boolean; version?: string }> =>
    ipcRenderer.invoke('hyperframes:check'),
  render: (
    html: string,
    options?: { format?: 'mp4' | 'mov' | 'webm'; fps?: number; width?: number; height?: number }
  ): Promise<{ success: boolean; savedPath?: string; error?: string }> =>
    ipcRenderer.invoke('hyperframes:render', html, options ?? {}),
  onProgress: (handler: (message: string) => void): (() => void) => {
    const listener = (_: IpcRendererEvent, msg: string) => handler(msg);
    ipcRenderer.on('hyperframes:progress', listener);
    return () => ipcRenderer.removeListener('hyperframes:progress', listener);
  },
};

// ─── Artifact preview capture ────────────────────────────────────────────────
// A real render of a site an agent built, as a picture it can look at.

const artifactPreviewAPI = {
  capture: (req: { html: string; width?: number; height?: number; fullPage?: boolean }): Promise<{
    ok: boolean;
    image?: string;
    width?: number;
    height?: number;
    error?: string;
  }> => ipcRenderer.invoke('artifact-preview:capture', req),
  /** Serve a Build page at an allternit-preview:// URL with its own CSP (CDNs load). */
  host: (html: string): Promise<{ ok: true; url: string } | { ok: false; error: string }> =>
    ipcRenderer.invoke('artifact-preview:host', html),
};

// ─── Browser API Capture ─────────────────────────────────────────────────────
// Records network traffic from the ACI browser and returns a HAR archive for
// the platform's HAR-derived API service.

const browserCaptureAPI = {
  isAvailable: (): Promise<boolean> => ipcRenderer.invoke('browser-capture:is-available'),
  start: (options?: { filterUrls?: string[] }): Promise<{ success: boolean; sessionId?: string; error?: string }> =>
    ipcRenderer.invoke('browser-capture:start', options),
  stop: (sessionId: string): Promise<{ success: boolean; har?: string; error?: string }> =>
    ipcRenderer.invoke('browser-capture:stop', sessionId),
};

// ─── Expose ───────────────────────────────────────────────────────────────────

const allternitDesktopAPI = {
  sdk: sdkAPI,
  connection: connectionAPI,
  backend: backendAPI,
  engines: enginesAPI,
  bonsai: bonsaiAPI,
  systemOne: systemOneAPI,
  vm: vmAPI,
  window: windowAPI,
  store: storeAPI,
  state: stateAPI,
  app: appAPI,
  links: linksAPI,
  auth: authAPI,
  devicePairing: devicePairingAPI,
  mesh: meshAPI,
  shell: shellAPI,
  office: officeAPI,
  officeAddins: officeAddinsAPI,
  theme: themeAPI,
  extension: extensionAPI,
  tunnel: tunnelAPI,
  permissionGuide: permissionGuideAPI,
  phones: phonesAPI,
  featureFlags: featureFlagsAPI,
  siwc: siwcAPI,
  findInPage: findInPageAPI,
  locale: localeAPI,
  menuBar: menuBarAPI,
  startup: startupAPI,
  mcp: mcpAPI,
  research: researchAPI,
  worker: workerAPI,
  hyperframes: hyperframesAPI,
  miniApps: miniAppsAPI,
  hermesRouting: hermesRoutingAPI,
  browserCapture: browserCaptureAPI,
  artifactPreview: artifactPreviewAPI,
  voice: voiceAPI,
};

contextBridge.exposeInMainWorld('allternit', allternitDesktopAPI);

// Temporary diagnostic: log a stack trace for native window.close() calls
// (e.g. TerminalClerkPage.tsx's self-close), which bypass windowAPI.close above.
const nativeWindowClose = window.close.bind(window);
window.close = () => {
  console.warn('[preload] native window.close() invoked', new Error('trace').stack);
  nativeWindowClose();
};

// ─── allternitSidecar bridge ──────────────────────────────────────────────────
// The platform renderer calls window.allternitSidecar to detect Electron and
// discover the credential-brokering gizzi-code URL. This is the well-known
// interface defined in surfaces/ai.allternit.com/src/lib/globals.d.ts.
contextBridge.exposeInMainWorld('allternitSidecar', {
  getStatus: (): Promise<'stopped' | 'starting' | 'running' | 'error' | 'crashed'> =>
    ipcRenderer.invoke('sidecar:get-status'),
  getApiUrl: (): Promise<string | undefined> =>
    ipcRenderer.invoke('sidecar:get-api-url'),
});

declare global {
  interface Window {
    allternit: typeof allternitDesktopAPI;
    allternitSidecar: {
      getStatus: () => Promise<'stopped' | 'starting' | 'running' | 'error' | 'crashed'>;
      getApiUrl: () => Promise<string | undefined>;
    };
  }
}

console.log('[preload] Allternit Desktop preload loaded.');

// Small, purpose-specific bridge for the independent desktop companion windows.
contextBridge.exposeInMainWorld('allternitCompanion', {
  getState: () => ipcRenderer.invoke('companion:get'),
  update: (patch: { enabled?: boolean; size?: number; agentId?: string | null }) => ipcRenderer.invoke('companion:update', patch),
  resetPosition: () => ipcRenderer.invoke('companion:reset'),
  open: (panel: 'chat' | 'settings' = 'chat') => ipcRenderer.invoke('companion:open', panel),
  close: () => ipcRenderer.invoke('companion:close'),
  menu: () => ipcRenderer.invoke('companion:menu'),
  drag: (phase: 'start' | 'move' | 'end') => ipcRenderer.send('companion:drag', phase),
  setHeight: (height: number) => ipcRenderer.send('companion:height', height),
  ignoreMouse: (ignore: boolean) => ipcRenderer.send('companion:ignore-mouse', ignore),
  onState: (callback: (state: unknown) => void) => {
    const listener = (_event: IpcRendererEvent, state: unknown) => callback(state);
    ipcRenderer.on('companion:state', listener);
    return () => ipcRenderer.removeListener('companion:state', listener);
  },
});
