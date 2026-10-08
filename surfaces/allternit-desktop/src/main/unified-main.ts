/**
 * Allternit Desktop — Unified Main Process
 * 
 * Unified architecture: Desktop is the control plane.
 * - Manages local backend (bundled, auto-extracted)
 * - Connects to remote backend (VPS)
 * - Version-locked: Desktop 1.2.3 = Backend 1.2.3
 */

import { app, autoUpdater, BrowserWindow, ipcMain, nativeTheme, Notification, safeStorage, session, Tray, Menu, dialog, globalShortcut, screen, protocol, type WebContents } from 'electron';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve, basename } from 'node:path';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as http from 'node:http';
import * as https from 'node:https';
import * as os from 'node:os';
import { execFile } from 'node:child_process';
import Store from 'electron-store';
import log from 'electron-log';
import { updateElectronApp } from 'update-electron-app';
import fixPath from 'fix-path';
import { installDesktopCompanion } from './desktop-companion.js';
import { isPairingLink, pairingCodeFromUrl } from './pairing-link.js';
import { approvePairing, describePairingRequest } from './pairing-approval.js';
import { backendManager } from './backend-manager.js';
import { applyDesktopHumanProof, applyDesktopHumanProofTo, DESKTOP_PROOF_MARKER, HUMAN_PROOF_HEADER, takeDesktopProofParam } from './human-proof.js';
import { officeEngineManager } from './office-engine-manager.js';
import { factoryEngineManager, bundledGizziBinary } from './factory-engine-manager.js';
import { fabricWorkerManager, type FabricWorkerState } from './fabric-worker-manager.js';
import { readSecret, writeSecret, deleteSecret, FABRIC_WORKER_TOKEN_KEY } from './secure-store.js';
import { createSiwcManager, SIWC_FLAG, type SiwcManager } from './siwc.js';
import { startSiwcBroker, type SiwcBroker } from './siwc-broker.js';
import { localEngineManager } from './local-engine-manager.js';
import {
  editorForFile,
  extractOfficeFileArg,
  isOfficeTarget,
  type OfficeTarget,
} from './office-programs.js';
import {
  buildBotComputerWindowUrl,
  isBotComputerWindowUrl,
  isBotPhoneWindowUrl,
  buildBotPhoneWindowUrl,
  BOT_PHONE_WINDOW_SIZE,
} from './bot-computer-window.js';
import { bonsaiCompanion } from './bonsai-companion-manager.js';
import { systemOne } from './system-one-manager.js';
import { gizziManager } from './gizzi-manager.js';
import { connectorSidecarManager } from './connector-sidecar-manager.js';
import { gizziDaemonManager } from './gizzi-daemon-manager.js';
import { PORTS, URLS, devUiUrl, apiUrl, notebookUrl, staticUiUrl } from './config.js';
import { acceptsDeviceToken, isPublicCloudCatalogPath, rewriteCloudApiToProtocol, shouldInjectDesktopIdentity } from './api-protocol.js';
import { installMiniApp, startMiniApp, stopMiniApp, getMiniAppStatus, launchMiniAppDesktop, getMiniAppApproval, reviewAndApproveMiniApp, revokeMiniAppApproval, removeMiniAppRuntime, rollbackMiniAppRuntime, setMiniAppOAuthTokenResolver } from './mini-apps-manager.js';
import { installReleaseFromRegistry, rollbackReleaseInstall, removeReleaseInstall, listReleaseInstalls, getReleaseInstallState } from './mini-app-release-installer.js';
import { createMiniAppOAuthBroker, type MiniAppOAuthBroker, type MiniAppOAuthProvider } from './mini-app-oauth-broker.js';
import { setMiniAppSecret, listMiniAppSecrets, deleteMiniAppSecret } from './mini-app-secrets.js';
import { exportHermesRouting } from './hermes-routing-bridge.js';
import { OfficeAddinManager, type OfficeProductId } from './office-addin-manager.js';

import { tunnelManager } from './tunnel-manager.js';
import { authManager, FULL_CLERK_REFRESH_WAIT_MS } from './auth-manager.js';
// Side-effect import: constructing the manager registers the device-pairing:*
// IPC handlers. A named import that is never read gets elided by tsc, which
// left Settings → Devices failing with "No handler registered".
import './device-pairing-manager.js';
import { meshManager } from './mesh-manager.js';
import { startMeshBridgeServer } from './mesh-bridge-server.js';
import { createStartupWindow } from './startup-window.js';
import { notebookManager } from './notebook-manager.js';
import { voiceManager } from './voice-manager.js';
import { PLATFORM_MANIFEST, shouldUpdateBackend } from './manifest.js';
import { runtimePackages, runtimeResource } from './runtime-home.js';
import { DEFAULT_RUNTIME_FEED } from './runtime-package.js';
import {
  checkPermissions,
  presentGuide,
  dismissGuide,
  getGuideStatus,
  waitForGuideDismissed,
  runPermissionOnboarding,
  invalidatePermissionCache,
} from './permission-guide.js';
import { featureFlagManager } from './feature-flags.js';
import { persistedState } from './persisted-state.js';
import { workerBus } from './workers/worker-bus.js';
import { mcpHostManager } from './mcp-host-manager.js';
import { isLimaInstalled, installLima, startVM, stopVM, getVMStatus } from './lima.js';
import { computerUseDriverManager } from './computer-use-driver-manager.js';
import { acuGatewayManager } from './acu-gateway-manager.js';
import { phoneRemoteManager } from './phone-remote-manager.js';
import { phoneService } from './phone-service.js';
import { registerPhoneIpc } from './phone-ipc.js';
import {
  createCaptureSession,
  stopCaptureSession,
  isCaptureAvailable,
} from './browser-capture-manager.js';
import { captureArtifactPreview, type CaptureWindow, type PreviewCaptureRequest } from './artifact-preview-capture.js';
import { hostPreview, PREVIEW_SCHEME, servePreview } from './artifact-preview-host.js';
import {
  configureSecurity,
  installSessionSecurityHandlers,
  installWillNavigateGuard,
  installWindowOpenGuard,
  openExternalAllowlisted,
  assertTrustedSender,
} from './security.js';
import { resolveDevUserDataPath } from './desktop-data-dir.js';
import { coworkDeviceHeaders, coworkDeviceInfo, isInAppBrowsableUrl } from './cowork-device.js';
import { isProvisionedMode } from './provisioned-bootstrap.js';

// Fix PATH for macOS
fixPath();

// Unpackaged launches must not share the packaged app's Electron profile
// (`<appData>/@allternit/desktop`) — that SQLite is what worktree
// migrations kept rewriting. Not a second product: explicit scratch or
// an ephemeral temp dir. Must run before any getPath('userData').
{
  const decision = resolveDevUserDataPath({
    isPackaged: app.isPackaged,
    envUserData: process.env.ALLTERNIT_USER_DATA_DIR,
    argv: process.argv,
  });
  if (decision.kind === 'path') {
    app.setPath('userData', decision.path);
  } else if (decision.kind === 'ephemeral') {
    app.setPath('userData', fs.mkdtempSync(join(os.tmpdir(), 'allternit-desktop-')));
  }
}

// ============================================================================
// Connectivity helpers
// ============================================================================

function isUrlReachable(url: string, timeoutMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    const parsed = new URL(url);
    const client = parsed.protocol === 'https:' ? https : http;
    const req = client.get(url, { timeout: timeoutMs }, (res: any) => {
      // 2xx/3xx mean something answered at this URL. 4xx (404/401) means the
      // server is up but the target is wrong — treat as unreachable so callers
      // fall back instead of reporting a dead backend as healthy.
      resolve(res.statusCode >= 200 && res.statusCode < 400);
      res.destroy();
    });
    req.on('error', () => resolve(false));
    req.on('timeout', () => { req.destroy(); resolve(false); });
    req.setTimeout(timeoutMs);
  });
}

/**
 * Reads resources/company.json's `selfHosted` flag. Self-hosted builds ship
 * with no Clerk credentials and are not meant to pair with Allternit Cloud at
 * all — the startup gate must skip runtime pairing entirely for them rather
 * than opening a browser to a cloud sign-in flow that self-hosted operators
 * never configured. Defaults to false (normal cloud-paired behavior) if the
 * file is missing or unreadable, matching every build before this flag existed.
 */
/**
 * Headless computers (the hosted-driver image sets ALLTERNIT_HEADLESS=1) run
 * the runtime on a screen agents drive, so the app must never put its own
 * windows (welcome, sign-in, platform) on that screen: they'd be the first
 * thing a developer's screenshot shows.
 */
const HEADLESS = process.env.ALLTERNIT_HEADLESS === '1';

/** Keep a window off screen for good on headless computers. */
function keepHiddenIfHeadless<T extends BrowserWindow>(win: T): T {
  if (!HEADLESS) return win;
  win.hide();
  win.show = () => {};
  win.showInactive = () => {};
  win.focus = () => {};
  return win;
}

function isSelfHostedBuild(): boolean {
  const repoRoot = resolve(__dirname, '..', '..', '..', '..');
  const companyConfigPath = app.isPackaged
    ? join(process.resourcesPath ?? '', 'company.json')
    : join(repoRoot, 'resources', 'company.json');
  try {
    const raw = fs.readFileSync(companyConfigPath, 'utf8');
    return JSON.parse(raw)?.selfHosted === true;
  } catch {
    return false;
  }
}

/** Locate a locally-built platform static export to use in bundled mode. */
function resolveLocalPlatformStaticPath(): string | null {
  // __dirname is dist/main; go up four levels to reach the repo root.
  const repoRoot = resolve(__dirname, '..', '..', '..', '..');
  // Unpackaged runs prefer resources/platform — the same output
  // prepare-platform-static.cjs builds for packaged apps, with
  // NEXT_PUBLIC_ALLTERNIT_DESKTOP_AUTH baked in so the desktop shell bypasses
  // Clerk instead of hitting its production-domain origin lock. The raw
  // ai.allternit.com/dist candidates are a last-resort fallback only:
  // whatever's sitting there could have been built by a plain `vite build`
  // with no desktop flags at all, which is exactly what broke this before.
  const candidates = app.isPackaged
    ? [runtimeResource('platform')]
    : [
        join(repoRoot, 'surfaces', 'allternit-desktop', 'resources', 'platform'),
        join(repoRoot, 'surfaces', 'ai.allternit.com', 'dist'),
        join(repoRoot, 'surfaces', 'ai.allternit.com', 'out'),
        join(repoRoot, 'surfaces', 'platform', 'dist'),
        join(repoRoot, 'surfaces', 'platform', 'out'),
      ];
  for (const candidate of candidates) {
    if (fs.existsSync(join(candidate, 'index.html'))) {
      return candidate;
    }
  }
  return null;
}

// Configure logging
log.transports.file.resolvePath = () => join(app.getPath('userData'), 'main.log');
log.initialize();
log.transports.file.level = 'info';
if (!app.isPackaged) {
  log.info(
    `[Main] unpackaged userData=${app.getPath('userData')} (scratch, not a product profile; packaged sqlite stays at @allternit/desktop)`,
  );
}

// Last-resort crash handlers: log everything, keep the process alive where
// Electron allows it, and surface a fatal dialog only once the app is ready.
process.on('uncaughtException', (error) => {
  log.error('[Main] Uncaught exception:', error);
  if (app.isReady() && !(app as unknown as { isQuitting?: boolean }).isQuitting) {
    dialog.showErrorBox('Allternit Desktop encountered an error', String(error?.stack ?? error));
  }
});
process.on('unhandledRejection', (reason) => {
  log.error('[Main] Unhandled rejection:', reason);
});
const goneWebContents = new WeakSet<WebContents>();
app.on('render-process-gone', (_event, webContents, details) => {
  log.error('[Main] Renderer process gone:', details.reason, details.exitCode);
  if (details.reason === 'clean-exit' || details.reason === 'killed') return;
  if (goneWebContents.has(webContents) || webContents.isDestroyed()) return;
  goneWebContents.add(webContents);
  try {
    webContents.reload();
  } catch (error) {
    log.error('[Main] Failed to reload crashed renderer:', error);
  }
});

// Auto-updater
// Defaults read package.json repository, but we make the feed explicit so
// local/self-hosted builds never accidentally phone home to the wrong repo.
updateElectronApp({
  repo: 'Allternit/desktop',
  updateInterval: '1 hour',
  logger: log,
  notifyUser: false, // renderer will observe app:update-status and prompt
});

// Forward updater events to any renderer window so the platform UI can show
// status and a restart prompt. These are Electron's built-in autoUpdater
// events (the same channel used by update-electron-app); they carry less
// detail than electron-updater's events, but they are sufficient for status
// UI.
autoUpdater.on('checking-for-update', () => {
  broadcastUpdateStatus({ state: 'checking' });
});
autoUpdater.on('update-available', () => {
  broadcastUpdateStatus({ state: 'available' });
});
autoUpdater.on('update-not-available', () => {
  broadcastUpdateStatus({ state: 'up-to-date' });
});
autoUpdater.on('update-downloaded', (_event, releaseNotes, releaseName, _releaseDate, updateURL) => {
  appUpdateDownloaded = true;
  broadcastUpdateStatus({
    state: 'downloaded',
    version: releaseName,
    releaseNotes: typeof releaseNotes === 'string' ? releaseNotes : undefined,
    updateURL,
  });
});
autoUpdater.on('error', (error) => {
  log.error('[autoUpdater]', error);
  broadcastUpdateStatus({ state: 'error', message: error?.message ?? String(error) });
});

let appUpdateDownloaded = false;
let runtimeUpdateTimer: NodeJS.Timeout | null = null;

/**
 * Runtime packages (allternit-api, gizzi-code, screens) update in the
 * background from the signed runtime feed; a staged one applies on restart.
 * ALLTERNIT_RUNTIME_FEED overrides the feed; "off" disables it.
 */
function scheduleRuntimeUpdates() {
  const feed = process.env.ALLTERNIT_RUNTIME_FEED?.trim() || DEFAULT_RUNTIME_FEED;
  if (!app.isPackaged || feed === 'off' || runtimeUpdateTimer) return;
  const check = async () => {
    try {
      const staged = await runtimePackages().checkForUpdate(feed);
      if (staged) broadcastUpdateStatus({ state: 'downloaded', version: `runtime ${staged}` });
    } catch (err) {
      log.warn('[Runtime] update check failed:', (err as Error).message ?? err);
    }
  };
  runtimeUpdateTimer = setTimeout(function tick() {
    void check();
    runtimeUpdateTimer = setTimeout(tick, 6 * 60 * 60 * 1000);
  }, 60 * 1000);
}

function broadcastUpdateStatus(status: UpdateStatus) {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) {
      win.webContents.send('app:update-status', status);
    }
  }
}

type UpdateStatus =
  | { state: 'checking' }
  | { state: 'available' }
  | { state: 'up-to-date' }
  | { state: 'downloaded'; version?: string; releaseNotes?: string; updateURL?: string }
  | { state: 'error'; message: string };

const __dirname = dirname(fileURLToPath(import.meta.url));
const isDev = process.env.NODE_ENV === 'development';
const isMac = process.platform === 'darwin';

// ============================================================================
// State
// ============================================================================

let mainWindow: BrowserWindow | null = null;
let designWindow: BrowserWindow | null = null;
let officeWindow: BrowserWindow | null = null;
let hudWindow: BrowserWindow | null = null;
let annotationWindow: BrowserWindow | null = null;
let remoteControlWindow: BrowserWindow | null = null;
/** Active session id reported by the HUD renderer for app-window handoff. */
let hudSessionId: string | null = null;
/**
 * Office opens are delivered to the main window's renderer, not to separate
 * office windows: the single office surface is the shell's ACI "Office &
 * Extensions" hub (in-shell editor views). Deliveries that arrive before the
 * main window has finished loading queue here and flush on did-finish-load.
 */
const pendingOfficeDeliveries: { channel: string; payload: unknown }[] = [];
let splashWindow: BrowserWindow | null = null;

// Send to the startup window only while it is alive. A destroyed BrowserWindow
// is not null — accessing .webContents throws "Object has been destroyed" —
// and an unguarded throw inside startup init (or its error path) used to skip
// the error dialog + quit, leaving a windowless zombie app ("app not
// rendering"). Every splash send goes through here.
function sendToSplash(channel: string, ...args: unknown[]): void {
  if (splashWindow && !splashWindow.isDestroyed()) {
    splashWindow.webContents.send(channel, ...args);
  }
}

// Service state for splash screen progress (module-level so IPC handlers can update it)
let serviceState = {
  api: { status: 'pending', detail: 'Starting…' },
  gateway: { status: 'pending', detail: 'Starting…' },
  gizzi: { status: 'pending', detail: 'Starting…' },
  fabricWorker: { status: 'pending', detail: 'Waiting…' },
  office: { status: 'pending', detail: 'Waiting…' },
  connector: { status: 'pending', detail: 'Waiting…' },
  platform: { status: 'pending', detail: 'Waiting…' },
  research: { status: 'pending', detail: 'Waiting…' },
};
let pushServiceState = () => {
  sendToSplash('services', serviceState);
};
let miniWindow: BrowserWindow | null = null;
let tray: Tray | null = null;
let activePlatformUrl: string = isDev ? URLS.DEV_UI : URLS.PRODUCTION_UI;

// Central security policy for this process. App origins are the platform UI
// (remote or local static export), the HUD/office/design/session windows that
// load from the platform origin, and in dev the Vite dev server.
configureSecurity({
  isDev,
  getAppOrigins: () => {
    const origins = [activePlatformUrl, staticUiUrl(), URLS.PRODUCTION_UI];
    if (isDev) {
      origins.push(URLS.DEV_UI, 'http://127.0.0.1:3014');
    }
    return origins;
  },
});

/**
 * Wrap an ipcMain.handle registration with a sender-trust check. Every
 * sensitive channel goes through this so a compromised/misbehaving renderer
 * (or any non-app frame) cannot invoke main-process powers. Read-only and
 * window-management channels stay unguarded for performance and compat.
 */
function handleGuarded(
  channel: string,
  fn: (event: Electron.IpcMainInvokeEvent, ...args: any[]) => unknown,
): void {
  ipcMain.handle(channel, (event, ...args) => {
    assertTrustedSender(event, channel);
    return fn(event, ...args);
  });
}

const QUICK_CHAT_HOTKEY = 'CommandOrControl+Shift+A';
// Hermes Desktop uses ⌘/Ctrl+Shift+H for its global HUD toggle.  Register that
// as the primary shortcut and keep Alt+Shift+H as a fallback for users whose
// muscle memory expects it.
const HUD_HOTKEY = 'CommandOrControl+Shift+H';
const HUD_HOTKEY_FALLBACK = 'Alt+Shift+H';
const HUD_DEFAULT_WIDTH = 720;
const HUD_DEFAULT_HEIGHT = 72;
const MINI_WINDOW_WIDTH = 520;
const MINI_WINDOW_HEIGHT = 600;
/** Resolved backend URL — set once the app initializes. Used by sdk:get-backend-url IPC. */
let activeBackendUrl: string = URLS.API;
/** Active TCP connection from the native messaging host (Chrome extension bridge) */
const extensionSockets = new Set<net.Socket>();

function getConnectedExtensionSockets(): net.Socket[] {
  return [...extensionSockets].filter((socket) => !socket.destroyed);
}

function sendToExtension(message: unknown): boolean {
  const sockets = getConnectedExtensionSockets();
  const line = JSON.stringify(message) + '\n';
  for (const socket of sockets) socket.write(line);
  return sockets.length > 0;
}
function updateSidecarConfig(gizziUrl: string) {
  const config = {
    apiUrl: gizziUrl,
    port: new URL(gizziUrl).port ? Number(new URL(gizziUrl).port) : PORTS.GIZZI,
  };
  persistedState.set('sidecar', config);
}

async function startGizziRuntime(): Promise<string> {
  const daemonStatus = await gizziDaemonManager.getStatus();
  let existingPassword = daemonStatus.installed && daemonStatus.running
    ? await gizziDaemonManager.getConnectionPassword()
    : null;
  // In development, allow adopting an existing password-protected runtime
  // launched outside the desktop shell (e.g. a stale binary with a known dev
  // password). The password never leaves the main process.
  if (!existingPassword && process.env.GIZZI_SERVER_PASSWORD) {
    existingPassword = process.env.GIZZI_SERVER_PASSWORD;
  }
  if (existingPassword) {
    // One-time migration from the former persistent local Basic credential.
    // If the service cannot be rewritten, adopt it for this launch so existing
    // users are not stranded; the password never reaches renderer code.
    try {
      log.info('[GizziManager] Migrating legacy daemon to loopback runtime identity');
      await gizziDaemonManager.install(null, activeBackendUrl);
      existingPassword = null;
    } catch (error) {
      log.warn('[GizziManager] Legacy daemon migration deferred:', error);
    }
  }
  const session = authManager.getSessionSnapshot();
  return gizziManager.start({
    existingPassword,
    apiToken: session?.accessToken,
    runtimeId: session?.runtimeId,
    extraEnv: {
      ...authManager.getConnectorSidecarEnvironment(),
      ...(await siwcLaunchEnvironment()),
    },
  });
}

async function installAlwaysOnGizziRuntime(): Promise<void> {
  // Hand port 4096 from the per-session child to launchd before installing.
  // The daemon remains loopback-only and uses the same credential-free local
  // boundary as the managed runtime.
  gizziManager.stop();
  try {
    await gizziDaemonManager.install(null, activeBackendUrl);
    const url = await startGizziRuntime();
    updateSidecarConfig(url);
  } catch (error) {
    // Best-effort recovery: restore a usable runtime if daemon installation
    // failed after the per-session process was stopped.
    await startGizziRuntime().catch((restartError) => {
      log.error('[GizziManager] Failed to restore runtime after daemon install error:', restartError);
    });
    throw error;
  }
}

/**
 * Managed Fabric Transport worker (consumer-packaged Cowork P1). One local
 * ensure call per launch rotates the gizzi principal credential (the API
 * returns it once; previous tokens die), the Keychain-backed secure store
 * holds it, and the bundled gizzi-code binary claims work as
 * `gizzi-code fabric-worker`. The worker manager handles crash-respawn and
 * graceful quit; failures surface in the engine status, never silently.
 */
async function startManagedFabricWorker(): Promise<void> {
  const headers = backendManager.getLocalAuthHeaders();
  const ensureRes = await fetch(`${URLS.API}/api/v1/fabric/transport/local/ensure-worker-principal`, {
    method: 'POST',
    headers: { ...headers, 'content-type': 'application/json' },
    body: JSON.stringify({ workspace: 'default' }),
  });
  if (!ensureRes.ok) {
    const body = await ensureRes.text().catch(() => '');
    throw new Error(`worker credential provision failed: ${ensureRes.status} ${body.slice(0, 200)}`);
  }
  const ensured = (await ensureRes.json()) as { principal_id: string; token: string };
  writeSecret(FABRIC_WORKER_TOKEN_KEY, ensured.token);
  log.info(`[Main] Fabric worker credential provisioned for ${ensured.principal_id} (stored in Keychain)`);
  // P2.2/2.3: the worker's agentic jobs use the existing model router (the
  // api's operator key) and confine file tools to the granted folders.
  const prefsRes = await fetch(`${URLS.API}/api/v1/cowork-preferences`, { headers });
  const trustedFolders: string[] = prefsRes.ok
    ? (((await prefsRes.json().catch(() => ({}))) as { trusted_folders?: string[] }).trusted_folders ?? [])
    : [];
  const state = await fabricWorkerManager.start({
    token: ensured.token,
    apiUrl: URLS.API,
    operatorKey: backendManager.getApiKey(),
    trustedFolders,
  });
  serviceState.fabricWorker = { status: state.status === 'up' ? 'up' : state.status, detail: state.detail };
  pushServiceState();
}

/** Aggregate engine status for the shell indicator (API / gizzi / worker / office). */
async function getEngineStatus() {
  const worker = fabricWorkerManager.getStatus();
  const office = await officeEngineManager.getStatus().catch(() => null);
  return {
    api: serviceState.api,
    gizzi: serviceState.gizzi,
    fabricWorker: { status: worker.status, detail: worker.detail },
    office: office
      ? office.running
        ? { status: 'up', detail: `Connected on ${office.url}` }
        : { status: 'down', detail: 'Unavailable — restart the app' }
      : serviceState.office,
  };
}

async function pushEngineStatus(): Promise<void> {
  const status = await getEngineStatus();
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send('engines:status', status);
  }
}

/** If set, the permission onboarding flow should start when the renderer signals readiness. */
let permissionOnboardingResolver: (() => void) | null = null;

/**
 * Folder-grant onboarding step (consumer-packaged Cowork P1). When the
 * profile has no trusted_folders yet, the startup window shows a grant step
 * (Electron directory picker → /cowork-preferences) and boot waits for the
 * save. The desktop-access local auth gate is used — no Clerk session
 * exists at this point in first-run onboarding.
 */
let folderGrantResolver: (() => void) | null = null;
/** True while the splash folder-grant step is waiting on the user. */
let folderGrantsPending = false;

async function maybeRequestFolderGrants(): Promise<void> {
  if (!splashWindow || splashWindow.isDestroyed()) return;
  const headers = { ...backendManager.getLocalAuthHeaders(), 'content-type': 'application/json' };
  const res = await fetch(`${URLS.API}/api/v1/cowork-preferences`, { headers });
  if (!res.ok) throw new Error(`preferences read failed: ${res.status}`);
  const prefs = (await res.json()) as { trusted_folders?: string[] };
  if (Array.isArray(prefs.trusted_folders) && prefs.trusted_folders.length > 0) return;
  log.info('[Main] No trusted folders yet — showing grant step in startup window');
  folderGrantsPending = true;
  sendToSplash('folders:show');
  await new Promise<void>((resolve) => {
    folderGrantResolver = resolve;
  });
  folderGrantResolver = null;
  folderGrantsPending = false;
}

type OfficeHostId = 'word' | 'excel' | 'powerpoint';

type OfficeHostRuntimeStatus = {
  installed: boolean;
  running: boolean;
  bundlePath: string | null;
};

const OFFICE_HOSTS: Record<OfficeHostId, { appName: string; bundleId: string; windowsExe: string; commonPaths: string[] }> = {
  word: {
    appName: 'Microsoft Word',
    bundleId: 'com.microsoft.Word',
    windowsExe: 'WINWORD.EXE',
    commonPaths: [
      '/Applications/Microsoft Word.app',
      join(os.homedir(), 'Applications/Microsoft Word.app'),
    ],
  },
  excel: {
    appName: 'Microsoft Excel',
    bundleId: 'com.microsoft.Excel',
    windowsExe: 'EXCEL.EXE',
    commonPaths: [
      '/Applications/Microsoft Excel.app',
      join(os.homedir(), 'Applications/Microsoft Excel.app'),
    ],
  },
  powerpoint: {
    appName: 'Microsoft PowerPoint',
    bundleId: 'com.microsoft.Powerpoint',
    windowsExe: 'POWERPNT.EXE',
    commonPaths: [
      '/Applications/Microsoft PowerPoint.app',
      join(os.homedir(), 'Applications/Microsoft PowerPoint.app'),
    ],
  },
};

function execFileText(file: string, args: string[]): Promise<string> {
  return new Promise((resolve) => {
    execFile(file, args, { timeout: 2500 }, (error, stdout) => {
      if (error) {
        resolve('');
        return;
      }
      resolve(String(stdout ?? '').trim());
    });
  });
}

async function detectOfficeHostStatus(): Promise<Record<OfficeHostId, OfficeHostRuntimeStatus>> {
  const result = {} as Record<OfficeHostId, OfficeHostRuntimeStatus>;

  await Promise.all((Object.entries(OFFICE_HOSTS) as Array<[OfficeHostId, typeof OFFICE_HOSTS[OfficeHostId]]>).map(async ([host, meta]) => {
    if (process.platform === 'win32') {
      const programRoots = [process.env.ProgramFiles, process.env['ProgramFiles(x86)']].filter((value): value is string => Boolean(value));
      const candidates = programRoots.flatMap((root) => [
        join(root, 'Microsoft Office', 'root', 'Office16', meta.windowsExe),
        join(root, 'Microsoft Office', 'Office16', meta.windowsExe),
      ]);
      const bundlePath = candidates.find((candidate) => fs.existsSync(candidate)) ?? null;
      const tasks = await execFileText('tasklist.exe', ['/FI', `IMAGENAME eq ${meta.windowsExe}`, '/FO', 'CSV', '/NH']);
      result[host] = { installed: Boolean(bundlePath), running: tasks.toUpperCase().includes(meta.windowsExe), bundlePath };
      return;
    }
    if (process.platform !== 'darwin') {
      result[host] = { installed: false, running: false, bundlePath: null };
      return;
    }
    let bundlePath = meta.commonPaths.find((candidate) => fs.existsSync(candidate)) ?? null;

    if (!bundlePath) {
      const found = await execFileText('/usr/bin/mdfind', [`kMDItemCFBundleIdentifier == "${meta.bundleId}"`]);
      bundlePath = found.split('\n').map((line) => line.trim()).find(Boolean) ?? null;
    }

    const runningOutput = await execFileText('/usr/bin/pgrep', ['-x', meta.appName]);

    result[host] = {
      installed: Boolean(bundlePath),
      running: runningOutput.length > 0,
      bundlePath,
    };
  }));

  return result;
}

function resolveOfficeManifestDir(): string {
  const candidates = [
    process.env.ALLTERNIT_OFFICE_MANIFEST_DIR,
    join(app.getAppPath(), 'surfaces', 'allternit-extensions', 'allternit-office-addin', 'manifests'),
    join(process.resourcesPath, 'office-addins', 'manifests'),
    join(process.cwd(), 'surfaces', 'allternit-extensions', 'allternit-office-addin', 'manifests'),
  ].filter((value): value is string => Boolean(value));
  return candidates.find((candidate) => fs.existsSync(candidate)) ?? candidates[0];
}

async function getOfficeAddinManager(): Promise<OfficeAddinManager> {
  return new OfficeAddinManager({ manifestDir: resolveOfficeManifestDir(), hostStatus: await detectOfficeHostStatus() });
}

interface StoreSchema {
  windowBounds: { width: number; height: number; x?: number; y?: number };
  hudBounds: { width: number; height: number; x?: number; y?: number };
  theme: 'light' | 'dark' | 'system';
  backend: {
    mode: 'bundled' | 'remote' | 'development';
    remoteUrl?: string;
    lastLocalVersion?: string;
  };
  onboardingComplete: boolean;
  /** This computer's stable id in Settings → Cowork devices (cowork-device.ts). */
  coworkDeviceId?: string;
  /** Settings → Cowork "Open links in the built-in browser", mirrored from the API prefs. */
  openLinksInApp?: boolean;
  /** Whether the startup onboarding wizard (welcome → sign-in) has been completed or skipped */
  startupWizardCompleted: boolean;
  permissions: {
    /** Whether the user has been shown the permission guide during onboarding */
    promptedDuringOnboarding: boolean;
    /** Last known permission status (cached for quick checks) */
    lastStatus?: {
      accessibility: 'granted' | 'denied' | 'unknown' | 'not-applicable';
      screenRecording: 'granted' | 'denied' | 'unknown' | 'not-applicable';
      checkedAt: string;
    };
  };
}

const store = new Store<StoreSchema>({
  defaults: {
    windowBounds: { width: 1400, height: 900 },
    hudBounds: { width: HUD_DEFAULT_WIDTH, height: HUD_DEFAULT_HEIGHT },
    theme: 'system',
    backend: {
      // Dev builds should default to development mode so a fresh profile opens
      // the local platform without the bundled-mode onboarding wizard.
      mode: isDev ? 'development' : 'bundled',
    },
    onboardingComplete: false,
    startupWizardCompleted: false,
    permissions: {
      promptedDuringOnboarding: false,
    },
  },
});

const coworkDevice = coworkDeviceInfo(store);

let meshBridgeUrl: Promise<string | null> | null = null;
/** Start the local mesh bridge once; null if it couldn't start. */
function ensureMeshBridge(): Promise<string | null> {
  meshBridgeUrl ??= startMeshBridgeServer({
    secret: () => backendManager.getDesktopAccessToken(),
    loopbackFor: (target) => meshManager.loopbackFor(target),
  })
    .then(({ url }) => url)
    .catch((error) => {
      log.warn('[MeshBridge] could not start:', error);
      return null;
    });
  return meshBridgeUrl;
}
let openLinksInApp = store.get('openLinksInApp') ?? false;

/**
 * External link from an app window: with "Open links in the built-in
 * browser" on, the main window's renderer opens it as a browser tab;
 * otherwise (or with no main window to show it) it goes to the system browser.
 */
function openExternalLink(url: string): void {
  if (openLinksInApp && isInAppBrowsableUrl(url) && mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send('links:open-in-app', url);
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    return;
  }
  openExternalAllowlisted(url);
}

// ============================================================================
// Main Window
// ============================================================================

const desktopCompanion = installDesktopCompanion({
  origin: () => activePlatformUrl,
  main: () => mainWindow,
  preload: join(__dirname, '../preload/index.js'),
  openHud: (agentId) => openHudForCompanion(agentId),
  closeHud: () => hideHudWindow(),
  moveHudWithPet: (dx, dy) => {
    if (hudCompanionAgentId === undefined || !hudWindow || hudWindow.isDestroyed() || !hudWindow.isVisible()) return false;
    const bounds = hudWindow.getBounds();
    hudWindow.setPosition(bounds.x + dx, bounds.y + dy);
    return true;
  },
});

/**
 * Bring the main window up, recreating it if it was closed. With the desktop
 * companion (pet) window always open, "no windows" is never true after the
 * main window closes, so `mainWindow?.show()` alone left the app with no way
 * back from the Dock or tray.
 */
function showOrReopenMainWindow(): void {
  if (mainWindow && !mainWindow.isDestroyed()) {
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
    return;
  }
  log.info('[Main] Main window was closed; reopening');
  mainWindow = createMainWindow();
  mainWindow.loadURL(activePlatformUrl);
}

function createMainWindow(): BrowserWindow {
  appWindowOpened = true;
  let bounds = store.get('windowBounds');
  
  // Ensure bounds are valid - if not, use defaults
  if (!bounds || !bounds.width || !bounds.height || bounds.width < 100 || bounds.height < 100) {
    bounds = { width: 1400, height: 900 };
  }

  // Never let the minimum size exceed the screen: a cloud computer's display
  // is 1280x720, and 1366x768 laptops lose height to the taskbar. Larger
  // minimums pushed the bottom of the app (composer, wizard buttons) off-screen.
  const workArea = screen.getPrimaryDisplay().workAreaSize;
  const minWidth = Math.min(1024, workArea.width);
  const minHeight = Math.min(768, workArea.height);

  log.info(`[Main] Creating window with bounds:`, bounds);

  const window = new BrowserWindow({
    width: Math.min(bounds.width, workArea.width),
    height: Math.min(bounds.height, workArea.height),
    x: bounds.x,
    y: bounds.y,
    minWidth,
    minHeight,
    title: 'Allternit Desktop',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 16 },
    show: true, // Show immediately
    backgroundColor: '#0F0C0A', // Match the app's dark background
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      // Browser Mode uses Electron webviews for real page isolation. Guest
      // preferences are locked down again in will-attach-webview below.
      webviewTag: true,
      // Secure by default: API calls are routed through the allternit-api
      // custom protocol handler (registered in app.whenReady) which proxies
      // to the local API without mixed-content issues.
      allowRunningInsecureContent: false,
      // Agents keep working in the open artifact while the window is hidden
      // or minimized; throttled timers/frames there made every pane_artifact
      // call time out ("the editor isn't responding") until it was shown.
      backgroundThrottling: false,
    },
  });

  // A cloud computer's desktop exists only to show this app.
  if (isProvisionedMode()) {
    window.maximize();
  }

  installWillNavigateGuard(window.webContents);

  // Flush any office opens that were requested before the main window
  // finished loading (cold-start file associations, app-menu clicks).
  window.webContents.on('did-finish-load', () => {
    flushPendingOfficeDeliveries();
    desktopCompanion.start();
  });

  // Route operator + cloud-api calls through the allternit-api custom protocol.
  // Local `/api/*` gets the paired device token. Cloud host `cloud` is a
  // CORS-free broker that MUST keep the renderer's Clerk JWT — swapping in the
  // desktop device token is what produced the billing 401s.
  const platformOrigin = activePlatformUrl;
  const publicApiOrigin = URLS.CLOUD_API;
  window.webContents.session.webRequest.onBeforeRequest((details, callback) => {
    const cloudRedirect = rewriteCloudApiToProtocol(details.url, publicApiOrigin);
    if (cloudRedirect) {
      callback({ redirectURL: cloudRedirect });
      return;
    }
    if (details.url.startsWith(`${platformOrigin}/api/`)) {
      callback({
        redirectURL: details.url.replace(platformOrigin, `allternit-api://localhost:${PORTS.API}`),
      });
      return;
    }
    callback({});
  });

  window.webContents.session.webRequest.onBeforeSendHeaders((details, callback) => {
    const session = authManager.getSessionSnapshot();
    const injectDesktop = shouldInjectDesktopIdentity(details.url, URLS.API, URLS.CLOUD_API);
    if (session && injectDesktop) {
      details.requestHeaders.Authorization = `Bearer ${session.accessToken}`;
      details.requestHeaders['X-Allternit-Desktop-Access-Token'] = session.accessToken;
      details.requestHeaders['X-Allternit-User-Id'] = session.userId;
      details.requestHeaders['X-Allternit-User-Email'] = session.userEmail;
      if (session.organizationId) {
        details.requestHeaders['X-Allternit-Tenant-Id'] = session.organizationId;
      }
    }
    // D16: the window's own person-act marker becomes Desktop's proof, only
    // on requests to this machine's API; stripped everywhere else.
    applyDesktopHumanProof(details.requestHeaders, injectDesktop ? backendManager.getHumanProof() : null);
    if (injectDesktop) {
      Object.assign(details.requestHeaders, coworkDeviceHeaders(coworkDevice));
    }
    callback({ requestHeaders: details.requestHeaders });
  });



  const saveBounds = () => {
    if (window && !window.isDestroyed()) {
      store.set('windowBounds', window.getBounds());
    }
  };

  const emitWindowEvent = (event: string, data: unknown) => {
    if (window && !window.isDestroyed() && window.webContents) {
      window.webContents.send(`window:event:${event}`, data);
    }
  };

  window.on('resize', () => {
    saveBounds();
    emitWindowEvent('resize', window.getBounds());
  });
  window.on('move', () => {
    saveBounds();
    emitWindowEvent('move', window.getBounds());
  });
  window.on('focus', () => emitWindowEvent('focus', { focused: true }));
  window.on('blur', () => emitWindowEvent('blur', { focused: false }));
  window.on('maximize', () => emitWindowEvent('maximize', { maximized: true }));
  window.on('unmaximize', () => emitWindowEvent('unmaximize', { maximized: false }));
  window.on('minimize', () => emitWindowEvent('minimize', { minimized: true }));
  window.on('restore', () => emitWindowEvent('restore', { minimized: false }));
  window.on('enter-full-screen', () => emitWindowEvent('enter-full-screen', { fullscreen: true }));
  window.on('leave-full-screen', () => emitWindowEvent('leave-full-screen', { fullscreen: false }));
  window.on('closed', () => {
    emitWindowEvent('closed', {});
    mainWindow = null;
  });
  
  keepHiddenIfHeadless(window);
  // Ensure window is visible and focused
  window.once('ready-to-show', () => {
    log.info('[Main] ready-to-show event fired');
    window.show();
    window.focus();
  });

  window.webContents.setWindowOpenHandler(({ url }) => {
    try {
      const requestedUrl = new URL(url);
      const appUrl = new URL(window.webContents.getURL());

      if (requestedUrl.origin !== appUrl.origin) {
        openExternalLink(url);
        return { action: 'deny' };
      }

      if (requestedUrl.pathname === '/design') {
        return {
          action: 'allow',
          overrideBrowserWindowOptions: {
            width: 1440,
            height: 960,
            minWidth: 960,
            minHeight: 640,
            backgroundColor: '#0F0C0A',
            autoHideMenuBar: true,
            title: 'Allternit Design',
          },
        };
      }

      if (
        (requestedUrl.pathname === '/platform' || requestedUrl.pathname === '/shell') &&
        requestedUrl.searchParams.get('detachedSurface') === 'code'
      ) {
        return {
          action: 'allow',
          overrideBrowserWindowOptions: {
            width: 1180,
            height: 820,
            minWidth: 760,
            minHeight: 560,
            backgroundColor: '#0F0C0A',
            autoHideMenuBar: true,
            title: 'Allternit Code Session',
          },
        };
      }

      if (isBotPhoneWindowUrl(requestedUrl)) {
        return {
          action: 'allow',
          overrideBrowserWindowOptions: { ...BOT_PHONE_WINDOW_SIZE, backgroundColor: '#FFFFFF', autoHideMenuBar: true, title: 'Bot phone' },
        };
      }

      if (isBotComputerWindowUrl(requestedUrl)) {
        return {
          action: 'allow',
          overrideBrowserWindowOptions: {
            width: 1280,
            height: 840,
            minWidth: 820,
            minHeight: 560,
            backgroundColor: '#0F0C0A',
            autoHideMenuBar: true,
            title: requestedUrl.searchParams.get('title') || 'Bot computer',
          },
        };
      }
    } catch {
      // Invalid or non-standard URLs fall through to the external browser.
    }

    openExternalAllowlisted(url);
    return { action: 'deny' };
  });

  window.webContents.on('will-attach-webview', (event, webPreferences, params) => {
    let protocol: string;
    try {
      protocol = new URL(params.src).protocol;
    } catch {
      event.preventDefault();
      return;
    }
    if (!['http:', 'https:', 'about:'].includes(protocol)) {
      log.warn(`[Main] Blocked Browser Mode webview protocol: ${protocol}`);
      event.preventDefault();
      return;
    }
    delete webPreferences.preload;
    webPreferences.nodeIntegration = false;
    webPreferences.contextIsolation = true;
    webPreferences.sandbox = true;
    webPreferences.webSecurity = true;
    webPreferences.allowRunningInsecureContent = false;
  });

  window.webContents.on('did-attach-webview', (_event, guestContents) => {
    guestContents.setWindowOpenHandler(({ url }) => {
      try {
        const protocol = new URL(url).protocol;
        if (protocol === 'http:' || protocol === 'https:') {
          openExternalLink(url);
        }
      } catch {
        log.warn(`[Main] Ignored malformed Browser Mode popup URL: ${url}`);
      }
      return { action: 'deny' };
    });
  });

  // Auto-open DevTools only when flag is set
  if (featureFlagManager.get<boolean>('devtools.auto-open')) {
    window.webContents.openDevTools({ mode: 'detach' });
  }
  
  // Log console messages
  window.webContents.on('console-message', (event) => {
    log.info(`[Renderer] ${event.message} (${event.sourceId}:${event.lineNumber})`);
  });

  return window;
}

// ============================================================================
// App Initialization (Unified Flow)
// ============================================================================

// Initialize the app exactly once. `app.on('activate')` fires on every macOS
// launch (not just dock clicks) and can race `whenReady().then(initializeApp)`:
// if it lands before the startup window exists, window count is 0 and a second
// concurrent initializeApp ran — two initializeBundledMode passes fighting over
// the singleton BackendManager (spawn/kill/re-spawn ping-pong on :8013,
// destroyed startup window, "did not start within 30s" wedges). Guard all
// entry points through this single promise.
let initPromise: Promise<void> | null = null;
function initializeAppOnce(): Promise<void> {
  if (!initPromise) {
    initPromise = initializeApp()
      .catch((error) => {
        log.error('[Main] initializeApp failed:', error);
      })
      .finally(() => {
        // Reset once settled so a later dock-click with no windows can revive
        // the app (the original activate behavior) — only concurrent calls
        // during boot share the in-flight promise.
        initPromise = null;
      });
  }
  return initPromise;
}

async function initializeApp(): Promise<void> {
  log.info('[Main] Initializing Allternit Desktop v' + PLATFORM_MANIFEST.version);
  // Voice is an optional local capability: start it automatically, but never
  // prevent the rest of the desktop from opening if model initialization fails.
  if (!process.env.ALLTERNIT_DISABLE_VOICE) {
    void voiceManager.start().catch((error) => {
      log.warn('[Main] Voice service unavailable; continuing without Voice Mode:', error);
    });
  } else {
    log.info('[Main] Voice service disabled via ALLTERNIT_DISABLE_VOICE');
  }

  // Phones paired earlier reconnect in the background; nothing starts for users who never paired one.
  void phoneService.initIfPaired();

  // phone-remote server (Fabric desktop viewer proxies /frame + /hello to
  // 127.0.0.1:8477). Independent of auth — the viewer needs the server
  // regardless of sign-in state. Non-fatal: the app must still render if it
  // cannot start; a foreign :8477 listener is adopted, never killed.
  if (!process.env.ALLTERNIT_DISABLE_PHONE_REMOTE) {
    void phoneRemoteManager.ensureStarted().catch((error) => {
      log.warn('[Main] phone-remote unavailable; continuing without it:', error);
    });
  } else {
    log.info('[Main] phone-remote disabled via ALLTERNIT_DISABLE_PHONE_REMOTE');
  }
  
  const backendConfig = store.get('backend');
  
  // Local smoke-test convenience: force development mode to skip bundled backend boot.
  if (process.env.ALLTERNIT_FORCE_DEV_MODE === '1') {
    log.info('[Main] ALLTERNIT_FORCE_DEV_MODE=1 — forcing development mode');
    await initializeDevelopmentMode();
    return;
  }

  // Determine which mode to use
  const effectiveMode = backendConfig?.mode ?? (isDev ? 'development' : 'bundled');
  if (effectiveMode === 'development') {
    // Development mode - connect to the local Gizzi runtime
    await initializeDevelopmentMode();
  } else if (effectiveMode === 'remote' && backendConfig?.remoteUrl) {
    // Remote mode - connect to user VPS
    await initializeRemoteMode(backendConfig.remoteUrl);
  } else {
    // Bundled mode - manage local backend
    await initializeBundledMode();
  }
}

async function initializeBundledMode(): Promise<void> {
  log.info('[Main] Bundled mode - managing local backend');
  if (app.isPackaged) {
    // Pick the runtime package (allternit-api, gizzi-code, screens) before any of it starts.
    await runtimePackages().prepareForLaunch().catch((err) => log.warn('[Runtime] prepare failed, using bundled runtime:', err));
  }
  
  // Show startup window: full onboarding wizard on first launch / when signed
  // out, plain loading screen for returning signed-in users. Self-hosted
  // builds have no Clerk credentials and never pair with Allternit Cloud, so
  // they always skip straight to the loading screen and into the platform —
  // otherwise they'd sit at "Waiting for browser login..." forever.
  const selfHosted = isSelfHostedBuild();
  const showStartupWizard = !selfHosted && !HEADLESS && (!store.get('startupWizardCompleted') || !authManager.hasSession());
  log.info(`[Main] Startup window: ${showStartupWizard ? 'onboarding wizard' : 'loading only'}${selfHosted ? ' (self-hosted)' : ''}`);
  splashWindow = keepHiddenIfHeadless(createStartupWindow({ initialStep: showStartupWizard ? 'welcome' : 'loading' }));
  appWindowOpened = true;
  // Device pairing is independent of local service readiness. Start waiting
  // immediately so the user can approve in parallel while the runtime boots.
  const startupSignIn = showStartupWizard
    ? authManager.waitForStartupSignIn(splashWindow)
    : Promise.resolve(null);
  const updateSplash = (status: string, progress?: number) => {
    sendToSplash('status', status);
    if (progress !== undefined) {
      sendToSplash('progress', progress);
    }
  };
  // Reset service state at start of bundled mode initialization
  serviceState = {
    api: { status: 'pending', detail: 'Starting…' },
    gateway: { status: 'pending', detail: 'Starting…' },
    gizzi: { status: 'pending', detail: 'Starting…' },
    fabricWorker: { status: 'pending', detail: 'Waiting…' },
    office: { status: 'pending', detail: 'Waiting…' },
    connector: { status: 'pending', detail: 'Waiting…' },
    platform: { status: 'pending', detail: 'Waiting…' },
    research: { status: 'pending', detail: 'Waiting…' },
  };
  pushServiceState = () => {
    sendToSplash('services', serviceState);
  };
  pushServiceState();
  
  try {
    log.info('[Main] Starting initialization sequence...');
    // Step 1 — connector sidecar (open-connector, ephemeral loopback port).
    // allternit-api's connector routes proxy to this and gizzi-code's Lens
    // vault connectors call it directly — it must be up BEFORE both so
    // their env gets the real announced URL. Non-fatal if it fails
    // (connector-backed sources stay unavailable).
    {
      connectorSidecarManager.onDegraded(() => {
        serviceState.connector = { status: 'down', detail: 'Connector unavailable — restart the app' };
        pushServiceState();
      });
      const sidecarEnv = authManager.getConnectorSidecarEnvironment();
      try {
        const connectorUrl = await connectorSidecarManager.start({
          encryptionKey: authManager.getPlatformEncryptionEnvironment().ALLTERNIT_ENCRYPTION_KEY,
          adminToken: sidecarEnv.ALLTERNIT_CONNECTOR_SIDECAR_ADMIN_TOKEN,
          runtimeToken: sidecarEnv.ALLTERNIT_CONNECTOR_SIDECAR_RUNTIME_TOKEN,
        });
        log.info('[Main] Connector sidecar started successfully');
        serviceState.connector = { status: 'up', detail: `Connected on ${connectorUrl}` };
        pushServiceState();
      } catch (sidecarErr) {
        log.warn('[Main] Connector sidecar failed to start, continuing without it:', sidecarErr);
        serviceState.connector = { status: 'down', detail: 'Connector unavailable' };
        pushServiceState();
      }
    }

    // Step 1.5–1.7 — runtime sidecars, all CONCURRENTLY. The connector sidecar
    // above had to be first (API + gizzi env depend on its announced URL);
    // these five are independent of each other. Each task owns its
    // serviceState update, never throws, and resolves null when degraded —
    // the API below only needs gizziUrl, localEngineUrl, and the launch
    // environments from the driver/ACU managers.
    updateSplash('Starting services…', 10);
    // System One + Laya (Q28/Q29): S1 server now, Laya once installed (first run
    // installs it in the background). Never awaited: app start never waits on
    // it; failures land in system-one:get-status. Started this early so Laya is
    // ready for the first decisions (the API's "auto" S1 backend uses it once healthy).
    void systemOne.startWithApp();
    const gizziTask = (async (): Promise<string | null> => {
      try {
        const url = await startGizziRuntime();
        activeBackendUrl = url;
        updateSidecarConfig(url);
        log.info('[Main] Gizzi-code started successfully');
        serviceState.gizzi = { status: 'up', detail: `Connected on ${url}` };
        pushServiceState();
        void meshManager.start().catch((error) => {
          log.warn('[Mesh] Fabric mesh unavailable (relay still works):', error);
        });
        return url;
      } catch (gizziErr) {
        log.warn('[Main] Gizzi-code failed to start, continuing without AI runtime:', gizziErr);
        serviceState.gizzi = { status: 'down', detail: `Failed to start on ${PORTS.GIZZI}` };
        pushServiceState();
        return null;
      }
    })();
    const officeTask = (async (): Promise<void> => {
      try {
        const engineUrl = await officeEngineManager.start();
        if (engineUrl) {
          log.info(`[Main] Office engine ready (${officeEngineManager.getMode()}) at ${engineUrl}`);
          serviceState.office = { status: 'up', detail: `Connected on ${engineUrl}` };
        } else {
          log.warn('[Main] Office engine unavailable, continuing without it');
          serviceState.office = { status: 'down', detail: 'Unavailable — restart the app' };
        }
      } catch (officeErr) {
        log.warn('[Main] Office engine failed to start, continuing without it:', officeErr);
        serviceState.office = { status: 'down', detail: 'Unavailable — restart the app' };
      }
      pushServiceState();
    })();
    const driverTask = (async () => {
      try {
        // Spawn the embedded driver from the GUI app itself so macOS attributes
        // both privacy grants to Allternit, then give the backend only its socket.
        const driver = await computerUseDriverManager.start();
        if (!driver.running) {
          log.warn('[Main] Embedded computer-use driver unavailable:', driver.error);
        }
        return driver;
      } catch (driverErr) {
        log.warn('[Main] Computer-use driver failed to start, continuing without it:', driverErr);
        return null;
      }
    })();
    const acuTask = (async (): Promise<string | null> => {
      try {
        acuGatewayManager.setDesktopTokenProvider(() => backendManager.ensureDesktopAccessToken());
        const acuUrl = await acuGatewayManager.start();
        if (acuUrl) {
          log.info(`[Main] ACU computer-use gateway ready at ${acuUrl}`);
        } else {
          log.warn('[Main] ACU computer-use gateway unavailable; Open computer will 502 until it is started');
        }
        return acuUrl;
      } catch (acuErr) {
        log.warn('[Main] ACU gateway failed to start, continuing without it:', acuErr);
        return null;
      }
    })();
    // Local engine: allternit-api reads LOCAL_ENGINE_URL at request time (it
    // proxies /api/local-engine/* per request), but readiness is ~1s with the
    // ECONNREFUSED fast-path, so it still starts in the parallel group.
    const localEngineTask = (async (): Promise<string | null> => {
      try {
        const url = await localEngineManager.ensureStarted();
        log.info(`[Main] Local engine ready at ${url}`);
        return url;
      } catch (engineErr) {
        log.warn('[Main] Local engine failed to start, continuing without it:', engineErr);
        return null;
      }
    })();
    // Factory engine: allternit-api proxies /api/factory/* to it per request
    // (ALLTERNIT_FACTORY_URL), so it starts in the parallel group. A packaged
    // app also puts `gizzi` on PATH and removes the retired rails/ao tools.
    const factoryTask = (async (): Promise<void> => {
      try {
        const url = await factoryEngineManager.start();
        if (url) {
          log.info(`[Main] Factory engine ready (${factoryEngineManager.getMode()}) at ${url}`);
        } else {
          log.warn('[Main] Factory engine unavailable; /api/factory will answer 502 until it starts');
        }
        if (app.isPackaged) {
          factoryEngineManager.installCli(bundledGizziBinary(process.resourcesPath));
        }
      } catch (factoryErr) {
        log.warn('[Main] Factory engine failed to start, continuing without it:', factoryErr);
      }
    })();
    const [gizziUrl, , , , localEngineUrl] = await Promise.all([
      gizziTask,
      officeTask,
      driverTask,
      acuTask,
      localEngineTask,
      factoryTask,
    ]);

    // Step 2 — allternit-api (Rust operator API, port ${PORTS.API} — VM, rails, terminal)
    const apiStatus = await backendManager.getStatus();
    if (!apiStatus.installed) {
      updateSplash('Setting up Allternit Desktop for the first time…', 30);
    } else if (apiStatus.version && shouldUpdateBackend(apiStatus.version)) {
      updateSplash('Updating Allternit Desktop…', 30);
    } else {
      updateSplash('Starting operator backend…', 30);
    }

    // Remote computers on the mesh: the API reaches their VNC servers through
    // this loopback bridge (mesh-bridge-server.ts).
    const meshBridge = await ensureMeshBridge();

    try {
      await backendManager.ensureBackend({
        gizziUrl,
        gizziPassword: gizziManager.getPassword(),
        gizziUsername: 'gizzi',
        extraEnv: {
          ...(meshBridge ? { ALLTERNIT_MESH_BRIDGE_URL: meshBridge } : {}),
          ...(localEngineUrl ? { LOCAL_ENGINE_URL: localEngineUrl } : {}),
          ...computerUseDriverManager.getLaunchEnvironment(),
          ...acuGatewayManager.getLaunchEnvironment(),
          ...authManager.getPlatformEncryptionEnvironment(),
          ...authManager.getConnectorSidecarEnvironment(),
        },
      });
    } catch (apiErr) {
      if (runtimePackages().failTrial()) {
        // The just-updated runtime would not start: it is marked bad, so relaunch on the previous one.
        log.error('[Runtime] updated runtime failed to start; rolling back and relaunching:', apiErr);
        app.relaunch();
        app.exit(0);
        return;
      }
      const message = (apiErr as Error).message ?? String(apiErr);
      sendToSplash('error', message);
      serviceState.api = { status: 'down', detail: message.slice(0, 120) };
      serviceState.gateway = { status: 'down', detail: message.slice(0, 120) };
      pushServiceState();
      throw apiErr;
    }
    serviceState.api = { status: 'up', detail: `Connected on ${URLS.API}` };
    serviceState.gateway = { status: 'up', detail: `Connected on ${URLS.API}` };
    pushServiceState();
    runtimePackages().confirmHealthy();
    scheduleRuntimeUpdates();
    void registerThisComputer();
    store.set('backend.lastLocalVersion', PLATFORM_MANIFEST.backend.version);

    // Step 2.5 — managed Fabric Transport worker (consumer-packaged Cowork
    // P1). The desktop owns the worker lifecycle end-to-end: the loopback
    // ensure route mints the gizzi principal credential (returned once), the
    // desktop keeps it in the Keychain-backed secure store, and the bundled
    // gizzi-code binary runs `fabric-worker` with crash-respawn + graceful
    // SIGTERM quit. Degrades visibly, never silently.
    updateSplash('Starting fabric worker…', 60);
    try {
      await startManagedFabricWorker();
    } catch (workerErr) {
      log.warn('[Main] Fabric worker failed to start, continuing without it:', workerErr);
      serviceState.fabricWorker = { status: 'down', detail: 'Worker unavailable — restart the app' };
      pushServiceState();
    }

    // Bonsai companion follows the app lifecycle: auto-start when installed.
    bonsaiCompanion.getStatus()
      .then((status) => (status.installed && !status.running ? bonsaiCompanion.start() : undefined))
      .catch((err) => log.warn('[Bonsai] auto-start skipped:', err));

    updateSplash('Connecting to platform…', 80);

    // Step 3 — Platform URL
    // Dev:        local Next.js dev server on port ${PORTS.DEV_UI}
    // Dev(static): ALLTERNIT_DESKTOP_USE_STATIC_UI forces the dev build to load
    //              the local platform static export instead of the Vite server.
    // Bundled:    Prefer the local static UI build so the desktop app always
    //              ships with the matching platform version and is not at the
    //              mercy of a stale ai.allternit.com deployment. Fall back to
    //              the remote URL only when no local build is present.
    // Offline:    If remote URL is unreachable, fall back to local static files
    //              served by the Rust API at the local API URL.
    // Allow ALLTERNIT_PLATFORM_URL to override the platform URL in dev/bundled
    // mode so local worktree UI builds (e.g. Vite on a non-default port) can be
    // tested without repackaging the desktop.
    let platformUrl: string = process.env.ALLTERNIT_PLATFORM_URL?.trim()
      || (isDev ? URLS.DEV_UI : URLS.PRODUCTION_UI);

    if (isDev && process.env.ALLTERNIT_DESKTOP_USE_STATIC_UI) {
      const localStaticPath = resolveLocalPlatformStaticPath();
      if (localStaticPath) {
        log.info(`[Main] Dev mode forced to local platform static UI from ${localStaticPath}`);
        platformUrl = staticUiUrl();
        serviceState.platform = { status: 'up', detail: 'Local static UI (dev)' };
      } else {
        log.warn('[Main] ALLTERNIT_DESKTOP_USE_STATIC_UI set but no local static UI found; using dev server');
      }
      pushServiceState();
    }

    if (!isDev) {
      // If the operator explicitly set ALLTERNIT_PLATFORM_URL, honor it and
      // skip the local static UI preference. Only fall back to the bundled
      // static UI when no override is set, or when the override URL is
      // unreachable.
      const envPlatformUrl = process.env.ALLTERNIT_PLATFORM_URL;
      const localStaticPath = resolveLocalPlatformStaticPath();
      if (envPlatformUrl) {
        const remoteReachable = await isUrlReachable(envPlatformUrl, 5000);
        if (remoteReachable) {
          log.info(`[Main] Using ALLTERNIT_PLATFORM_URL override: ${envPlatformUrl}`);
          serviceState.platform = { status: 'up', detail: envPlatformUrl };
        } else {
          log.warn(`[Main] ALLTERNIT_PLATFORM_URL ${envPlatformUrl} is unreachable — falling back to local static UI`);
          platformUrl = staticUiUrl();
          serviceState.platform = { status: 'up', detail: 'Offline mode (local static)' };
        }
      } else if (localStaticPath) {
        log.info(`[Main] Using local platform static UI from ${localStaticPath}`);
        platformUrl = staticUiUrl();
        serviceState.platform = { status: 'up', detail: 'Local static UI' };
      } else {
        const remoteReachable = await isUrlReachable(platformUrl, 5000);
        if (!remoteReachable) {
          log.warn(`[Main] Remote platform ${platformUrl} is unreachable — falling back to local static UI`);
          platformUrl = staticUiUrl();
          serviceState.platform = { status: 'up', detail: 'Offline mode (local static)' };
        } else {
          log.warn(`[Main] No local platform static UI found; using remote ${platformUrl}`);
          serviceState.platform = { status: 'up', detail: 'ai.allternit.com' };
        }
      }
      pushServiceState();
    }

    if (isDev) {
      // Write gizzi credentials to a session file so the external dev server can read them.
      const sessionFile = join(os.homedir(), '.allternit', 'gizzi-dev-session.json');
      try {
        await fs.promises.mkdir(dirname(sessionFile), { recursive: true });
        await fs.promises.writeFile(sessionFile, JSON.stringify({
          gizziUrl: gizziUrl ?? URLS.GIZZI,
          gizziPassword: gizziManager.getPassword() ?? '',
          writtenAt: Date.now(),
        }), 'utf8');
        log.info(`[Main] Wrote gizzi dev session credentials to ${sessionFile}`);
      } catch (err) {
        log.warn('[Main] Failed to write gizzi dev session file:', err);
      }
    }

    // Preserve offline/static detail if we fell back; otherwise set the normal label.
    if (!['Offline mode (local static)', 'Local static UI', 'Local static UI (dev)'].includes(serviceState.platform.detail)) {
      serviceState.platform = { status: 'up', detail: isDev ? `Dev on port ${PORTS.DEV_UI}` : 'ai.allternit.com' };
      pushServiceState();
    }

    activePlatformUrl = process.env.ALLTERNIT_PLATFORM_URL || platformUrl;
    log.info(`[Main] Platform URL: ${activePlatformUrl}`);
    // Clerk authenticates the human in the browser. The desktop waits for the
    // separately scoped runtime pairing that was started alongside boot.
    if (showStartupWizard) {
      updateSplash('Waiting for sign-in…');
      await startupSignIn;
      store.set('startupWizardCompleted', true);
    }

    // Complete — the runtime is up. The main window loads immediately; the
    // splash stays on screen only until that window has finished loading AND
    // the folder-grant step (when shown) has resolved.
    sendToSplash('progress', 100);
    sendToSplash('complete');

    mainWindow = createMainWindow();

    log.info(`[Main] Loading platform URL: ${platformUrl}`);

    // Splash dismissal state. Grants never gate launch: the main window loads
    // underneath the splash, and a 15s force-close bounds the wait so the
    // splash (and boot) can never hang on the grant dialog.
    let mainWindowLoaded = false;
    let folderGrantsDone = false;
    let splashClosed = false;
    const maybeCloseSplash = (force = false) => {
      if (splashClosed || (!force && (!mainWindowLoaded || !folderGrantsDone || folderGrantsPending))) return;
      splashClosed = true;
      clearTimeout(splashForceClose);
      // Brief beat so the 'Ready' state paints before the window swaps.
      setTimeout(() => {
        if (splashWindow && !splashWindow.isDestroyed()) {
          splashWindow.close();
        }
        splashWindow = null;
      }, 150);
    };
    const splashForceClose = setTimeout(() => {
      // The force-close must win over a still-pending grant prompt, or the
      // always-on-top splash would sit on top of the loaded app forever when
      // the user walks away from the dialog. Ungranted profiles are simply
      // re-asked on the next launch.
      log.warn('[Main] Splash force-close timer fired (window load or folder grants still pending)');
      maybeCloseSplash(true);
    }, 15_000);

    // Log loading events for debugging
    mainWindow.webContents.on('did-start-loading', () => {
      log.info('[Main] Window started loading');
    });
    mainWindow.webContents.on('did-finish-load', () => {
      mainWindowLoaded = true;
      log.info('[Main] Window finished loading');
      mainWindow?.show();
      maybeCloseSplash();
    });
    mainWindow.webContents.on('did-fail-load', (event, errorCode, errorDescription, validatedURL) => {
      log.error(`[Main] Window failed to load: ${errorDescription} (${errorCode}) at ${validatedURL}`);
    });
    mainWindow.webContents.on('did-navigate', (_event, url) => {
      log.info(`[Main] Window did-navigate: ${url}`);
    });
    mainWindow.webContents.on('did-navigate-in-page', (_event, url) => {
      log.info(`[Main] Window did-navigate-in-page: ${url}`);
    });
    mainWindow.on('close', (event) => {
      log.warn('[Main] mainWindow close event fired', { destroyed: mainWindow?.isDestroyed() });
    });
    mainWindow.webContents.on('dom-ready', () => {
      log.info('[Main] DOM ready');
    });
    
    mainWindow.loadURL(activePlatformUrl);

    // Step 2.9 — folder grants (consumer-packaged Cowork P1). Ask once, while
    // the platform loads underneath, when the profile has no trusted folders
    // yet. Awaited for bookkeeping only — the splash close above does not
    // depend on it beyond the force-close bound.
    void (async () => {
      try {
        await maybeRequestFolderGrants();
      } catch (folderErr) {
        log.warn('[Main] Folder-grant step skipped:', folderErr);
      }
      folderGrantsDone = true;
      maybeCloseSplash();
    })();

    // First launch: used for permission onboarding gating below
    const isFirstLaunch = !store.get('onboardingComplete');

    // Push tunnel state changes to the renderer
    tunnelManager.onStatusChange((state) => {
      mainWindow?.webContents.send('tunnel:state', state);
    });

    // Fallback: show window after a timeout even if ready-to-show doesn't fire
    setTimeout(() => {
      if (mainWindow && !mainWindow.isVisible()) {
        log.info('[Main] Showing window via timeout fallback');
        mainWindow.show();
      }
    }, 3000);

    // ── First-launch: macOS permission onboarding ─────────────────────────
    // Desktop automation requires Accessibility and Screen Recording.
    // On macOS, guide the user through granting these permissions.
    // The renderer signals readiness via permissionGuide.readyForCheck() when
    // its onboarding wizard reaches the permissions step. We also set a
    // fallback timeout so old platform versions still get guided.
    if (isFirstLaunch) {
      mainWindow.webContents.once('did-finish-load', async () => {
        if (!store.get('permissions').promptedDuringOnboarding) {
          let onboardingStarted = false;
          const startPermissionOnboarding = async () => {
            if (onboardingStarted) return;
            onboardingStarted = true;
            try {
              const result = await runPermissionOnboarding((status) => {
                mainWindow?.webContents.send('permission-guide:status', status);
              });
              store.set('permissions.promptedDuringOnboarding', true);
              store.set('permissions.lastStatus', {
                accessibility: result.accessibility,
                screenRecording: result.screenRecording,
                checkedAt: new Date().toISOString(),
              });
            } catch (err) {
              log.error('[Main] Permission onboarding failed:', err);
            }
          };

          // Fallback: if the renderer never signals ready, start after 8s.
          setTimeout(() => startPermissionOnboarding(), 8000);

          // Register a one-shot resolver so the global permission-guide:ready-for-check
          // handler can trigger the onboarding flow when the renderer explicitly
          // signals it has reached the permissions step.
          permissionOnboardingResolver = () => {
            startPermissionOnboarding();
          };
        }
      });
    }

    // ── First-launch: optional always-on cloud scheduler daemon ────────────
    // Gizzi Code can run as a background daemon so scheduled tasks and cron
    // jobs survive app restarts. On first launch we ask once.
    if (isFirstLaunch) {
      mainWindow.webContents.once('did-finish-load', async () => {
        try {
          const daemonStatus = await gizziDaemonManager.getStatus();
          if (daemonStatus.installed) return;

          // An Allternit cloud computer autostarts this app and never closes
          // it, so the app's own gizzi runtime already keeps schedules running.
          // Installing the daemon there would only stop and restart gizzi.
          if (isProvisionedMode()) {
            log.info('[Main] Provisioned computer: the app is the always-on runtime; no daemon needed');
            return;
          }

          // The installer script isn't shipped with every build; don't offer
          // a choice that can only fail.
          if (!gizziDaemonManager.resolveInstallScript()) {
            log.info('[Main] Daemon install script not bundled; not offering Cloud Scheduling');
            return;
          }

          const { response } = await dialog.showMessageBox(mainWindow!, {
            type: 'info',
            title: 'Enable Cloud Scheduling?',
            message: 'Allow Allternit to install a small background daemon?',
            detail:
              'This lets Allternit run scheduled tasks and cron jobs even when the desktop app is closed. You can change this anytime in Settings.',
            buttons: ['Enable Scheduling', 'Not Now'],
            defaultId: 0,
            cancelId: 1,
          });

          if (response === 0) {
            await installAlwaysOnGizziRuntime();
          }
        } catch (err) {
          log.error('[Main] Daemon onboarding failed:', err);
        }
      });
    }

    // ── Service Watchdog: Maintain backend health ──────────────────────────
    setInterval(async () => {
      try {
        const status = await backendManager.getStatus();
        if (!status.running) {
          log.warn('[Watchdog] Backend services found stopped, attempting auto-recovery...');
          await backendManager.ensureBackend();
        }
      } catch (err) {
        log.error('[Watchdog] Health check failed:', err);
      }
    }, 30_000);
    
  } catch (error) {
    log.error('[Main] Failed to initialize bundled mode:', error);
    sendToSplash('error', (error as Error).message);

    dialog.showErrorBox(
      'Allternit Desktop Initialization Error',
      `Failed to start Allternit Backend:\n${(error as Error).message}\n\nPlease try again or contact support.`
    );
    
    app.quit();
  }
}

async function initializeRemoteMode(remoteUrl: string): Promise<void> {
  log.info('[Main] Remote mode - connecting to', remoteUrl);
  
  // Check connection
  try {
    const response = await fetch(`${remoteUrl}/health`, {
      signal: AbortSignal.timeout(5000)
    });
    
    if (!response.ok) {
      throw new Error('Backend not responding');
    }
    
    // Check version compatibility
    const versionResponse = await fetch(`${remoteUrl}/version`);
    const versionData = await versionResponse.json() as { version: string };
    const version = versionData.version;
    
    if (shouldUpdateBackend(version)) {
      const result = await dialog.showMessageBox({
        type: 'warning',
        title: 'Allternit Desktop Backend Update Required',
        message: `Your remote backend (${version}) does not match Allternit Desktop ${PLATFORM_MANIFEST.backend.version}.`,
        detail:
          'Desktop cannot SSH into the remote host. Update allternit-api on that server to ' +
          `${PLATFORM_MANIFEST.backend.version}, then reconnect. You can keep this mismatched session or switch to the local backend.`,
        buttons: ['Continue Anyway', 'Switch to Local'],
        defaultId: 0,
        cancelId: 0,
      });
      if (result.response === 1) {
        store.set('backend.mode', 'bundled');
        await initializeBundledMode();
        return;
      }
    }
    
    // Track the resolved URL for SDK consumers
    activeBackendUrl = remoteUrl;

    // Open main window
    mainWindow = createMainWindow();
    mainWindow.loadURL(`${remoteUrl}/platform`);
    mainWindow.show();
    
  } catch (error) {
    log.error('[Main] Remote connection failed:', error);
    
    const result = await dialog.showMessageBox({
      type: 'error',
      title: 'Connection Failed',
      message: `Cannot connect to Allternit Backend at ${remoteUrl}.`,
      buttons: ['Try Again', 'Switch to Local Mode', 'Quit'],
      defaultId: 0,
    });
    
    if (result.response === 0) {
      await initializeRemoteMode(remoteUrl);
    } else if (result.response === 1) {
      store.set('backend.mode', 'bundled');
      await initializeBundledMode();
    } else {
      app.quit();
    }
  }
}

async function initializeDevelopmentMode(): Promise<void> {
  const platformUrl = process.env.ALLTERNIT_PLATFORM_URL?.trim() || URLS.DEV_UI;
  activePlatformUrl = platformUrl;
  log.info('[Main] Development mode', { platformUrl });
  activeBackendUrl = URLS.DEV_UI;

  // Show the platform window immediately so the UI is usable while optional
  // runtime services start in the background.
  mainWindow = createMainWindow();
  mainWindow.loadURL(URLS.DEV_UI);
  mainWindow.show();

  // Adopt or start the local Gizzi runtime so the sidecar can broker
  // credential-injected requests via the allternit-gizzi custom protocol.
  // In dev this is best-effort: if no runtime is available the rest of the
  // app still loads.
  try {
    const gizziUrl = await startGizziRuntime();
    updateSidecarConfig(gizziUrl);
    log.info('[Main] Gizzi runtime ready in development mode:', gizziUrl);
  } catch (error) {
    log.warn('[Main] No adoptable Gizzi runtime in development mode; continuing without brokered AI runtime:', error);
  }

  mainWindow = createMainWindow();
  activePlatformUrl = process.env.ALLTERNIT_PLATFORM_URL || platformUrl;
  log.info(`[Main] Dev mode loading platform URL: ${activePlatformUrl}`);
  mainWindow.loadURL(activePlatformUrl);
  mainWindow.webContents.openDevTools();
  mainWindow.show();
}

// ============================================================================
// Extension Bridge — TCP server for native messaging host
// The Chrome extension connects via chrome.runtime.connectNative('com.allternit.desktop').
// Chrome spawns native-host/native-host as a child process; it then connects back
// here over TCP on port EXTENSION_BRIDGE to relay messages bidirectionally.
// ============================================================================

const EXTENSION_BRIDGE_PORT = PORTS.EXTENSION_BRIDGE;

/**
 * Pending HTTP relay requests waiting for a response from the extension.
 * Key = message id sent to the extension; value = resolver function.
 */
const pendingRelayResponses = new Map<string, (data: unknown) => void>();

function startExtensionBridge(): void {
  const server = net.createServer((socket) => {
    log.info('[ExtensionBridge] Native host connected');
    extensionSockets.add(socket);

    // Notify renderer that extension is now connected
    mainWindow?.webContents.send('extension:status', { connected: true });

    let buffer = '';
    socket.on('data', (data) => {
      buffer += data.toString();
      const lines = buffer.split('\n');
      buffer = lines.pop() ?? '';
      for (const line of lines) {
        if (!line.trim()) continue;
        try {
          const message = JSON.parse(line) as Record<string, unknown>;
          // Forward to renderer
          mainWindow?.webContents.send('extension:message', message);
          // Resolve any pending ACU relay request waiting on this id
          const id = message['id'] as string | undefined;
          if (id && pendingRelayResponses.has(id)) {
            pendingRelayResponses.get(id)!(message);
            pendingRelayResponses.delete(id);
          }
        } catch {
          log.warn('[ExtensionBridge] Unparseable message from native host:', line.slice(0, 100));
        }
      }
    });

    socket.on('close', () => {
      log.info('[ExtensionBridge] Native host disconnected');
      extensionSockets.delete(socket);
      mainWindow?.webContents.send('extension:status', {
        connected: getConnectedExtensionSockets().length > 0,
      });
    });

    socket.on('error', (err) => {
      log.warn('[ExtensionBridge] Socket error:', err.message);
    });
  });

  server.listen(EXTENSION_BRIDGE_PORT, '127.0.0.1', () => {
    log.info(`[ExtensionBridge] Listening on port ${EXTENSION_BRIDGE_PORT}`);
  });

  server.on('error', (err) => {
    log.warn('[ExtensionBridge] Server error (native host may not be registered):', err.message);
  });
}

// ============================================================================
// ACU Extension Relay — HTTP endpoint for the Computer-Use engine
//
// ACU (Python, port 8760) cannot connect to TCP EXTENSION_BRIDGE directly because that
// port is owned exclusively by this process as a server for the native host.
// Instead ACU POSTs to http://127.0.0.1:ACU_RELAY/extension/send and this handler
// forwards the message onto the extension socket.
//
// Port: ACU_EXTENSION_RELAY_PORT env var, default PORTS.ACU_RELAY
// ============================================================================

const ACU_RELAY_PORT = parseInt(process.env['ACU_EXTENSION_RELAY_PORT'] ?? String(PORTS.ACU_RELAY), 10);

function startAcuExtensionRelay(): void {
  const RELAY_TIMEOUT_MS = parseInt(process.env['ACU_EXTENSION_TIMEOUT_MS'] ?? '15000', 10);

  const server = http.createServer((req, res) => {
    if (req.method === 'POST' && req.url === '/extension/send') {
      let body = '';
      req.on('data', (chunk: Buffer) => { body += chunk.toString(); });
      req.on('end', () => {
        try {
          const message = JSON.parse(body) as Record<string, unknown>;
          if (getConnectedExtensionSockets().length === 0) {
            res.writeHead(503, { 'Content-Type': 'application/json' });
            res.end(JSON.stringify({ ok: false, error: 'extension_not_connected' }));
            return;
          }

          const msgId = message['id'] as string | undefined;
          sendToExtension(message);

          if (!msgId) {
            // Fire-and-forget: no id means caller doesn't expect a correlated response
            res.writeHead(200, { 'Content-Type': 'application/json' });
            res.end(JSON.stringify({ ok: true }));
            return;
          }

          // Wait for correlated response via pendingRelayResponses
          const timer = setTimeout(() => {
            if (pendingRelayResponses.has(msgId)) {
              pendingRelayResponses.delete(msgId);
              res.writeHead(504, { 'Content-Type': 'application/json' });
              res.end(JSON.stringify({ ok: false, error: 'extension_timeout' }));
            }
          }, RELAY_TIMEOUT_MS);

          pendingRelayResponses.set(msgId, (responseData) => {
            clearTimeout(timer);
            res.writeHead(200, { 'Content-Type': 'application/json' });
            res.end(JSON.stringify({ ok: true, data: responseData }));
          });
        } catch {
          res.writeHead(400, { 'Content-Type': 'application/json' });
          res.end(JSON.stringify({ ok: false, error: 'invalid_json' }));
        }
      });
    } else if (req.method === 'GET' && req.url === '/extension/status') {
      const connected = getConnectedExtensionSockets().length > 0;
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ connected }));
    } else {
      res.writeHead(404);
      res.end();
    }
  });

  server.listen(ACU_RELAY_PORT, '127.0.0.1', () => {
    log.info(`[AcuRelay] Extension relay HTTP server listening on port ${ACU_RELAY_PORT}`);
  });

  server.on('error', (err) => {
    log.warn('[AcuRelay] Failed to start extension relay server:', err.message);
  });
}

// ============================================================================
// Quick Chat Mini-Window (Cmd+Shift+A)
// ============================================================================

function createMiniWindow(): BrowserWindow {
  const { workArea } = screen.getPrimaryDisplay();
  const x = Math.round(workArea.x + (workArea.width - MINI_WINDOW_WIDTH) / 2);
  const y = Math.round(workArea.y + (workArea.height - MINI_WINDOW_HEIGHT) / 2);

  const win = new BrowserWindow({
    width: MINI_WINDOW_WIDTH,
    height: MINI_WINDOW_HEIGHT,
    x,
    y,
    minWidth: 480,
    minHeight: 400,
    show: false,
    frame: false,
    resizable: true,
    movable: true,
    alwaysOnTop: true,
    skipTaskbar: true,
    fullscreenable: false,
    maximizable: false,
    transparent: true,
    ...(isMac ? { vibrancy: 'under-window', visualEffectState: 'active' } : {}),
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
    ...(isMac ? {
      titleBarStyle: 'hidden',
      trafficLightPosition: { x: -100, y: -100 },
      roundedCorners: true,
    } : {}),
  });

  installWillNavigateGuard(win.webContents);
  installWindowOpenGuard(win.webContents);

  const platformUrl = isDev
    ? devUiUrl('/?mini=1')
    : `${activePlatformUrl}/?mini=1`;

  win.loadURL(platformUrl);

  win.on('blur', () => {
    if (!win.webContents.isDevToolsFocused()) win.hide();
  });

  win.on('close', (e) => {
    e.preventDefault();
    win.hide();
  });

  return win;
}

function toggleMiniWindow(): void {
  if (!miniWindow || miniWindow.isDestroyed()) {
    miniWindow = createMiniWindow();
    miniWindow.once('ready-to-show', () => {
      miniWindow?.show();
      miniWindow?.focus();
    });
    return;
  }
  if (miniWindow.isVisible() && miniWindow.isFocused()) {
    miniWindow.hide();
  } else {
    miniWindow.show();
    miniWindow.focus();
  }
}

// ============================================================================
// Tray
// ============================================================================

function showConnectionSettings(): void {
  if (!mainWindow) {
    mainWindow = createMainWindow();
  }
  mainWindow.show();
  mainWindow.focus();
  mainWindow.loadFile(join(__dirname, '../../static/connect.html'));
}

function createTray(): void {
  const iconPath = join(__dirname, '../../build/tray-icon.png');
  if (!fs.existsSync(iconPath)) return;

  tray = new Tray(iconPath);
  tray.setToolTip('Allternit Desktop');
  // In menu bar mode, single-click toggles window visibility
  tray.on('click', () => {
    const prefs = persistedState.get('prefs');
    if (prefs.menuBarMode) {
      if (mainWindow?.isVisible()) {
        mainWindow.hide();
      } else {
        showOrReopenMainWindow();
      }
    }
  });
  updateTrayMenu();
}

async function updateTrayMenu(): Promise<void> {
  if (!tray) return;

  const backend = store.get('backend');
  const status = await backendManager.getStatus();
  
  const statusIcon = status.running ? '🟢' : '🔴';
  const modeLabel = backend.mode === 'bundled' ? 'Local' : 
                    backend.mode === 'remote' ? 'VPS' : 'Dev';

  // Desktop permission status for tray indicator
  let permItem: Electron.MenuItemConstructorOptions | undefined;
  const perm = store.get('permissions')?.lastStatus;
  const hasIssue = perm && (perm.accessibility === 'denied' || perm.screenRecording === 'denied');
  const allOk = perm && perm.accessibility !== 'denied' && perm.screenRecording !== 'denied';
  permItem = {
    label: hasIssue ? '⚠️ Check Permissions' : allOk ? '✅ Permissions OK' : '🔍 Check Permissions',
    click: async () => {
      showOrReopenMainWindow();
      const status = await checkPermissions();
      store.set('permissions.lastStatus', { ...status, checkedAt: new Date().toISOString() });
      mainWindow?.webContents.send('permission-guide:status', status);
      if (status.accessibility === 'denied') {
        await presentGuide('accessibility');
      } else if (status.screenRecording === 'denied') {
        await presentGuide('screen-recording');
      }
    },
  };

  const template: Electron.MenuItemConstructorOptions[] = [
    { label: 'Allternit Desktop', enabled: false },
    { type: 'separator' },
    { label: `${statusIcon} ${status.running ? 'Running' : 'Stopped'}`, enabled: false },
    { label: `Mode: ${modeLabel}`, enabled: false },
    { type: 'separator' },
    {
      label: 'Connection Settings...',
      click: () => {
        showConnectionSettings();
      },
    },
    ...(permItem ? [permItem, { type: 'separator' as const }] : []),
    { label: 'Show Window', click: () => showOrReopenMainWindow() },
    { label: 'Show Desktop Pet', click: () => desktopCompanion.show() },
    { label: 'Desktop Pet Settings…', click: () => desktopCompanion.settings() },
    { label: 'Reset Pet Position', click: () => desktopCompanion.reset() },
    {
      // Opens the popped-out Allternit Office window (the ACI rail's bottom
      // tab) — the renderer routes the 'launcher' target there.
      label: 'Allternit Office',
      click: () => openOfficeTarget('launcher'),
    },
    { label: 'Quick Chat', accelerator: QUICK_CHAT_HOTKEY, click: () => toggleMiniWindow() },
    { label: 'Toggle HUD', accelerator: HUD_HOTKEY, click: () => toggleHudWindow() },
    { type: 'separator' },
    { label: 'Quit', click: () => app.quit() },
  ];

  tray.setContextMenu(Menu.buildFromTemplate(template));
}

// ============================================================================
// App Lifecycle
// ============================================================================

const gotTheLock = app.requestSingleInstanceLock();
if (!gotTheLock) {
  app.quit();
  process.exit(0);
}

function extractProtocolUrl(argv: string[]): string | null {
  return argv.find((value) => value.startsWith('allternit://')) ?? null;
}

function registerProtocolHandler(): void {
  const success = process.defaultApp
    ? app.setAsDefaultProtocolClient('allternit', process.execPath, [process.argv[1]!])
    : app.setAsDefaultProtocolClient('allternit');

  log.info('[Main] Protocol handler registration attempted', {
    success,
    isDefaultProtocolClient: app.isDefaultProtocolClient('allternit'),
    defaultApp: process.defaultApp,
    execPath: process.execPath,
    argv1: process.argv[1] ?? null,
  });

  if (process.defaultApp) {
    return;
  }
}

// Register protocol handler as early as possible (before ready)
registerProtocolHandler();

// URLs received before authManager.initialize() completes are buffered here.
let earlyProtocolUrl: string | null = null;
let authManagerReady = false;

async function handleProtocolCallback(url: string | null): Promise<void> {
  if (!url) return;
  log.info('[Main] handleProtocolCallback:', url);

  // Platform deep links handled directly by the desktop shell.
  if (url === 'allternit://hud' || url === 'allternit://open/hud') {
    toggleHudWindow();
    return;
  }

  // A:// Studio (Design mode) deep link from gizzi-code's `/design` command:
  // allternit://design?prompt=... (allternit-dev:// in dev builds). Opens the
  // design window at /design, carrying the prompt into the studio composer.
  if (
    url.startsWith('allternit://design') ||
    url.startsWith('allternit-dev://design')
  ) {
    let prompt: string | null = null;
    try {
      prompt = new URL(url).searchParams.get('prompt');
    } catch {
      prompt = null;
    }
    openDesignStudio(prompt);
    return;
  }

  // `gizzi login` from a terminal on this Mac: allternit://pair?code=ABCD-1234
  // opens the pairing approval page in the signed-in app, so the terminal
  // uses the same account with one click instead of a second browser sign-in.
  if (isPairingLink(url)) {
    const code = pairingCodeFromUrl(url);
    if (code) openPairingApproval(code);
    return;
  }

  // Buffer if auth manager hasn't initialized yet — will be flushed after initialize()
  if (!authManagerReady) {
    log.info('[Main] Auth manager not yet initialized — buffering URL');
    earlyProtocolUrl = url;
    return;
  }

  try {
    const handled = await authManager.handleCallbackUrl(url);
    log.info('[Main] handleCallbackUrl result:', handled);
    if (handled && mainWindow) {
      if (mainWindow.isMinimized()) mainWindow.restore();
      mainWindow.show();
      mainWindow.focus();
    }
  } catch (error) {
    log.error('[Main] Failed to handle auth callback URL:', error);
    dialog.showErrorBox('Authentication Error', (error as Error).message);
  }
}

// Register allternit-api as a privileged scheme so it supports fetch() and CORS.
// Must happen before app ready.
protocol.registerSchemesAsPrivileged([
  // Build-output previews (artifact-preview-host): a real origin of their
  // own so a page gets its own CSP instead of inheriting the app's.
  { scheme: PREVIEW_SCHEME, privileges: { standard: true, secure: true } },
  {
    scheme: 'allternit-api',
    privileges: {
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
      standard: true,
    },
  },
  {
    scheme: 'allternit-gizzi',
    privileges: {
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
      standard: true,
    },
  },
]);

// On macOS, open-url fires during will-finish-launching (before ready).
// Registering here is the documented correct place for this event.
app.on('will-finish-launching', () => {
  app.on('open-url', (event, url) => {
    log.info('[Main] open-url fired:', url);
    event.preventDefault();
    void handleProtocolCallback(url);
  });
});

app.on('second-instance', (_event, argv) => {
  const url = extractProtocolUrl(argv);
  log.info('[Main] second-instance fired, URL:', url);
  void handleProtocolCallback(url);
  // File-association launch (Windows/Linux pass the file in argv).
  const officeFile = extractOfficeFileArg(argv);
  if (officeFile) openOfficeWithFile(officeFile);
  if (mainWindow) {
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.show();
    mainWindow.focus();
  }
});

// macOS file association ("Open with Allternit").
app.on('open-file', (event, filePath) => {
  event.preventDefault();
  log.info('[Main] open-file:', filePath);
  if (app.isReady()) {
    openOfficeWithFile(filePath);
  } else {
    app.whenReady().then(() => openOfficeWithFile(filePath));
  }
});

app.whenReady().then(async () => {
  console.log('[Main] App is ready...');

  // Phase 0 convenience hook: open the Allternit Docs editor on startup
  // when explicitly requested (e.g. dev smoke test). Delivered to the main
  // window (queued until it has loaded).
  if (process.env.ALLTERNIT_OPEN_DOCS_ON_START) {
    openOfficeTarget('docs');
  }

  // Cold-start file association (Windows/Linux first instance).
  const officeFileArg = extractOfficeFileArg(process.argv);
  if (officeFileArg) {
    openOfficeWithFile(officeFileArg);
  }

  console.log('[Main] Initializing auth manager...');
  await authManager.initialize();
  authManagerReady = true;
  const startupBackend = store.get('backend');
  void authManager.updateBackendProfile({
    mode: startupBackend.mode,
    remoteUrl: startupBackend.remoteUrl,
  });

  // Flush any URL that arrived before initialize() completed
  const urlToProcess = earlyProtocolUrl ?? extractProtocolUrl(process.argv);
  earlyProtocolUrl = null;
  if (urlToProcess) {
    console.log('[Main] Processing buffered/startup protocol URL...');
    void handleProtocolCallback(urlToProcess);
  }

  console.log('[Main] Registering allternit-api protocol handler...');
  protocol.handle(PREVIEW_SCHEME, (request) => servePreview(request.url));

  protocol.handle('allternit-api', async (request) => {
    const url = new URL(request.url);
    // D16: the window's person-act marker rides in the URL because Chromium
    // drops custom headers on the /api redirect; never forwarded as-is.
    const personMarked = takeDesktopProofParam(url);
    const pathAndQuery = `${url.pathname}${url.search}`;
    const isCloud = url.hostname === 'cloud' || url.host === 'cloud';
    const targetUrl = isCloud ? `${URLS.CLOUD_API}${pathAndQuery}` : apiUrl(pathAndQuery);

    // CORS preflight for custom-protocol cross-origin requests
    if (request.method === 'OPTIONS') {
      return new Response(null, {
        status: 204,
        headers: {
          'Access-Control-Allow-Origin': '*',
          'Access-Control-Allow-Methods': 'GET, POST, PUT, DELETE, PATCH, OPTIONS',
          'Access-Control-Allow-Headers': '*',
          'Access-Control-Max-Age': '86400',
        },
      });
    }

    const headers = new Headers(request.headers);
    if (personMarked) headers.set(HUMAN_PROOF_HEADER, DESKTOP_PROOF_MARKER);
    // D16: Desktop's proof only goes to this machine's API, never the cloud.
    applyDesktopHumanProofTo(headers, isCloud ? null : backendManager.getHumanProof());
    if (isCloud) {
      // Pairing device tokens are not Clerk JWTs. Cloud-api rejects them with
      // 401. Public catalog routes take no bearer at all.
      headers.delete('X-Allternit-Desktop-Access-Token');
      headers.delete('X-Allternit-User-Id');
      headers.delete('X-Allternit-User-Email');
      headers.delete('X-Allternit-User-Name');
      headers.delete('X-Allternit-Tenant-Id');
      if (isPublicCloudCatalogPath(pathAndQuery)) {
        headers.delete('Authorization');
      } else {
        const clerk = await authManager.getClerkToken().catch((error) => {
          log.warn('[Protocol] Clerk JWT unavailable for cloud request:', error);
          return null;
        });
        const deviceSession = !clerk && acceptsDeviceToken(pathAndQuery)
          ? await authManager.getSession().catch(() => null)
          : null;
        if (clerk) {
          headers.set('Authorization', `Bearer ${clerk}`);
        } else if (deviceSession) {
          // Signed in by device pairing only: routes that accept the paired
          // device token (mesh enroll, computer pairing) get it instead.
          headers.set('Authorization', `Bearer ${deviceSession.accessToken}`);
        } else {
          headers.delete('Authorization');
        }
      }
    } else {
      const desktopSession = await authManager.getSession().catch((error) => {
        log.warn('[Protocol] Paired runtime identity is temporarily unavailable:', error);
        return null;
      });
      if (desktopSession) {
        // The renderer never receives this credential. Electron main brokers the
        // scoped identity plus user metadata to the loopback-only API.
        headers.set('Authorization', `Bearer ${desktopSession.accessToken}`);
        headers.set('X-Allternit-Desktop-Access-Token', desktopSession.accessToken);
        headers.set('X-Allternit-User-Id', desktopSession.userId);
        headers.set('X-Allternit-User-Email', desktopSession.userEmail);
        headers.set('X-Allternit-User-Name', desktopSession.userEmail);
        if (desktopSession.organizationId) {
          headers.set('X-Allternit-Tenant-Id', desktopSession.organizationId);
        }
      } else if (isDev && !headers.has('X-Allternit-Desktop-Access-Token')) {
        // Chromium strips the renderer's custom identity headers when
        // onBeforeRequest redirects /api/* across origins to this protocol.
        // Development-only bootstrap; packaged apps must have a paired runtime.
        headers.set('X-Allternit-Desktop-Access-Token', 'desktop-dev-bootstrap');
        headers.set('X-Allternit-User-Id', 'desktop-dev-user');
        headers.set('X-Allternit-User-Email', 'desktop@allternit.local');
        headers.set('X-Allternit-User-Name', 'Desktop Dev User');
      }
    }

    try {
      // Electron exposes the custom-protocol request body as a ReadableStream.
      // Passing that stream directly to Node's fetch requires a non-standard
      // `duplex` option and caused all Design POST requests to fail. Buffer the
      // payload first so ordinary RequestInit semantics work reliably.
      const requestBody = request.method === 'GET' || request.method === 'HEAD'
        ? undefined
        : await request.arrayBuffer();
      const response = await fetch(targetUrl, {
        method: request.method,
        headers,
        body: requestBody,
      });

      const responseHeaders = new Headers(response.headers);
      responseHeaders.set('Access-Control-Allow-Origin', '*');

      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers: responseHeaders,
      });
    } catch (error) {
      log.error('[Protocol] Proxy error:', error);
      return new Response(JSON.stringify({ error: 'proxy_error', message: String(error) }), {
        status: 502,
        headers: { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': '*' },
      });
    }
  });

  // Gizzi remains loopback-only and keeps its internal process credential.
  // Renderer code receives only this broker URL; Electron main injects the
  // credential and streams the response without exposing the secret.
  protocol.handle('allternit-gizzi', async (request) => {
    const url = new URL(request.url);
    const targetUrl = `${gizziManager.getUrl()}${url.pathname}${url.search}`;
    if (request.method === 'OPTIONS') {
      return new Response(null, {
        status: 204,
        headers: {
          'Access-Control-Allow-Origin': '*',
          'Access-Control-Allow-Methods': 'GET, POST, PUT, DELETE, PATCH, OPTIONS',
          'Access-Control-Allow-Headers': '*',
        },
      });
    }
    const headers = new Headers(request.headers);
    const authHeader = gizziManager.getAuthHeader();
    if (authHeader) headers.set('Authorization', authHeader);
    try {
      const body = request.method === 'GET' || request.method === 'HEAD'
        ? undefined
        : await request.arrayBuffer();
      const response = await fetch(targetUrl, { method: request.method, headers, body });
      const responseHeaders = new Headers(response.headers);
      responseHeaders.set('Access-Control-Allow-Origin', '*');
      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers: responseHeaders,
      });
    } catch (error) {
      log.error('[GizziBroker] Request failed:', error);
      return new Response(JSON.stringify({ error: 'gizzi_unavailable' }), {
        status: 502,
        headers: { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': '*' },
      });
    }
  });

  console.log('[Main] Initializing foundation systems...');
  // Session-wide CSP + default-deny permission handler, scoped to app origins
  // so Browser Mode webviews and third-party content keep working untouched.
  installSessionSecurityHandlers(session.defaultSession);
  // Initialize foundation systems before everything else
  featureFlagManager.initialize();
  // Push flag changes to all renderer windows
  featureFlagManager.onChange((key, value) => {
    BrowserWindow.getAllWindows().forEach(w => {
      if (!w.isDestroyed()) w.webContents.send('featureFlags:changed', { key, value });
    });
  });

  // Apply startup-on-login from persisted prefs
  const prefs = persistedState.get('prefs');
  if (isMac) {
    app.setLoginItemSettings({ openAtLogin: prefs.startupOnLogin });
    if (prefs.menuBarMode) app.dock?.hide();
  }

  // Initialize MCP host if enabled
  if (featureFlagManager.get<boolean>('mcp.enabled')) {
    mcpHostManager.initialize();
  }

  // Initialize worker bus if enabled
  if (featureFlagManager.get<boolean>('workers.enabled')) {
    const workerBase = new URL('../main/workers/', import.meta.url);
    workerBus.register('sqlite', new URL('sqlite-worker.js', workerBase), {
      dbPath: join(app.getPath('userData'), 'allternit.db'),
    });
    workerBus.register('search', new URL('transcript-search-worker.js', workerBase), {
      transcriptDir: join(app.getPath('userData'), 'transcripts'),
    });
    workerBus.register('shell-path', new URL('shell-path-worker.js', workerBase));
  }

  initializeAppOnce();
  createTray();
  startExtensionBridge();
  startAcuExtensionRelay();

  // Global hotkey: Cmd+Shift+A → Quick Chat floating window
  const hotkeyRegistered = globalShortcut.register(QUICK_CHAT_HOTKEY, toggleMiniWindow);
  if (!hotkeyRegistered) {
    log.warn(`[Main] Failed to register global hotkey: ${QUICK_CHAT_HOTKEY}`);
  }

  // Global hotkey: ⌘/Ctrl+Shift+H (Hermes-style) → toggle floating HUD window
  const hudHotkeyRegistered = globalShortcut.register(HUD_HOTKEY, toggleHudWindow);
  if (hudHotkeyRegistered) {
    log.info(`[Main] Registered global HUD hotkey: ${HUD_HOTKEY}`);
  } else {
    log.warn(`[Main] Failed to register global hotkey: ${HUD_HOTKEY}. On macOS this usually means Allternit needs Accessibility permission in System Settings > Privacy & Security > Accessibility.`);
  }

  // Fallback global hotkey for users who expect the earlier Alt+Shift+H binding.
  const hudHotkeyFallbackRegistered = globalShortcut.register(HUD_HOTKEY_FALLBACK, toggleHudWindow);
  if (hudHotkeyFallbackRegistered) {
    log.info(`[Main] Registered fallback global HUD hotkey: ${HUD_HOTKEY_FALLBACK}`);
  }

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      initializeAppOnce();
    } else {
      showOrReopenMainWindow();
    }
  });

  // Refresh tray every 10s
  setInterval(updateTrayMenu, 10000);

  // ── Permission re-check on app activation ────────────────────────────────
  // macOS users can revoke permissions in System Settings at any time.
  // Silently re-check when the app becomes active and push updates to renderer.
  app.on('did-become-active', async () => {
    if (!mainWindow || mainWindow.isDestroyed()) return;
    const status = await checkPermissions();
    const last = store.get('permissions')?.lastStatus;
    const changed = !last || last.accessibility !== status.accessibility || last.screenRecording !== status.screenRecording;
    if (changed) {
      store.set('permissions.lastStatus', { ...status, checkedAt: new Date().toISOString() });
      mainWindow.webContents.send('permission-guide:status', status);
      log.info('[Main] Permission status changed on activation:', status);
    }
  });
}).catch((error) => {
  log.error('[Main] Startup failed:', error);
  if (error instanceof Error && error.message === 'Authentication cancelled') {
    app.quit();
    return;
  }
  dialog.showErrorBox('Allternit Desktop Startup Error', error instanceof Error ? error.message : 'Unknown startup failure');
  app.quit();
});

/**
 * Set once the app's own startup or main window exists. Before that the only
 * window can be the hidden Clerk sign-in window, and its timing out must not
 * quit the app (it did: a launch that verifies a staged runtime package takes
 * long enough for it to close, and the app quit on every start).
 */
let appWindowOpened = false;

app.on('window-all-closed', () => {
  if (!isMac && appWindowOpened) app.quit();
});

async function handoffInFlightToCloud(): Promise<void> {
  const headers = backendManager.getLocalAuthHeaders();
  const api = backendManager.getUrl();
  try {
    const ensureClerk = await authManager.getClerkToken(FULL_CLERK_REFRESH_WAIT_MS);
    if (ensureClerk) {
      const ensure = await fetch(`${URLS.CLOUD_API}/api/v1/continuation/ensure`, {
        method: 'POST',
        headers: { authorization: `Bearer ${ensureClerk}`, 'content-type': 'application/json' },
      });
      if (!ensure.ok) {
        log.error(`[Main] continuation ensure failed: ${ensure.status} ${await ensure.text().then((t) => t.slice(0, 300))}`);
      }
    }
    const res = await fetch(`${api}/api/v1/fabric/transport/continuation/handoff-all`, {
      method: 'POST',
      headers: { ...headers, 'content-type': 'application/json' },
    });
    const raw = await res.text();
    if (!res.ok) {
      log.error(`[Main] cloud continuation handoff failed: ${res.status} ${raw.slice(0, 400)}`);
      return;
    }
    const body = JSON.parse(raw) as {
      jobs?: Array<{ job_id: string; envelope?: unknown }>;
      files?: unknown[];
      forwarded?: number;
      target?: string;
    };
    const clerk = ensureClerk ?? (await authManager.getClerkToken(FULL_CLERK_REFRESH_WAIT_MS));
    let relayed = 0;
    if (clerk && body.jobs?.length) {
      for (const job of body.jobs) {
        if (!job.envelope) continue;
        const ingest = await fetch(`${URLS.CLOUD_API}/api/v1/fabric/transport/continuation/ingest`, {
          method: 'POST',
          headers: { authorization: `Bearer ${clerk}`, 'content-type': 'application/json' },
          body: JSON.stringify({ envelope: job.envelope, files: body.files ?? [] }),
        });
        if (ingest.ok) relayed += 1;
        else log.error(`[Main] continuation ingest ${ingest.status}: ${(await ingest.text()).slice(0, 200)}`);
      }
    }
    log.info(
      `[Main] cloud continuation local-forwarded ${body.forwarded ?? 0}, cloud-api relayed ${relayed}/${body.jobs?.length ?? 0}` +
        (body.target ? ` url=${body.target}` : ''),
    );
  } catch (err) {
    log.error('[Main] cloud continuation handoff failed', err);
  }
}

// Electron does not await before-quit, so an async handler that awaits
// before reaping lets the process exit with gizzi-code still running
// (spawned detached, reparented to launchd). Hold the first quit, run the
// full cleanup under a deadline, then quit for real.
const QUIT_CLEANUP_DEADLINE_MS = 10_000;
let quitCleanup: Promise<void> | null = null;
let quitCleanupDone = false;

function runQuitCleanup(): Promise<void> {
  quitCleanup ??= Promise.race([
    shutdownAllServices().catch((err) => log.error('[Main] quit cleanup failed', err)),
    new Promise<void>((resolve) => setTimeout(() => {
      log.warn(`[Main] quit cleanup exceeded ${QUIT_CLEANUP_DEADLINE_MS}ms; quitting anyway`);
      resolve();
    }, QUIT_CLEANUP_DEADLINE_MS)),
  ]).finally(() => {
    // Idempotent: guarantees the reap even if the deadline won the race.
    gizziManager.stop({ reapExternal: true });
    quitCleanupDone = true;
  });
  return quitCleanup;
}

app.on('before-quit', (event) => {
  if (quitCleanupDone) return;
  event.preventDefault();
  (app as unknown as { isQuitting?: boolean }).isQuitting = true;
  void runQuitCleanup().then(() => app.quit());
});

app.on('will-quit', () => {
  // Last line of defence for exits that skipped before-quit's cleanup.
  gizziManager.stop({ reapExternal: true });
});

async function shutdownAllServices(): Promise<void> {
  await handoffInFlightToCloud();
  gizziManager.stop({ reapExternal: true });
  try {
    gizziDaemonManager.stopSync();
  } catch (err) {
    log.warn('[Main] gizzi daemon stop on quit failed', err);
  }

  if (app.isReady()) {
    globalShortcut.unregisterAll();
  }
  // SIGTERM first — the worker finishes its in-flight claim and releases the
  // lease path via the sweeper. Must precede the API shutdown it claims from.
  await fabricWorkerManager.stop();
  persistedState.flush();
  featureFlagManager.destroy();
  mcpHostManager.shutdown();
  await workerBus.shutdown();
  tunnelManager.stop();
  await backendManager.stopBackend();
  await localEngineManager.stop();
  connectorSidecarManager.stop();
  officeEngineManager.stop();
  factoryEngineManager.stop();
  meshManager.stop().catch(() => {}); // best-effort mesh sidecar shutdown
  notebookManager.stop();
  voiceManager.stop();
  bonsaiCompanion.stop();
  systemOne.stop();
  computerUseDriverManager.stop();
  acuGatewayManager.stop();
  phoneRemoteManager.stop();
  phoneService.shutdown();
  stopVM().catch(() => {}); // best-effort Lima VM shutdown
  // Remove dev session credentials file so stale credentials don't persist across restarts
  if (isDev) {
    const sessionFile = join(os.homedir(), '.allternit', 'gizzi-dev-session.json');
    fs.promises.unlink(sessionFile).catch(() => {});
  }
  // Ensure any floating permission-guide overlay is torn down before quit
  dismissGuide();
}

// ============================================================================
// IPC Handlers
// ============================================================================

// SDK — exposes the resolved backend URL so the renderer can init createAllternitClient()
ipcMain.handle('sdk:get-backend-url', () => activeBackendUrl);

// Voice call-mode native dictation bridge
voiceManager.registerIpcHandlers();

// Backend management
ipcMain.handle('backend:get-status', () => backendManager.getStatus());

// Engine status surface (consumer-packaged Cowork P1): one aggregate
// green/yellow/red indicator over API / gizzi / fabric worker / office engine.
ipcMain.handle('engines:get-status', async () => getEngineStatus());
fabricWorkerManager.onStateChange((state: FabricWorkerState) => {
  serviceState.fabricWorker =
    state.status === 'stopped'
      ? serviceState.fabricWorker.status === 'pending'
        ? { status: 'pending', detail: 'Waiting…' }
        : { status: 'down', detail: state.detail }
      : { status: state.status, detail: state.detail };
  pushServiceState();
  void pushEngineStatus();
});

// Folder-grant wizard bridges (startup window preload → main).
ipcMain.handle('startup:pick-folder', async () => {
  if (!splashWindow || splashWindow.isDestroyed()) return null;
  const result = await dialog.showOpenDialog(splashWindow, {
    title: 'Grant a folder to Allternit Cowork',
    properties: ['openDirectory', 'createDirectory'],
  });
  return result.canceled || result.filePaths.length === 0 ? null : result.filePaths[0];
});
ipcMain.handle('startup:save-folders', async (_event, folders: unknown) => {
  const list = Array.isArray(folders) ? folders.filter((f): f is string => typeof f === 'string') : [];
  const headers = { ...backendManager.getLocalAuthHeaders(), 'content-type': 'application/json' };
  const res = await fetch(`${URLS.API}/api/v1/cowork-preferences`, {
    method: 'PUT',
    headers,
    body: JSON.stringify({ trusted_folders: list }),
  });
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new Error(`saving trusted folders failed: ${res.status} ${body.slice(0, 200)}`);
  }
  sendToSplash('folders:hide');
  folderGrantsPending = false;
  folderGrantResolver?.();
  return { saved: list.length };
});
handleGuarded('backend:restart', async () => {
  await backendManager.stopBackend();

  await computerUseDriverManager.start();
  return backendManager.ensureBackend({
    extraEnv: {
      ...computerUseDriverManager.getLaunchEnvironment(),
      ...authManager.getPlatformEncryptionEnvironment(),
      ...authManager.getConnectorSidecarEnvironment(),
    },
  });
});

ipcMain.handle('computer-use-driver:get-status', () => computerUseDriverManager.getStatus());

// Phones (Lane 2a): Android over Wi-Fi, no cable — see phone-device-manager.ts
registerPhoneIpc(handleGuarded);

// Bonsai local image companion (install / lifecycle / removal)
ipcMain.handle('bonsai:get-status', () => bonsaiCompanion.getStatus());
handleGuarded('bonsai:install', () => bonsaiCompanion.install());
handleGuarded('bonsai:cancel-install', () => bonsaiCompanion.cancelInstall());
handleGuarded('bonsai:start', () => bonsaiCompanion.start());
handleGuarded('bonsai:stop', () => { bonsaiCompanion.stop(); return true; });
handleGuarded('bonsai:remove', () => bonsaiCompanion.remove());

// System One (S1) + Laya (install / repair / lifecycle / checkpoint swap)
ipcMain.handle('system-one:get-status', () => systemOne.getStatus());
handleGuarded('system-one:install', () => systemOne.install());
handleGuarded('system-one:cancel-install', () => systemOne.cancelInstall());
handleGuarded('system-one:repair', () => systemOne.repair());
handleGuarded('system-one:remove', () => systemOne.remove());
handleGuarded('system-one:start', () => systemOne.startWithApp());
handleGuarded('system-one:stop', () => { systemOne.stop(); return true; });
handleGuarded('system-one:set-checkpoint', (_event, checkpoint: { revision?: string; path?: string } | null) =>
  systemOne.setCheckpoint(checkpoint));
handleGuarded('system-one:set-shadow-state', (_event, enabled: unknown) =>
  systemOne.setShadowState(enabled === true));
handleGuarded('system-one:set-live-mode', (_event, enabled: unknown) =>
  systemOne.setLiveMode(enabled === true));

// Research backend (notebook engine) — lazy start
ipcMain.handle('research:get-status', () => notebookManager.getStatus());
handleGuarded('research:start', async () => {
  const result = await notebookManager.start();
  serviceState.research = result
    ? { status: 'up', detail: `Connected on ${notebookUrl()}` }
    : { status: 'down', detail: 'Failed to start' };
  pushServiceState();
  return result;
});
handleGuarded('research:stop', () => {
  notebookManager.stop();
  serviceState.research = { status: 'down', detail: 'Stopped' };
  pushServiceState();
});

// Store
ipcMain.handle('store:get', (_event, key: keyof StoreSchema) => store.get(key));
handleGuarded('store:set', (_event, key: keyof StoreSchema, value: unknown) => {
  store.set(key, value);
});

// App info
function readBuildInfo(): Record<string, unknown> | null {
  try {
    const raw = fs.readFileSync(join(process.resourcesPath, 'build-info.json'), 'utf8');
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' ? parsed : null;
  } catch {
    return null;
  }
}
ipcMain.handle('app:get-info', () => ({
  version: app.getVersion(),
  platform: process.platform,
  isPackaged: app.isPackaged,
  manifest: PLATFORM_MANIFEST,
  buildInfo: readBuildInfo(),
}));

// Auto-update control plane (renderer observes status via app:update-status)
ipcMain.handle('app:check-for-updates', async () => {
  if (!app.isPackaged) {
    log.info('[autoUpdater] Skipping update check in unpackaged dev build');
    return { ok: false, reason: 'dev-build' } as const;
  }
  try {
    autoUpdater.checkForUpdates();
    return { ok: true } as const;
  } catch (error) {
    log.error('[autoUpdater] Manual check failed:', error);
    return { ok: false, reason: 'check-failed', message: String(error) } as const;
  }
});
handleGuarded('app:install-update', async () => {
  // Clean up first so before-quit does not cancel the updater's own quit.
  await runQuitCleanup();
  if (!appUpdateDownloaded && runtimePackages().hasPendingUpdate()) {
    // Only the runtime changed: a restart promotes the staged package.
    app.relaunch();
    app.exit(0);
    return;
  }
  autoUpdater.quitAndInstall();
});
// Preload uses sendSync at module load; handle() only answers invoke().
ipcMain.on('app:get-platform-url', (event) => {
  event.returnValue = {
    platformUrl: activePlatformUrl,
    gatewayUrl: URLS.CLOUD_API,
  };
});

  // Shell
handleGuarded('shell:open-external', (_event, url: string) => {
  openExternalAllowlisted(url);
});
function openDesignStudio(prompt?: string | null): void {
  const target = new URL('/design', activePlatformUrl);
  if (prompt) target.searchParams.set('prompt', prompt);
  const targetUrl = target.toString();
  if (designWindow && !designWindow.isDestroyed()) {
    void designWindow.loadURL(targetUrl);
    designWindow.show();
    designWindow.focus();
    return;
  }

  designWindow = new BrowserWindow({
    width: 1440,
    height: 960,
    minWidth: 960,
    minHeight: 640,
    title: 'Allternit Design',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 16 },
    show: false,
    backgroundColor: '#0F0C0A',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      // Same editors as the main window's pane: keep agent edits running
      // while this window is in the background.
      backgroundThrottling: false,
    },
  });

  installWillNavigateGuard(designWindow.webContents);

  designWindow.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalAllowlisted(url);
    return { action: 'deny' };
  });
  designWindow.once('ready-to-show', () => designWindow?.show());
  designWindow.on('closed', () => { designWindow = null; });
  void designWindow.loadURL(targetUrl);
}

// Approves a terminal's `gizzi login` (see handleProtocolCallback): a native
// confirmation, then the account's Clerk token approves it; the hosted page
// in the browser when there is no Clerk token (see pairing-approval.ts).
function openPairingApproval(code: string): void {
  const session = authManager.getSessionSnapshot();
  void approvePairing(code, {
    cloudApiBase: process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API,
    hostedPairUrl: (c) => new URL(`/pair?code=${encodeURIComponent(c)}`, URLS.PLATFORM).toString(),
    getClerkToken: () => authManager.getClerkToken(FULL_CLERK_REFRESH_WAIT_MS),
    fetch: (input, init) => fetch(input, init),
    confirm: async (request) => {
      const parent = BrowserWindow.getFocusedWindow() ?? BrowserWindow.getAllWindows().find((w) => !w.isDestroyed() && w.isVisible());
      if (parent) {
        if (parent.isMinimized()) parent.restore();
        parent.show();
        parent.focus();
      }
      if (isMac) app.focus({ steal: true });
      const options = {
        type: 'question' as const,
        buttons: ['Approve', 'Cancel'],
        defaultId: 0,
        cancelId: 1,
        message: 'Sign in gizzi on this Mac?',
        detail: describePairingRequest(request),
      };
      const { response } = parent
        ? await dialog.showMessageBox(parent, options)
        : await dialog.showMessageBox(options);
      return response === 0;
    },
    openBrowser: (url) => { openExternalAllowlisted(url); },
    notify: (title, body) => {
      if (Notification.isSupported()) new Notification({ title, body }).show();
      else log.info(`[Pairing] ${title}: ${body}`);
    },
    account: { email: session?.userEmail },
  }).then((outcome) => log.info(`[Pairing] gizzi login ${code}: ${outcome}`));
}

ipcMain.handle('shell:open-design', () => {
  openDesignStudio();
});

// The Allternit Office window — the ACI rail's bottom-tab surface. Same
// window profile as Design above (the popped-out office suite host).
ipcMain.handle('shell:open-office-window', () => {
  if (officeWindow && !officeWindow.isDestroyed()) {
    void officeWindow.loadURL(new URL('/office', activePlatformUrl).toString());
    officeWindow.show();
    officeWindow.focus();
    return;
  }

  officeWindow = new BrowserWindow({
    width: 1440,
    height: 960,
    minWidth: 960,
    minHeight: 640,
    title: 'Allternit Office',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 16 },
    show: false,
    backgroundColor: '#0F0C0A',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      // Same editors as the main window's pane: keep agent edits running
      // while this window is in the background.
      backgroundThrottling: false,
    },
  });

  installWillNavigateGuard(officeWindow.webContents);

  officeWindow.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalAllowlisted(url);
    return { action: 'deny' };
  });
  officeWindow.once('ready-to-show', () => officeWindow?.show());
  officeWindow.on('closed', () => { officeWindow = null; });
  void officeWindow.loadURL(new URL('/office', activePlatformUrl).toString());
});

// HUD mode defaults — a chrome-free floating panel anchored near the bottom
// of the primary display, inspired by Hermes Desktop's HUD windowing profile.
// The default shape is a wide, short bar so the composer dominates, matching
// Hermes' 620×320 bottom-band layout.
const HUD_WIDTH = 620;
const HUD_HEIGHT = 220;
const HUD_BOTTOM_MARGIN = 72;
let hudCompanionAgentId: string | null | undefined;
function showHudWindow(): void {
  if (!hudWindow || hudWindow.isDestroyed()) return;
  hudWindow.show();
  hudWindow.focus();
  hudWindow.moveTop();
  if (hudCompanionAgentId !== undefined) desktopCompanion.attachPetToHud(hudWindow.getBounds());
  pushHudState();
}

function hideHudWindow(): void {
  if (hudWindow && !hudWindow.isDestroyed()) hudWindow.hide();
  desktopCompanion.detachPetFromHud();
  pushHudState();
}

function hudUrl(): string {
  const url = new URL('/hud', activePlatformUrl);
  if (hudCompanionAgentId !== undefined) url.searchParams.set('companion', hudCompanionAgentId ?? 'first');
  return url.toString();
}

function openHudForCompanion(agentId: string | null): void {
  if (hudCompanionAgentId === agentId && hudWindow && !hudWindow.isDestroyed() && hudWindow.isVisible()) {
    hideHudWindow();
    return;
  }
  const changed = hudCompanionAgentId !== agentId;
  hudCompanionAgentId = agentId;
  if (changed && hudWindow && !hudWindow.isDestroyed()) void hudWindow.loadURL(hudUrl());
  openHudWindow();
}

function computeHudBounds() {
  const { width: screenW, height: screenH } = screen.getPrimaryDisplay().workAreaSize;
  return {
    width: Math.min(HUD_WIDTH, screenW),
    height: Math.min(HUD_HEIGHT, screenH),
    x: Math.round((screenW - Math.min(HUD_WIDTH, screenW)) / 2),
    y: Math.round(Math.max(0, screenH - Math.min(HUD_HEIGHT, screenH) - HUD_BOTTOM_MARGIN)),
  };
}

/** Broadcast the HUD's open/closed state and active session to the main window. */
function pushHudState() {
  const open = hudWindow !== null && !hudWindow.isDestroyed() && hudWindow.isVisible();
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send('shell:hud:state', { open, sessionId: hudSessionId });
  }
}

function createHudWindow(): BrowserWindow {
  const win = new BrowserWindow({
    ...computeHudBounds(),
    minWidth: 380,
    minHeight: 160,
    maxWidth: 1600,
    maxHeight: 1000,
    title: 'Allternit HUD',
    frame: false,
    transparent: true,
    // Keep resizable false so a transparent frameless window does not expose a
    // system-level edge-resize hot-zone (Windows grows a few px per drag when
    // this is true).  Resizing is done by the renderer's corner handles if we
    // add them later; dragging is handled by the renderer drag handle + IPC.
    resizable: false,
    movable: true,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    skipTaskbar: !isMac,
    hasShadow: false,
    alwaysOnTop: true,
    // NSPanel on macOS keeps the HUD out of the cmd-tab anchor and lets it
    // float above fullscreen apps like Hermes' HUD.
    type: isMac ? 'panel' : undefined,
    roundedCorners: true,
    visualEffectState: 'active',
    hiddenInMissionControl: isMac,
    // Show immediately so the panel gains a rendering surface. When hidden,
    // ready-to-show can be delayed (especially under Playwright), leaving the
    // HUD inaccessible to automation. The transparent background keeps any
    // pre-content flash invisible.
    show: true,
    backgroundColor: '#00000000',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  installWillNavigateGuard(win.webContents);

  // Ensure the panel floats above normal windows and appears on all macOS
  // Spaces while fullscreen apps are running.
  win.setAlwaysOnTop(true, isMac ? 'floating' : 'screen-saver');
  try {
    win.setVisibleOnAllWorkspaces?.(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
  } catch {
    // Best effort — not supported on every platform/configuration.
  }

  return win;
}

function openHudWindow(): void {
  log.info('[HUD] openHudWindow called', { activePlatformUrl });
  if (hudWindow && !hudWindow.isDestroyed()) {
    log.info('[HUD] Reusing existing HUD window:', hudWindow.webContents.getURL());
    showHudWindow();
    return;
  }

  log.info('[HUD] Creating new floating HUD window');
  hudWindow = createHudWindow();
  showHudWindow();

  log.info('[HUD] HUD window created', { id: hudWindow.id, bounds: hudWindow.getBounds(), visible: hudWindow.isVisible() });
  hudWindow.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalAllowlisted(url);
    return { action: 'deny' };
  });
  hudWindow.webContents.on('did-fail-load', (_event, errorCode, errorDescription) => {
    log.error('[HUD] Failed to load HUD window:', errorCode, errorDescription);
  });
  hudWindow.webContents.on('did-finish-load', () => {
    log.info('[HUD] HUD window finished loading', { url: hudWindow?.webContents.getURL(), title: hudWindow?.getTitle() });
  });
  hudWindow.webContents.on('did-navigate', (_event, url) => {
    log.info('[HUD] HUD window did-navigate:', url);
  });
  hudWindow.once('ready-to-show', () => {
    log.info('[HUD] HUD window ready-to-show');
    if (hudWindow?.isVisible()) showHudWindow();
  });
  hudWindow.on('closed', () => {
    log.info('[HUD] HUD window closed');
    if (annotationWindow && !annotationWindow.isDestroyed()) {
      annotationWindow.close();
    }
    hudWindow = null;
    hideHudWindow();
  });
  hudWindow.on('move', () => { if (hudCompanionAgentId !== undefined && hudWindow) desktopCompanion.attachPetToHud(hudWindow.getBounds()); });
  hudWindow.on('resize', () => { if (hudCompanionAgentId !== undefined && hudWindow) desktopCompanion.attachPetToHud(hudWindow.getBounds()); });
  log.info('[HUD] Loading HUD window URL:', hudUrl());
  void hudWindow.loadURL(hudUrl());
}

function toggleHudWindow(): void {
  log.info('[HUD] toggleHudWindow called');
  if (hudWindow && !hudWindow.isDestroyed()) {
    if (hudWindow.isVisible() && hudWindow.isFocused()) {
      log.info('[HUD] HUD is visible and focused — hiding');
      hideHudWindow();
      return;
    }
    log.info('[HUD] HUD exists — showing and focusing');
    showHudWindow();
    return;
  }
  openHudWindow();
}

ipcMain.handle('shell:open-hud', () => { hudCompanionAgentId = undefined; openHudWindow(); });
ipcMain.handle('shell:close-hud', () => {
  // Hide, not close: the HUD is a persistent panel and closing it would tear
  // down its webContents and lose composer state.
  if (hudWindow && !hudWindow.isDestroyed()) {
    hideHudWindow();
  }
});
ipcMain.handle('shell:toggle-hud', toggleHudWindow);
ipcMain.handle('shell:show-hud', () => {
  if (hudWindow && !hudWindow.isDestroyed()) {
    showHudWindow();
  } else {
    openHudWindow();
  }
});
let hudMoveOrigin: { cursorX: number; cursorY: number; x: number; y: number; width: number; height: number } | null = null;
ipcMain.handle('shell:hud:begin-move', () => {
  if (!hudWindow || hudWindow.isDestroyed()) return;
  const cursor = screen.getCursorScreenPoint();
  const bounds = hudWindow.getBounds();
  hudMoveOrigin = { cursorX: cursor.x, cursorY: cursor.y, ...bounds };
  hudWindow.setIgnoreMouseEvents(false);
});
ipcMain.handle('shell:hud:end-move', () => { hudMoveOrigin = null; });
ipcMain.handle('shell:move-hud', (_event, delta: { dx?: number; dy?: number; x?: number; y?: number; width?: number; height?: number }) => {
  // Two renderer call sites send different shapes: HudApp's drag handler
  // sends {dx, dy}; composer-drag sends {x, y, width, height} deltas. Accept
  // both, and only apply a size change when width/height are provided.
  if (!hudWindow || hudWindow.isDestroyed()) return;
  const cursor = hudMoveOrigin ? screen.getCursorScreenPoint() : null;
  const dx = cursor && hudMoveOrigin ? cursor.x - hudMoveOrigin.cursorX : Number(delta?.dx ?? delta?.x ?? 0);
  const dy = cursor && hudMoveOrigin ? cursor.y - hudMoveOrigin.cursorY : Number(delta?.dy ?? delta?.y ?? 0);
  const [currentWidth, currentHeight] = hudWindow.getSize();
  const width = delta?.width !== undefined ? Number(delta.width) : currentWidth;
  const height = delta?.height !== undefined ? Number(delta.height) : currentHeight;
  if (![dx, dy, width, height].every(Number.isFinite)) return;
  const [x, y] = hudMoveOrigin ? [hudMoveOrigin.x, hudMoveOrigin.y] : hudWindow.getPosition();
  // setBounds (not setPosition) keeps a transparent frameless window from
  // drifting on Windows per Electron frameless-transparent quirks.
  const wasResizable = hudWindow.isResizable();
  if (!wasResizable) hudWindow.setResizable(true);
  try {
    hudWindow.setBounds({
      x: Math.round(x + dx),
      y: Math.round(y + dy),
      width: Math.max(380, Math.round(width)),
      height: Math.max(160, Math.round(height)),
    });
  } finally {
    if (!wasResizable && !hudWindow.isDestroyed()) hudWindow.setResizable(false);
  }
});
ipcMain.handle('shell:resize-hud', (_event, bounds: { height: number }) => {
  if (!hudWindow || hudWindow.isDestroyed()) return;
  const requestedHeight = Math.max(160, Math.round(Number(bounds?.height ?? HUD_HEIGHT)));
  const [x, y] = hudWindow.getPosition();
  const [width] = hudWindow.getSize();
  const currentBottom = y + hudWindow.getSize()[1];
  // Keep the window's bottom edge anchored so it grows/collapses upward,
  // mirroring the Hermes HUD expansion behavior.
  const newY = Math.max(0, Math.round(currentBottom - requestedHeight));
  const newHeight = Math.min(1000, Math.max(160, Math.round(currentBottom - newY)));
  const wasResizable = hudWindow.isResizable();
  if (!wasResizable) hudWindow.setResizable(true);
  try {
    hudWindow.setBounds({ x, y: newY, width, height: newHeight });
  } finally {
    if (!wasResizable && !hudWindow.isDestroyed()) hudWindow.setResizable(false);
  }
});

ipcMain.handle('shell:set-hud-bounds', (_event, bounds: { x?: number; y?: number; width?: number; height?: number }) => {
  if (!hudWindow || hudWindow.isDestroyed()) return;
  const current = hudWindow.getBounds();
  const next = {
    x: Number.isFinite(bounds?.x) ? Math.round(bounds.x!) : current.x,
    y: Number.isFinite(bounds?.y) ? Math.round(bounds.y!) : current.y,
    width: Number.isFinite(bounds?.width) ? Math.max(380, Math.round(bounds.width!)) : current.width,
    height: Number.isFinite(bounds?.height) ? Math.max(160, Math.min(1000, Math.round(bounds.height!))) : current.height,
  };
  const wasResizable = hudWindow.isResizable();
  if (!wasResizable) hudWindow.setResizable(true);
  try {
    hudWindow.setBounds(next);
  } finally {
    if (!wasResizable && !hudWindow.isDestroyed()) hudWindow.setResizable(false);
  }
});

ipcMain.handle('shell:hud:frost', (_event, showing: boolean) => {
  if (!hudWindow || hudWindow.isDestroyed()) return { ok: true };
  try {
    if (process.platform === 'darwin') {
      hudWindow.setVibrancy(showing ? 'hud' : null);
    }
    // Windows/Linux: no native vibrancy equivalent exposed here; the CSS scrim
    // in the renderer still provides the visual treatment.
    return { ok: true };
  } catch (error) {
    log.warn('[HUD] Failed to set frost:', error);
    return { ok: false };
  }
});

ipcMain.handle('shell:hud:reset-layout', () => {
  if (!hudWindow || hudWindow.isDestroyed()) return { ok: true };
  const wasResizable = hudWindow.isResizable();
  if (!wasResizable) hudWindow.setResizable(true);
  try {
    hudWindow.setBounds(computeHudBounds());
  } finally {
    if (!wasResizable && !hudWindow.isDestroyed()) hudWindow.setResizable(false);
  }
  return { ok: true };
});

ipcMain.on('shell:hud:ignore-mouse', (_event, ignore: boolean) => {
  if (!hudWindow || hudWindow.isDestroyed()) return;
  try {
    hudWindow.setIgnoreMouseEvents(Boolean(ignore), { forward: true });
  } catch (error) {
    log.warn('[HUD] Failed to set ignore mouse events:', error);
  }
});

ipcMain.on('shell:hud:workspace-transfer', (_event, transferring: boolean) => {
  if (!hudWindow || hudWindow.isDestroyed()) return;
  try {
    hudWindow.setVisibleOnAllWorkspaces?.(Boolean(transferring), { visibleOnFullScreen: true, skipTransformProcessType: true });
  } catch (error) {
    log.warn('[HUD] Failed to set workspace transfer:', error);
  }
});

ipcMain.on('shell:hud:session', (_event, sessionId: string | null) => {
  hudSessionId = typeof sessionId === 'string' ? sessionId : null;
  pushHudState();
});

ipcMain.on('device:info', (event) => {
  // Sync IPC must always set returnValue or the renderer blocks.
  try {
    assertTrustedSender(event, 'device:info');
    event.returnValue = coworkDevice;
  } catch {
    event.returnValue = null;
  }
});

/**
 * Settings → Cowork devices: record this computer as trusted. Only main holds
 * the local API's spawn-time secret (backend-manager), and the API trusts a
 * desktop registration only when that secret is present, so a renderer
 * request can't do it. Runs once the backend is up and again from the
 * renderer after sign-in (`device:register`).
 */
async function registerThisComputer(): Promise<boolean> {
  const session = authManager.getSessionSnapshot();
  if (!session || !backendManager.getDesktopAccessToken()) return false;
  try {
    const response = await fetch(`${URLS.API}/api/v1/cowork/devices/register`, {
      method: 'POST',
      headers: {
        ...backendManager.getLocalAuthHeaders(session.userId),
        'x-allternit-user-email': session.userEmail,
        ...coworkDeviceHeaders(coworkDevice),
        'content-type': 'application/json',
      },
      body: JSON.stringify(coworkDevice),
    });
    if (!response.ok) log.warn(`[Cowork] Registering this computer failed (${response.status})`);
    // The same Mac as a computer (ACI P4): sessions show it as their
    // computer, and control / hand-offs apply to it like any other.
    const computer = await fetch(`${URLS.API}/api/v1/computers/this-device`, {
      method: 'POST',
      headers: {
        ...backendManager.getLocalAuthHeaders(session.userId),
        'x-allternit-user-email': session.userEmail,
        'content-type': 'application/json',
      },
      body: JSON.stringify(coworkDevice),
    });
    if (!computer.ok) log.warn(`[Computers] Registering this computer failed (${computer.status})`);
    return response.ok && computer.ok;
  } catch (error) {
    log.warn('[Cowork] Registering this computer failed:', error);
    return false;
  }
}

handleGuarded('device:register', () => registerThisComputer());

handleGuarded('links:set-open-in-app', (_event, enabled: unknown) => {
  openLinksInApp = enabled === true;
  store.set('openLinksInApp', openLinksInApp);
});

ipcMain.on('shell:hud:windowing', (event) => {
  event.returnValue = {
    clientPlacement: true,
    controlDrag: false,
    nativeDrag: false,
    workspaceTransfer: false,
  };
});

/** Broadcast the annotation overlay's open/closed state to the HUD window. */
function pushAnnotationState() {
  const open = annotationWindow !== null && !annotationWindow.isDestroyed() && annotationWindow.isVisible();
  hudWindow?.webContents.send('shell:hud:annotation:state', { open });
}

function createAnnotationWindow(): BrowserWindow {
  const hudBounds = hudWindow && !hudWindow.isDestroyed() ? hudWindow.getBounds() : undefined;
  const display = hudBounds
    ? screen.getDisplayNearestPoint({ x: hudBounds.x + hudBounds.width / 2, y: hudBounds.y + hudBounds.height / 2 })
    : screen.getPrimaryDisplay();
  const { x, y, width, height } = display.bounds;

  const win = new BrowserWindow({
    x,
    y,
    width,
    height,
    title: 'Allternit Annotation',
    frame: false,
    transparent: true,
    resizable: false,
    movable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    skipTaskbar: !isMac,
    hasShadow: false,
    alwaysOnTop: true,
    type: isMac ? 'panel' : undefined,
    roundedCorners: false,
    visualEffectState: 'active',
    hiddenInMissionControl: isMac,
    show: true,
    backgroundColor: '#00000000',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  installWillNavigateGuard(win.webContents);

  win.setAlwaysOnTop(true, isMac ? 'floating' : 'screen-saver');
  try {
    win.setVisibleOnAllWorkspaces?.(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
  } catch {
    // Best effort — not supported on every platform/configuration.
  }

  return win;
}

function openAnnotationWindow(): void {
  if (annotationWindow && !annotationWindow.isDestroyed()) {
    annotationWindow.show();
    annotationWindow.focus();
    annotationWindow.moveTop();
    pushAnnotationState();
    return;
  }

  annotationWindow = createAnnotationWindow();

  annotationWindow.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalAllowlisted(url);
    return { action: 'deny' };
  });
  annotationWindow.on('closed', () => {
    annotationWindow = null;
    pushAnnotationState();
  });
  annotationWindow.once('ready-to-show', () => {
    annotationWindow?.show();
    annotationWindow?.focus();
    annotationWindow?.moveTop();
    pushAnnotationState();
  });

  const url = new URL('/hud/annotate', activePlatformUrl).toString();
  log.info('[Annotation] Loading annotation window URL:', url);
  void annotationWindow.loadURL(url);
}

ipcMain.handle('shell:hud:annotation:open', openAnnotationWindow);
ipcMain.handle('shell:hud:annotation:close', () => {
  if (annotationWindow && !annotationWindow.isDestroyed()) {
    annotationWindow.close();
  }
});
ipcMain.on('shell:hud:annotation:clear', () => {
  annotationWindow?.webContents.send('shell:hud:annotation:clear');
});
handleGuarded('shell:hud:annotation:save', async (_event, base64Png: string) => {
  if (!base64Png || typeof base64Png !== 'string') {
    return { success: false, error: 'No image data provided' };
  }
  const saveOptions = {
    title: 'Save Annotation',
    defaultPath: 'annotation.png',
    filters: [{ name: 'PNG Images', extensions: ['png'] }],
  };
  const result = annotationWindow && !annotationWindow.isDestroyed()
    ? await dialog.showSaveDialog(annotationWindow, saveOptions)
    : await dialog.showSaveDialog(saveOptions);
  if (result.canceled || !result.filePath) {
    return { success: false };
  }
  try {
    const data = base64Png.replace(/^data:image\/png;base64,/, '');
    await fs.promises.writeFile(result.filePath, Buffer.from(data, 'base64'));
    return { success: true, path: result.filePath };
  } catch (error) {
    log.warn('[Annotation] Failed to save image:', error);
    return { success: false, error: String(error) };
  }
});

function openFabricSessionWindow(runtimeId?: string): void {
  if (remoteControlWindow && !remoteControlWindow.isDestroyed()) {
    remoteControlWindow.show();
    remoteControlWindow.focus();
    return;
  }

  remoteControlWindow = new BrowserWindow({
    width: 1280,
    height: 840,
    minWidth: 820,
    minHeight: 560,
    title: 'Allternit Fabric Transport',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 16 },
    show: false,
    backgroundColor: '#FFFFFF',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  installWillNavigateGuard(remoteControlWindow.webContents);

  remoteControlWindow.webContents.setWindowOpenHandler(({ url }) => {
    void openExternalAllowlisted(url);
    return { action: 'deny' };
  });
  remoteControlWindow.once('ready-to-show', () => remoteControlWindow?.show());
  remoteControlWindow.on('closed', () => { remoteControlWindow = null; });
  const dashboardUrl = process.env.ALLTERNIT_FABRIC_SESSION_URL
    ? new URL('/', process.env.ALLTERNIT_FABRIC_SESSION_URL)
    : new URL('/fabric-session.html', activePlatformUrl);
  if (runtimeId) {
    dashboardUrl.searchParams.set('runtime', runtimeId);
  }
  void remoteControlWindow.loadURL(dashboardUrl.toString());
}

ipcMain.handle('shell:open-remote-control', (_event, runtimeId?: string) => {
  openFabricSessionWindow(typeof runtimeId === 'string' ? runtimeId : undefined);
});

ipcMain.handle('shell:open-fabric-session', (_event, runtimeId?: string) => {
  openFabricSessionWindow(typeof runtimeId === 'string' ? runtimeId : undefined);
});

/**
 * Deliver an office payload to the main window's renderer over a preload
 * channel (`office:open-target` / `office:open-file`). The renderer's office
 * desktop bridge routes it into the shell's office surface (in-shell editor
 * view when the shell is mounted, otherwise the editor route).
 *
 * Queues while the main window is missing or still loading; the queue flushes
 * on did-finish-load (registered in createMainWindow). There are no separate
 * office windows anymore — one office surface, one window.
 */
function deliverToMainWindow(channel: string, payload: unknown): void {
  if (!mainWindow || mainWindow.isDestroyed() || mainWindow.webContents.isLoading()) {
    pendingOfficeDeliveries.push({ channel, payload });
    return;
  }
  if (mainWindow.isMinimized()) mainWindow.restore();
  mainWindow.show();
  mainWindow.focus();
  mainWindow.webContents.send(channel, payload);
}

function flushPendingOfficeDeliveries(): void {
  const pending = pendingOfficeDeliveries.splice(0, pendingOfficeDeliveries.length);
  for (const delivery of pending) {
    deliverToMainWindow(delivery.channel, delivery.payload);
  }
}

/** Open an office target in the main window (hub launcher target included). */
function openOfficeTarget(target: OfficeTarget, artifactId?: string): void {
  deliverToMainWindow('office:open-target', { target, artifactId: artifactId ?? null });
}

/**
 * Open a file from a file-association ("Open with Allternit") in its editor.
 * The bytes are delivered to the main window over IPC after load; the
 * web app's office desktop bridge stashes and routes them (file-handoff).
 */
function openOfficeWithFile(filePath: string): void {
  const editor = editorForFile(filePath);
  if (!editor) {
    log.warn('[Office] Unsupported file association:', filePath);
    return;
  }
  let bytes: Buffer;
  try {
    bytes = fs.readFileSync(filePath);
  } catch (error) {
    log.error('[Office] Failed to read associated file:', filePath, error);
    return;
  }
  deliverToMainWindow('office:open-file', { name: basename(filePath), bytes });
}

ipcMain.handle('shell:open-docs', (_event, artifactId?: unknown) => {
  openOfficeTarget('docs', typeof artifactId === 'string' && artifactId ? artifactId : undefined);
});

const openOfficeFromIpc = (target?: unknown, artifactId?: unknown) => {
  openOfficeTarget(
    isOfficeTarget(target) ? target : 'launcher',
    typeof artifactId === 'string' && artifactId ? artifactId : undefined,
  );
};

ipcMain.handle('shell:open-office', (_event, target?: unknown, artifactId?: unknown) => {
  openOfficeFromIpc(target, artifactId);
});

// `ipcMain.handle` registers an invoke-handler, not an EventEmitter listener,
// so tests (and fire-and-forget senders) use the `ipcMain.on` path.
ipcMain.on('shell:open-office', (_event, target?: unknown, artifactId?: unknown) => {
  openOfficeFromIpc(target, artifactId);
});

const codeSessionWindows = new Map<string, BrowserWindow>();
const botComputerWindows = new Map<string, BrowserWindow>();
const botPhoneWindows = new Map<string, BrowserWindow>();

ipcMain.handle('shell:open-bot-phone', (_event, options: { botId: string; title?: string }) => {
  if (!options?.botId) throw new Error('A bot ID is required');
  const existing = botPhoneWindows.get(options.botId);
  if (existing && !existing.isDestroyed()) {
    existing.show();
    existing.focus();
    return;
  }
  const phoneWindow = new BrowserWindow({
    ...BOT_PHONE_WINDOW_SIZE,
    title: options.title || 'Bot phone',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 12 },
    show: false,
    backgroundColor: '#FFFFFF',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  installWillNavigateGuard(phoneWindow.webContents);
  botPhoneWindows.set(options.botId, phoneWindow);
  phoneWindow.once('ready-to-show', () => phoneWindow.show());
  phoneWindow.on('closed', () => { botPhoneWindows.delete(options.botId); });
  phoneWindow.webContents.setWindowOpenHandler(({ url: target }) => {
    void openExternalAllowlisted(target);
    return { action: 'deny' };
  });
  void phoneWindow.loadURL(buildBotPhoneWindowUrl(activePlatformUrl, options.botId));
});

ipcMain.handle('shell:open-session', (_event, options: { sessionId: string; workspaceId?: string; title?: string }) => {
  if (!options?.sessionId) throw new Error('A session ID is required');

  const existing = codeSessionWindows.get(options.sessionId);
  if (existing && !existing.isDestroyed()) {
    existing.show();
    existing.focus();
    return;
  }

  const sessionWindow = new BrowserWindow({
    width: 1180,
    height: 820,
    minWidth: 760,
    minHeight: 560,
    title: options.title || 'Allternit Code Session',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 16 },
    show: false,
    backgroundColor: '#0F0C0A',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webviewTag: true,
    },
  });

  installWillNavigateGuard(sessionWindow.webContents);

  codeSessionWindows.set(options.sessionId, sessionWindow);

  // Use /shell so the detached session query params are consumed by ShellApp.
  // The old /platform path no longer exists as a routed page.
  const url = new URL('/shell', activePlatformUrl);
  url.searchParams.set('detachedSurface', 'code');
  url.searchParams.set('detachedSessionId', options.sessionId);
  if (options.workspaceId) url.searchParams.set('detachedWorkspaceId', options.workspaceId);

  sessionWindow.once('ready-to-show', () => sessionWindow.show());
  sessionWindow.on('closed', () => { codeSessionWindows.delete(options.sessionId); });
  sessionWindow.webContents.setWindowOpenHandler(({ url: target }) => {
    void openExternalAllowlisted(target);
    return { action: 'deny' };
  });
  void sessionWindow.loadURL(url.toString());
});

ipcMain.handle('shell:open-bot-computer', (_event, options: { botId: string; title?: string; sandboxId?: string }) => {
  if (!options?.botId) throw new Error('A bot ID is required');

  const existing = botComputerWindows.get(options.botId);
  if (existing && !existing.isDestroyed()) {
    existing.show();
    existing.focus();
    return;
  }

  const computerWindow = new BrowserWindow({
    width: 1280,
    height: 840,
    minWidth: 820,
    minHeight: 560,
    title: options.title || 'Bot computer',
    titleBarStyle: isMac ? 'hiddenInset' : 'default',
    trafficLightPosition: { x: 16, y: 16 },
    show: false,
    backgroundColor: '#0F0C0A',
    webPreferences: {
      preload: join(__dirname, '../preload/index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      backgroundThrottling: false,
    },
  });

  installWillNavigateGuard(computerWindow.webContents);
  computerWindow.webContents.setBackgroundThrottling(false);
  botComputerWindows.set(options.botId, computerWindow);

  const url = buildBotComputerWindowUrl(activePlatformUrl, options);
  computerWindow.once('ready-to-show', () => computerWindow.show());
  computerWindow.on('closed', () => { botComputerWindows.delete(options.botId); });
  computerWindow.webContents.setWindowOpenHandler(({ url: target }) => {
    void openExternalAllowlisted(target);
    return { action: 'deny' };
  });
  void computerWindow.loadURL(url);
});

ipcMain.handle('shell:get-office-host-status', async () => detectOfficeHostStatus());
ipcMain.handle('office-addins:get-status', async () => {
  const manager = await getOfficeAddinManager();
  return Object.fromEntries((['word', 'excel', 'powerpoint'] as OfficeProductId[]).map((product) => [product, manager.getStatus(product)]));
});
handleGuarded('office-addins:install', async (_event, product: OfficeProductId) => (await getOfficeAddinManager()).install(product));
handleGuarded('office-addins:repair', async (_event, product: OfficeProductId) => (await getOfficeAddinManager()).repair(product));
handleGuarded('office-addins:remove', async (_event, product: OfficeProductId) => (await getOfficeAddinManager()).remove(product));

// Desktop auth
ipcMain.handle('auth:get-session', async () => {
  const session = await authManager.getSession();
  if (!session) {
    return null;
  }

  return {
    userId: session.userId,
    userEmail: session.userEmail,
    expiresAt: session.expiresAt,
    runtimeId: session.runtimeId,
    organizationId: session.organizationId,
    capabilities: session.capabilities,
  };
});
ipcMain.handle('auth:list-accounts', async () => authManager.listAccounts());
handleGuarded('auth:forget-account', async (_event, userId: string) => {
  await authManager.forgetAccount(userId);
});
handleGuarded('auth:sign-out', async () => {
  await authManager.signOut();
  app.relaunch();
  app.quit();
});

// ============================================================================
// IPC: Window Controls
// ============================================================================

ipcMain.handle('window:minimize', () => { mainWindow?.minimize(); });

ipcMain.handle('window:maximize', () => {
  if (!mainWindow) return { maximized: false };
  if (mainWindow.isMaximized()) {
    mainWindow.unmaximize();
    return { maximized: false };
  }
  mainWindow.maximize();
  return { maximized: true };
});

ipcMain.handle('window:close', (event) => {
  log.warn('[Main] window:close invoked by renderer at', event.sender.getURL());
  mainWindow?.close();
});

ipcMain.handle('window:is-maximized', () => mainWindow?.isMaximized() ?? false);

ipcMain.handle('window:fullscreen', (_event, enabled?: boolean) => {
  if (!mainWindow) return { fullscreen: false };
  const next = enabled !== undefined ? enabled : !mainWindow.isFullScreen();
  mainWindow.setFullScreen(next);
  return { fullscreen: next };
});

ipcMain.handle('window:set-always-on-top', (_event, enabled: boolean) => {
  mainWindow?.setAlwaysOnTop(enabled);
});

ipcMain.handle('window:get-state', () => {
  if (!mainWindow) return null;
  return {
    maximized: mainWindow.isMaximized(),
    minimized: mainWindow.isMinimized(),
    fullscreen: mainWindow.isFullScreen(),
    focused: mainWindow.isFocused(),
    bounds: mainWindow.getBounds(),
  };
});

ipcMain.handle('window:get-bounds', () => mainWindow?.getBounds());

ipcMain.handle('window:set-bounds', (_event, bounds: Partial<{ x: number; y: number; width: number; height: number }>) => {
  if (mainWindow && bounds) {
    mainWindow.setBounds({ ...mainWindow.getBounds(), ...bounds });
  }
});

ipcMain.handle('window:center', () => { mainWindow?.center(); });
ipcMain.handle('window:hide', () => { mainWindow?.hide(); });
ipcMain.handle('window:show', () => { showOrReopenMainWindow(); });
ipcMain.handle('window:minimize-to-tray', () => { mainWindow?.hide(); });
ipcMain.on('mini-window:hide', () => { miniWindow?.hide(); });
ipcMain.on('mini-window:toggle', () => toggleMiniWindow());

// ============================================================================
// IPC: Theme
// ============================================================================

ipcMain.handle('theme:get', () =>
  nativeTheme.shouldUseDarkColors ? 'dark' : 'light'
);

handleGuarded('theme:set', (_event, theme: 'light' | 'dark' | 'system') => {
  nativeTheme.themeSource = theme;
  store.set('theme', theme);
});

// Push theme changes to all renderer windows
nativeTheme.on('updated', () => {
  const isDark = nativeTheme.shouldUseDarkColors;
  BrowserWindow.getAllWindows().forEach((w) => {
    if (!w.isDestroyed()) w.webContents.send('theme:updated', isDark);
  });
});

// ============================================================================
// IPC: Dialogs
// ============================================================================

handleGuarded('dialog:show-save', async (_event, options: Electron.SaveDialogOptions) => {
  const win = mainWindow ?? BrowserWindow.getFocusedWindow();
  if (!win) return { canceled: true };
  return dialog.showSaveDialog(win, options ?? {});
});

handleGuarded('dialog:show-open', async (_event, options: Electron.OpenDialogOptions) => {
  const win = mainWindow ?? BrowserWindow.getFocusedWindow();
  if (!win) return { canceled: true, filePaths: [] };
  return dialog.showOpenDialog(win, options ?? {});
});

// ============================================================================
// IPC: Sidecar — gizzi-code AI runtime (port ${PORTS.GIZZI})
// The platform uses window.allternitSidecar to discover the backend URL.
// ============================================================================

ipcMain.handle('sidecar:get-status', async (): Promise<'running' | 'stopped' | 'error'> => {
  if (!gizziManager.isRunning()) return 'stopped';
  try {
    const res = await fetch(`${gizziManager.getUrl()}/v1/global/health`, {
      headers: gizziManager.getAuthHeader()
        ? { Authorization: gizziManager.getAuthHeader()! }
        : undefined,
      signal: AbortSignal.timeout(1000),
    });
    return res.ok || res.status === 401 || res.status === 404 ? 'running' : 'error';
  } catch {
    return 'error';
  }
});

ipcMain.handle('sidecar:get-api-url', () => (
  gizziManager.isRunning() ? 'allternit-gizzi://runtime' : undefined
));

handleGuarded('sidecar:start', async () => {
  try {
    const url = await startGizziRuntime();
    updateSidecarConfig(url);
    return true;
  } catch {
    return false;
  }
});

handleGuarded('sidecar:stop', () => { gizziManager.stop(); return true; });

handleGuarded('sidecar:restart', async () => {
  try {
    gizziManager.stop();
    const url = await startGizziRuntime();
    updateSidecarConfig(url);
    return true;
  } catch {
    return false;
  }
});

// ============================================================================
// IPC: Connection settings
// ============================================================================

ipcMain.handle('connection:test', async () => {
  const backend = store.get('backend');
  try {
    const res = await fetch(`${activeBackendUrl}/health`, {
      signal: AbortSignal.timeout(3000),
    });
    return { mode: backend.mode, url: activeBackendUrl, status: res.ok ? 'connected' : 'error' };
  } catch {
    return { mode: backend.mode, url: activeBackendUrl, status: 'disconnected' };
  }
});

ipcMain.handle('connection:get-backend', () => {
  const backend = store.get('backend');
  return { mode: backend.mode, url: activeBackendUrl };
});

handleGuarded('connection:set-backend', async (_event, config: { mode: 'bundled' | 'remote' | 'development'; remoteUrl?: string }) => {
  const nextBackend = { ...store.get('backend'), ...config };
  store.set('backend', nextBackend);
  void authManager.updateBackendProfile({
    mode: nextBackend.mode,
    remoteUrl: nextBackend.remoteUrl,
  });

  // Resolve the new active backend URL so subsequent connection tests and
  // SDK consumers point at the right place.
  if (nextBackend.mode === 'remote' && nextBackend.remoteUrl) {
    activeBackendUrl = nextBackend.remoteUrl;
  } else if (nextBackend.mode === 'development') {
    activeBackendUrl = URLS.DEV_UI;
  } else {
    activeBackendUrl = URLS.API;
  }

  mainWindow?.webContents.send('connection:state', { mode: nextBackend.mode, url: activeBackendUrl });
});

// ============================================================================
// IPC: VM Setup (onboarding) — Lima-based
// Lima manages the Ubuntu VM lifecycle via `limactl`. No custom images needed.
// Install: brew install lima
// ============================================================================

ipcMain.handle('vm-setup:check-connectivity', async () => {
  const [internet, github, services] = await Promise.allSettled([
    fetch('https://1.1.1.1', { signal: AbortSignal.timeout(3000) }),
    fetch('https://github.com', { signal: AbortSignal.timeout(3000) }),
    fetch('https://allternit.com', { signal: AbortSignal.timeout(3000) }),
  ]);
  return {
    internet: internet.status === 'fulfilled',
    github: github.status === 'fulfilled',
    allternitServices: services.status === 'fulfilled',
  };
});

// "check-images-exist" now means "is Lima installed and the VM provisioned?"
ipcMain.handle('vm-setup:check-images-exist', async () => {
  const limaReady = await isLimaInstalled();
  log.info(`[VM Setup] Lima installed: ${limaReady}`);
  return limaReady;
});

// "download-images" now means "install Lima via brew if not present"
handleGuarded('vm-setup:download-images', async (event) => {
  const alreadyInstalled = await isLimaInstalled();
  if (alreadyInstalled) {
    event.sender.send('vm-setup:download-progress', {
      stage: 'complete',
      fileName: 'lima',
      bytesDownloaded: 1,
      totalBytes: 1,
      speed: 0,
      eta: 0,
    });
    return true;
  }

  log.info('[VM Setup] Installing Lima...');
  await installLima();
  event.sender.send('vm-setup:download-progress', {
    stage: 'complete',
    fileName: 'lima',
    bytesDownloaded: 1,
    totalBytes: 1,
    speed: 0,
    eta: 0,
  });
  return true;
});

handleGuarded('vm-setup:initialize-vm', async (event) => {
  const sendProgress = (stage: string, message: string, progress: number) => {
    event.sender.send('vm-setup:init-progress', { stage, message, progress });
  };

  try {
    sendProgress('verifying', 'Checking Lima installation...', 10);
    const limaReady = await isLimaInstalled();
    if (!limaReady) {
      throw new Error('Lima is not installed. Run vm-setup:download-images first.');
    }

    await startVM(sendProgress);
    log.info('[VM Setup] Lima VM started successfully');
    return true;
  } catch (error) {
    log.error('[VM Setup] Failed to start Lima VM:', error);
    throw new Error(`Failed to start VM: ${(error as Error).message}`);
  }
});

ipcMain.handle('vm-setup:get-vm-status', async (): Promise<'running' | 'stopped' | 'error'> => {
  const status = await getVMStatus();
  if (status === 'running') return 'running';
  if (status === 'error') return 'error';
  return 'stopped';
});

// ============================================================================
// IPC: Extension Bridge
// Lets the renderer subscribe to Chrome extension messages and send responses.
// ============================================================================

ipcMain.handle('extension:get-status', () => ({
  connected: getConnectedExtensionSockets().length > 0,
}));

ipcMain.handle('extension:send', (_event, message: unknown) => {
  return sendToExtension(message);
});

// ============================================================================
// IPC: Tunnel (Cloudflare Web Access)
// ============================================================================

handleGuarded('tunnel:enable', async () => {
  try {
    const url = await tunnelManager.enableWebAccess();
    return { success: true, url };
  } catch (error) {
    log.error('[Tunnel] Failed to enable web access:', error);
    return { success: false, error: (error as Error).message };
  }
});

// Start tunnel only — no browser redirect. Used by the in-app onboarding wizard
// so it can register the backend directly without opening a system browser tab.
handleGuarded('tunnel:start', async () => {
  try {
    const url = await tunnelManager.start();
    const token = tunnelManager.getToken();
    return { success: true, url, token };
  } catch (error) {
    log.error('[Tunnel] Failed to start:', error);
    return { success: false, error: (error as Error).message };
  }
});

handleGuarded('tunnel:disable', () => {
  tunnelManager.stop();
  return { success: true };
});

ipcMain.handle('tunnel:get-url', () => tunnelManager.getUrl());

ipcMain.handle('tunnel:get-status', () => tunnelManager.getState());

// ============================================================================
// IPC: Onboarding
// ============================================================================

ipcMain.handle('app:is-first-launch', () => !store.get('onboardingComplete'));

ipcMain.handle('app:complete-onboarding', () => {
  store.set('onboardingComplete', true);
  return true;
});

// ============================================================================
// IPC: Permission Guide — macOS permission detection + native overlay
// ============================================================================

ipcMain.handle('permission-guide:check', async () => checkPermissions());

handleGuarded('permission-guide:present', async (_event, panel: 'accessibility' | 'screen-recording') =>
  presentGuide(panel)
);

ipcMain.handle('permission-guide:dismiss', () => dismissGuide());

ipcMain.handle('permission-guide:get-status', () => getGuideStatus());

ipcMain.handle('permission-guide:request-check', async () => {
  invalidatePermissionCache();
  const status = await checkPermissions();
  store.set('permissions.lastStatus', { ...status, checkedAt: new Date().toISOString() });
  mainWindow?.webContents.send('permission-guide:status', status);
  return status;
});

ipcMain.handle('permission-guide:ready-for-check', async () => {
  // Called by the renderer's onboarding wizard when it reaches the permissions step.
  // This allows the platform UI to control exact timing instead of relying on a fixed delay.
  log.info('[Main] Renderer signaled ready for permission check');
  invalidatePermissionCache();
  const status = await checkPermissions();
  store.set('permissions.lastStatus', { ...status, checkedAt: new Date().toISOString() });
  mainWindow?.webContents.send('permission-guide:status', status);
  // If this is the first launch and onboarding hasn't started yet, kick it off now.
  if (permissionOnboardingResolver) {
    const resolver = permissionOnboardingResolver;
    permissionOnboardingResolver = null;
    resolver();
  }
  return status;
});

// ============================================================================
// IPC: Feature Flags
// ============================================================================

ipcMain.handle('featureFlags:get', (_event, key?: string) =>
  key ? featureFlagManager.get(key) : featureFlagManager.getAll()
);

handleGuarded('featureFlags:set', (_event, key: string, value: unknown) => {
  featureFlagManager.set(key, value as import('./feature-flags.js').FlagValue);
  return true;
});

// ============================================================================
// IPC: Persisted State
// ============================================================================

ipcMain.handle('state:get', (_event, key: string) => persistedState.get(key as never));

handleGuarded('state:set', (_event, key: string, value: unknown) => {
  persistedState.set(key as never, value as never);
  // Push to all renderer windows
  BrowserWindow.getAllWindows().forEach(w => {
    if (!w.isDestroyed()) w.webContents.send('state:changed', { key, value });
  });
  return true;
});

handleGuarded('state:patch', (_event, key: string, partial: unknown) => {
  persistedState.patch(key as never, partial as never);
  return true;
});

// ============================================================================
// IPC: Find in Page
// ============================================================================

ipcMain.handle('window:find-in-page', (_event, text: string | undefined, options?: Electron.FindInPageOptions) => {
  if (!mainWindow) return;
  if (!text && !options?.findNext) return;
  mainWindow.webContents.findInPage(text ?? '', options);
});

ipcMain.handle('window:find-stop', (_event, keepSelection?: boolean) => {
  mainWindow?.webContents.stopFindInPage(keepSelection ? 'keepSelection' : 'clearSelection');
});

// Forward find results to renderer
app.whenReady().then(() => {
  // Attach find-in-page result forwarding whenever a new window is created
  app.on('browser-window-created', (_e, win) => {
    win.webContents.on('found-in-page', (_event, result) => {
      win.webContents.send('window:find-result', result);
    });
  });
});

// ============================================================================
// IPC: Locale / i18n
// ============================================================================

ipcMain.handle('locale:get', () => {
  const stored = persistedState.get('prefs').locale;
  return stored ?? app.getLocale();
});

handleGuarded('locale:set', (_event, locale: string) => {
  persistedState.patch('prefs', { locale });
  BrowserWindow.getAllWindows().forEach(w => {
    if (!w.isDestroyed()) w.webContents.send('locale:changed', locale);
  });
  return true;
});

// ============================================================================
// IPC: Menu Bar Mode + Startup on Login
// ============================================================================

ipcMain.handle('menuBar:getMode', () => persistedState.get('prefs').menuBarMode);

handleGuarded('menuBar:setMode', (_event, enabled: boolean) => {
  persistedState.patch('prefs', { menuBarMode: enabled });
  if (isMac) {
    if (enabled) {
      app.dock?.hide();
    } else {
      app.dock?.show();
    }
  }
  mainWindow?.webContents.send('menuBar:modeChanged', enabled);
  return true;
});

ipcMain.handle('startup:getOnLogin', () => persistedState.get('prefs').startupOnLogin);

handleGuarded('startup:setOnLogin', (_event, enabled: boolean) => {
  persistedState.patch('prefs', { startupOnLogin: enabled });
  if (isMac) app.setLoginItemSettings({ openAtLogin: enabled });
  return true;
});

// ============================================================================
// IPC: Context Menu (native right-click for webview)
// ============================================================================

app.whenReady().then(() => {
  app.on('browser-window-created', (_e, win) => {
    win.webContents.on('context-menu', (_event, params) => {
      const menuItems: Electron.MenuItemConstructorOptions[] = [];

      if (params.selectionText) {
        menuItems.push(
          { label: 'Copy', role: 'copy' },
          { type: 'separator' },
        );
      }
      if (params.isEditable) {
        menuItems.push(
          { label: 'Cut', role: 'cut' },
          { label: 'Copy', role: 'copy' },
          { label: 'Paste', role: 'paste' },
          { type: 'separator' },
          { label: 'Select All', role: 'selectAll' },
        );
      }
      if (!params.selectionText && !params.isEditable) {
        menuItems.push(
          { label: 'Back', enabled: win.webContents.canGoBack(), click: () => win.webContents.goBack() },
          { label: 'Forward', enabled: win.webContents.canGoForward(), click: () => win.webContents.goForward() },
          { label: 'Reload', click: () => win.webContents.reload() },
        );
      }
      if (isDev) {
        menuItems.push(
          { type: 'separator' },
          { label: 'Inspect Element', click: () => win.webContents.inspectElement(params.x, params.y) },
        );
      }

      if (menuItems.length > 0) {
        Menu.buildFromTemplate(menuItems).popup({ window: win });
      }
    });
  });
});

// ============================================================================
// IPC: MCP Host
// ============================================================================

ipcMain.handle('mcp:list-servers', () => mcpHostManager.listServers());

ipcMain.handle('mcp:list-tools', (_event, serverId?: string) => mcpHostManager.listTools(serverId));

handleGuarded('mcp:call-tool', async (_event, serverId: string, toolName: string, args: unknown) => {
  try {
    const result = await mcpHostManager.callTool(serverId, toolName, args);
    return { success: true, result };
  } catch (err) {
    return { success: false, error: (err as Error).message };
  }
});

handleGuarded('mcp:add-server', async (_event, id: string, config: unknown) => {
  try {
    await mcpHostManager.addServer(id, config as import('./mcp-host-manager.js').McpServerConfig);
    return { success: true };
  } catch (err) {
    return { success: false, error: (err as Error).message };
  }
});

handleGuarded('mcp:remove-server', async (_event, id: string) => {
  try {
    await mcpHostManager.removeServer(id);
    return { success: true };
  } catch (err) {
    return { success: false, error: (err as Error).message };
  }
});

// ============================================================================
// IPC: Worker Bus
// ============================================================================

ipcMain.handle('worker:send', async (_event, workerName: string, message: unknown) => {
  try {
    const result = await workerBus.send(workerName, message as import('./workers/worker-bus.js').WorkerMessage);
    return { success: true, result };
  } catch (err) {
    return { success: false, error: (err as Error).message };
  }
});

ipcMain.handle('worker:list', () =>
  ['sqlite', 'search', 'shell-path'].filter(name => workerBus.isAlive(name))
);

// ============================================================================
// IPC: HyperFrames — render HTML artifacts as MP4/MOV/WebM video
// Requires: npx hyperframes (install via `npx skills add heygen-com/hyperframes`)
// ============================================================================

ipcMain.handle('hyperframes:check', async () => {
  return new Promise<{ available: boolean; version?: string }>((resolve) => {
    execFile('npx', ['hyperframes', '--version'], { timeout: 5000 }, (err, stdout) => {
      if (err) resolve({ available: false });
      else resolve({ available: true, version: stdout.trim() });
    });
  });
});

handleGuarded('hyperframes:render', async (event, html: string, options: {
  format?: 'mp4' | 'mov' | 'webm';
  fps?: number;
  width?: number;
  height?: number;
}) => {
  const { format = 'mp4', fps = 30, width = 1920, height = 1080 } = options;
  const tmpDir = os.tmpdir();
  const stamp = Date.now();
  const srcFile = join(tmpDir, `allternit-hf-${stamp}.html`);
  const outFile = join(tmpDir, `allternit-hf-${stamp}.${format}`);

  try {
    fs.writeFileSync(srcFile, html, 'utf-8');

    const renderResult = await new Promise<{ success: boolean; error?: string }>((resolve) => {
      const proc = execFile(
        'npx',
        ['hyperframes', 'render', srcFile,
          '--output', outFile,
          '--fps', String(fps),
          '--width', String(width),
          '--height', String(height),
        ],
        { timeout: 180_000 },
        (err) => {
          if (err) resolve({ success: false, error: err.message });
          else resolve({ success: true });
        }
      );
      proc.stdout?.on('data', (d: Buffer) => event.sender.send('hyperframes:progress', d.toString().trim()));
      proc.stderr?.on('data', (d: Buffer) => event.sender.send('hyperframes:progress', d.toString().trim()));
    });

    try { fs.unlinkSync(srcFile); } catch { /* ignore */ }

    if (!renderResult.success) return { success: false, error: renderResult.error };
    if (!fs.existsSync(outFile)) return { success: false, error: 'Render completed but output file not found' };

    // Native save dialog — let user choose where to save
    const win = BrowserWindow.getFocusedWindow();
    const saveResult = await dialog.showSaveDialog(win ?? BrowserWindow.getAllWindows()[0], {
      title: 'Save Video',
      defaultPath: `allternit-video-${stamp}.${format}`,
      filters: [
        { name: 'Video', extensions: [format] },
        { name: 'All Files', extensions: ['*'] },
      ],
    });

    if (saveResult.canceled || !saveResult.filePath) {
      try { fs.unlinkSync(outFile); } catch { /* ignore */ }
      return { success: false, error: 'Cancelled' };
    }

    fs.copyFileSync(outFile, saveResult.filePath);
    try { fs.unlinkSync(outFile); } catch { /* ignore */ }

    return { success: true, savedPath: saveResult.filePath };

  } catch (err) {
    try { fs.unlinkSync(srcFile); } catch { /* ignore */ }
    try { fs.unlinkSync(outFile); } catch { /* ignore */ }
    return { success: false, error: (err as Error).message };
  }
});

// ─── Mini-apps: install / start / stop / status ───────────────────────────────

handleGuarded('miniApps:install', async (event, id: string) => {
  return installMiniApp(id, (progress) => {
    event.sender.send('miniApps:install-progress', progress);
  });
});

handleGuarded('miniApps:start', async (event, id: string) => {
  return startMiniApp(id, (progress) => {
    event.sender.send('miniApps:install-progress', progress);
  });
});

handleGuarded('miniApps:stop', (_event, id: string) => {
  stopMiniApp(id);
  return { success: true };
});

ipcMain.handle('miniApps:getStatus', (_event, id: string) => getMiniAppStatus(id));
ipcMain.handle('miniApps:launchDesktop', (_event, id: string) => launchMiniAppDesktop(id));
ipcMain.handle('miniApps:getApproval', (_event, id: string, registration) => getMiniAppApproval(id, registration));
handleGuarded('miniApps:reviewAndApprove', (_event, registration) => reviewAndApproveMiniApp(registration));
handleGuarded('miniApps:revokeApproval', (_event, id: string) => {
  revokeMiniAppApproval(id);
  return { success: true };
});
handleGuarded('miniApps:setSecret', (_event, id: string, name: string, value: string) => setMiniAppSecret(id, name, value));
ipcMain.handle('miniApps:listSecrets', (_event, id: string) => listMiniAppSecrets(id));
// ACI export bridge: write a gateway-generated provider_routing section into
// the Hermes mini-app's ~/.hermes/config.yaml (backs up the previous file).
handleGuarded('hermesRouting:export', (_event, routingYaml: string) => exportHermesRouting(routingYaml));
handleGuarded('miniApps:deleteSecret', (_event, id: string, name: string) => deleteMiniAppSecret(id, name));
handleGuarded('miniApps:removeRuntime', (_event, id: string) => removeMiniAppRuntime(id));
handleGuarded('miniApps:rollbackRuntime', (_event, id: string) => rollbackMiniAppRuntime(id));

// Versioned marketplace releases (atomic install / rollback / uninstall)
handleGuarded('miniApps:installRelease', async (event, options: { registryUrl: string; id: string; version?: string }) => {
  return installReleaseFromRegistry(options, (progress) => {
    event.sender.send('miniApps:install-progress', progress);
  });
});
handleGuarded('miniApps:rollbackRelease', (_event, id: string, registryUrl?: string) => rollbackReleaseInstall(id, registryUrl));
handleGuarded('miniApps:removeRelease', (_event, id: string, registryUrl?: string) => {
  stopMiniApp(id);
  return removeReleaseInstall(id, registryUrl);
});
ipcMain.handle('miniApps:listReleaseInstalls', () => listReleaseInstalls());
ipcMain.handle('miniApps:getReleaseInstall', (_event, id: string) => getReleaseInstallState(id));

// ─── Sign in with ChatGPT (feature.siwc, default OFF; Desktop main only) ────
// OpenAI's documented ChatGPT-plan flow. Credentials stay in the secure store;
// the renderer only ever sees status. gizzi-code gets access tokens one
// request at a time from a loopback broker (see siwc-broker.ts).

let siwcManager: SiwcManager | null = null;
let siwcBroker: SiwcBroker | null = null;
function siwc(): SiwcManager {
  if (siwcManager) return siwcManager;
  const manager = createSiwcManager({
    flagEnabled: () => featureFlagManager.get<boolean>(SIWC_FLAG) === true,
    readSecret,
    writeSecret,
    deleteSecret,
    openExternal: (url) => {
      openExternalAllowlisted(url);
    },
    logger: (message) => log.info(message),
  });
  manager.onStatusChange((status) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed()) window.webContents.send('siwc:status-changed', status);
    }
  });
  featureFlagManager.onChange((key) => {
    if (key !== SIWC_FLAG) return;
    if (featureFlagManager.get<boolean>(SIWC_FLAG) !== true) manager.cancelSignIn();
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed()) window.webContents.send('siwc:status-changed', manager.status());
    }
  });
  siwcManager = manager;
  return manager;
}

/** Broker env for gizzi-code. The broker answers only while the flag is on. */
async function siwcLaunchEnvironment(): Promise<Record<string, string>> {
  try {
    if (!siwcBroker) siwcBroker = await startSiwcBroker(siwc());
    return siwcBroker.env();
  } catch (error) {
    log.warn('[SIWC] Token broker failed to start:', error);
    return {};
  }
}

ipcMain.handle('siwc:status', () => siwc().status());
handleGuarded('siwc:signIn', async (_event, opts?: { enablePlanUsage?: boolean }) => {
  try {
    return { ok: true as const, status: await siwc().signIn({ enablePlanUsage: opts?.enablePlanUsage === true }) };
  } catch (error) {
    return { ok: false as const, error: error instanceof Error ? error.message : String(error), status: siwc().status() };
  }
});
handleGuarded('siwc:cancel', () => {
  siwc().cancelSignIn();
  return siwc().status();
});
handleGuarded('siwc:signOut', () => siwc().signOut());

// ─── OAuth broker (main-process token vault; tokens never cross IPC) ────────

let miniAppOAuthBroker: MiniAppOAuthBroker | null = null;
function oauthBroker(): MiniAppOAuthBroker {
  if (miniAppOAuthBroker) return miniAppOAuthBroker;
  const broker = createMiniAppOAuthBroker({
    encrypt: (value) => {
      if (!safeStorage.isEncryptionAvailable()) throw new Error('Operating-system encryption is unavailable');
      return safeStorage.encryptString(value).toString('base64');
    },
    decrypt: (value) => safeStorage.decryptString(Buffer.from(value, 'base64')),
    openExternal: (url) => {
      openExternalAllowlisted(url);
    },
    storagePath: () => join(app.getPath('userData'), 'mini-app-oauth-tokens.json'),
    logger: (message) => log.info(message),
  });
  broker.onFlowComplete((result) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed()) window.webContents.send('miniApps:oauth-complete', result);
    }
  });
  miniAppOAuthBroker = broker;
  return broker;
}

// Sandboxed runtimes receive fresh OAuth tokens as environment variables at
// start time. Resolution happens entirely in the main process: the broker
// refreshes when needed, and a missing/unauthorized account simply means the
// variable is absent — tokens never cross IPC.
setMiniAppOAuthTokenResolver(async (appId, providerId) => {
  const result = await oauthBroker().getValidAccessToken(appId, providerId, 'default');
  return result.token ?? null;
});

handleGuarded('miniApps:oauthStart', (_event, appId: string, providerId: string, provider: MiniAppOAuthProvider, accountId: string) =>
  oauthBroker().startFlow(appId, providerId, provider, accountId));
ipcMain.handle('miniApps:oauthCancel', (_event, flowId: string) => oauthBroker().cancelFlow(flowId));
ipcMain.handle('miniApps:oauthAccounts', (_event, appId: string) => oauthBroker().listAccounts(appId));
handleGuarded('miniApps:oauthDisconnect', (_event, appId: string, providerId: string, accountId: string) =>
  oauthBroker().disconnect(appId, providerId, accountId));

// ─── Artifact preview capture ────────────────────────────────────────────────
// A session agent looks at the site it built (pane_artifact → Build output):
// rendered for real in a throwaway offscreen window, in its own in-memory
// session so it shares no cookies or storage with the app.
handleGuarded('artifact-preview:host', (_event, html: string) => hostPreview(html));
handleGuarded('artifact-preview:capture', (_event, req: PreviewCaptureRequest) =>
  captureArtifactPreview(req, ({ width, height }) =>
    new BrowserWindow({
      show: false,
      width,
      height,
      useContentSize: true,
      webPreferences: {
        offscreen: true,
        sandbox: true,
        contextIsolation: true,
        nodeIntegration: false,
        partition: `artifact-preview-${Date.now()}`,
        backgroundThrottling: false,
      },
    }) as unknown as CaptureWindow,
  ));

// ─── Browser API Capture (HAR-derived API client) ───────────────────────────
// Records network traffic from the default Electron session and returns a HAR
// archive that the platform ingests to derive reusable API contracts.

ipcMain.handle('browser-capture:is-available', () => isCaptureAvailable());
handleGuarded('browser-capture:start', (_event, options?: { filterUrls?: string[] }) => {
  try {
    const { sessionId } = createCaptureSession(options);
    return { success: true as const, sessionId };
  } catch (error) {
    log.error('[BrowserCapture] Failed to start session:', error);
    return { success: false as const, error: error instanceof Error ? error.message : 'Unknown error' };
  }
});
ipcMain.handle('browser-capture:stop', (_event, sessionId: string) => {
  const result = stopCaptureSession(sessionId);
  if (!result) {
    return { success: false as const, error: 'Session not found' };
  }
  return { success: true as const, har: result.har };
});
