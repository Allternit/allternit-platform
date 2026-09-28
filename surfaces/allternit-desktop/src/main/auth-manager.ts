import { app, BrowserWindow, ipcMain, net, safeStorage, session, shell } from 'electron';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { openClerkOAuthPopup, setCookieOnSession } from './clerk-oauth-popup.js';
import {
  buildAuthForwardInit,
  cookieFromSetCookieHeader,
  setCookiesFromResponse,
} from './auth-https-forward.js';
import { PORTS } from './config.js';
import { connectorSidecarManager } from './connector-sidecar-manager.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
import {
  createCipheriv,
  createDecipheriv,
  createHash,
  generateKeyPairSync,
  randomBytes,
  sign,
} from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';
import log from 'electron-log';
import WebSocket from 'ws';
import { URLS } from './config.js';
import { isDesktopAuthNavigation } from './desktop-auth-url.js';
import { ClerkTokenBroker } from './clerk-token-broker.js';

const RUNTIME_CLIENT_ID = 'allternit-desktop-runtime';
const PAIRING_TIMEOUT_MS = 10 * 60 * 1000;
const ROTATION_SKEW_MS = 7 * 24 * 60 * 60 * 1000;
const HEARTBEAT_INTERVAL_MS = 45 * 1000;
// Longest a request may wait for a Clerk token when none is cached. The
// renderer's value is decorative on Desktop (main injects credentials).
const REQUEST_CLERK_WAIT_MS = 4_000;
const RENDERER_CLERK_WAIT_MS = 1_500;
// One-shot flows that need a real token wait out a full hidden refresh.
export const FULL_CLERK_REFRESH_WAIT_MS = 25_000;
const SAFE_STORAGE_HEADER = 'allternit-safe-storage-v1\n';
const LOCAL_STORAGE_HEADER = 'allternit-local-aes-gcm-v1\n';

/**
 * The auth window loads from https://accounts.<clerk-instance-domain>/__desktop_auth__/
 * (served locally via protocol interception) so Clerk's production-origin
 * checks and the Turnstile CAPTCHA hostname lock pass legitimately.
 */
const AUTH_WINDOW_PATH_PREFIX = '/__desktop_auth__';
const AUTH_SESSION_PARTITION = 'persist:allternit-auth';

const AUTH_FILE_MIME_TYPES: Record<string, string> = {
  '.html': 'text/html',
  '.js': 'application/javascript',
  '.mjs': 'application/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
};

export interface DesktopAuthSession {
  accessToken: string;
  refreshToken: string;
  tokenType: string;
  scope: string;
  expiresAt: number;
  userId: string;
  userEmail: string;
  clientId: string;
  runtimeId: string;
  organizationId?: string;
  capabilities: string[];
}

interface PersistedRuntimeIdentity extends DesktopAuthSession {
  privateKeyPem: string;
  publicKey: string;
  savedAt: string;
}

interface PairingStartResponse {
  pairingId: string;
  deviceCode: string;
  userCode: string;
  challenge: string;
  verificationUrl: string;
  expiresAt: string;
  pollIntervalSeconds: number;
}

interface PairingExchangeResponse {
  runtimeId: string;
  userId: string;
  userEmail: string;
  organizationId?: string;
  deviceToken: string;
  tokenType: string;
  expiresAt: string;
  capabilities: string[];
}

interface PendingPairing {
  pairing: PairingStartResponse;
  privateKeyPem: string;
  publicKey: string;
  resolve: (session: DesktopAuthSession) => void;
  reject: (error: Error) => void;
  promise: Promise<DesktopAuthSession>;
  timeout: NodeJS.Timeout;
  exchangeInFlight: boolean;
}

export interface DesktopBackendProfile {
  mode: 'bundled' | 'remote' | 'development';
  remoteUrl?: string;
}

export interface DesktopAccountRecord {
  userId: string;
  userEmail: string;
  clientId: string;
  lastSignedInAt: string;
  lastSeenAt: string;
  current: boolean;
  backend?: DesktopBackendProfile;
}

function cloudApiBaseUrl(): string {
  return (process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API).replace(/\/$/, '');
}

function pairingSignatureMessage(pairingId: string, challenge: string): string {
  return `allternit-runtime-pairing:${pairingId}:${challenge}`;
}

const SYNTHETIC_EMAIL_SUFFIX = '@users.allternit.local';

interface ClerkSessionPayload {
  token: string;
  userId: string;
  email: string;
}

function isSyntheticEmail(email: string | null | undefined): boolean {
  if (!email || !email.includes('@')) return true;
  return email.endsWith(SYNTHETIC_EMAIL_SUFFIX);
}

function preferredEmail(...candidates: Array<string | null | undefined>): string | null {
  for (const value of candidates) {
    const email = typeof value === 'string' ? value.trim() : '';
    if (email && !isSyntheticEmail(email)) return email;
  }
  return null;
}

function decodeJwtPayload(token: string): Record<string, unknown> | null {
  try {
    const part = token.split('.')[1];
    if (!part) return null;
    return JSON.parse(Buffer.from(part, 'base64url').toString('utf8')) as Record<string, unknown>;
  } catch {
    return null;
  }
}

function emailFromJwt(token: string | null | undefined): string | null {
  if (!token) return null;
  const payload = decodeJwtPayload(token);
  if (!payload) return null;
  const direct = [payload.email, payload.email_address, payload.primary_email];
  const nested = Array.isArray(payload.email_addresses)
    ? payload.email_addresses.flatMap((entry) => {
        if (typeof entry === 'string') return [entry];
        if (entry && typeof entry === 'object' && 'email_address' in entry) {
          return [String((entry as { email_address?: string }).email_address ?? '')];
        }
        return [];
      })
    : [];
  return preferredEmail(...direct.map((value) => typeof value === 'string' ? value : null), ...nested);
}

function jwtExpiresAt(token: string): number | null {
  const payload = decodeJwtPayload(token);
  return typeof payload?.exp === 'number' ? payload.exp * 1000 : null;
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export class DesktopAuthManager {
  private session: DesktopAuthSession | null = null;
  private runtimeIdentity: PersistedRuntimeIdentity | null = null;
  private pendingPairing: PendingPairing | null = null;
  private heartbeatTimer: NodeJS.Timeout | null = null;
  private relaySocket: any = null;
  private relayReconnectTimer: NodeJS.Timeout | null = null;
  private relayReconnectDelayMs = 1_000;
  private relayWatchdogTimer: NodeJS.Timeout | null = null;
  private relayLastMessageAt = 0;
  private relayLocalSockets = new Map<string, WebSocket>();
  private refreshInFlight: Promise<void> | null = null;
  private readonly identityPath = path.join(app.getPath('userData'), 'auth', 'runtime-identity.json');
  private readonly platformKeyPath = path.join(app.getPath('userData'), 'auth', 'platform-encryption-key.bin');
  private readonly connectorSidecarTokensPath = path.join(app.getPath('userData'), 'auth', 'connector-sidecar-tokens.json');
  private readonly legacySessionPath = path.join(app.getPath('userData'), 'auth', 'desktop-session.json');
  private readonly accountsPath = path.join(app.getPath('userData'), 'auth', 'desktop-accounts.json');
  private signInResolver: ((session: DesktopAuthSession) => void) | null = null;
  private signInRejecter: ((error: Error) => void) | null = null;
  private startupGateResolver: ((session: DesktopAuthSession) => void) | null = null;
  private splashWindow: BrowserWindow | null = null;
  private clerkTokenResolver: ((payload: ClerkSessionPayload) => void) | null = null;
  private clerkTokenRejecter: ((error: Error) => void) | null = null;
  private authWindow: BrowserWindow | null = null;
  private authWindowBaseUrl: string | null = null;
  private authSessionProtocolRegistered = false;
  private oauthPopupInFlight = false;
  private pendingClerkEmail: string | null = null;
  // Clerk session JWT for cloud calls. Never makes a caller wait out the
  // hidden-window refresh (see clerk-token-broker.ts).
  private readonly clerkTokens = new ClerkTokenBroker({
    refresh: () => this.refreshClerkToken(),
    expiresAt: jwtExpiresAt,
    onFailure: (error, retryInMs) => {
      log.warn(`[Auth] Could not refresh Clerk session token (next try in ${Math.round(retryInMs / 1000)}s):`, error);
    },
  });

  constructor() {
    ipcMain.on('auth:start-login', () => {
      void this.handleLoginRequest();
    });
    // Kept for older startup-window bundles. Human login method selection now
    // belongs entirely to Clerk inside the desktop app.
    ipcMain.on('auth:start-google-login', () => {
      void this.handleLoginRequest();
    });

    ipcMain.handle('auth:clerk-token', (_event, payload: { token: string; userId: string; email: string }) => {
      const email = preferredEmail(payload?.email, emailFromJwt(payload?.token)) ?? payload?.email ?? '';
      const normalized: ClerkSessionPayload = {
        token: payload?.token ?? '',
        userId: payload?.userId ?? '',
        email,
      };
      log.info('[Auth] Clerk session token received from renderer; resolver present:', Boolean(this.clerkTokenResolver), 'email:', email || '(none)');
      if (normalized.token) this.cacheClerkToken(normalized.token);
      if (email) this.applyAccountEmail(email);
      this.clerkTokenResolver?.(normalized);
      this.clerkTokenResolver = null;
      this.clerkTokenRejecter = null;
    });

    // The renderer asks on every runtime API call; Electron main injects the
    // real credentials, so this must never hold a request up.
    ipcMain.handle('auth:get-clerk-token', () => this.getClerkToken(RENDERER_CLERK_WAIT_MS));

    ipcMain.handle('auth:clerk-error', (_event, message: string) => {
      log.warn('[Auth] Clerk renderer error:', message);
      this.clerkTokenRejecter?.(new Error(message));
      this.clerkTokenResolver = null;
      this.clerkTokenRejecter = null;
    });

    ipcMain.on('app:quit', () => app.quit());
  }

  async initialize(): Promise<void> {
    log.info('[Auth] Initializing paired runtime identity');
    this.quarantineLegacyOAuthSession();
    this.runtimeIdentity = this.readIdentityFromDisk();
    this.session = this.runtimeIdentity ? this.toSession(this.runtimeIdentity) : null;
    if (!this.session) {
      log.info('[Auth] This desktop has not been paired yet');
      return;
    }

    await this.upsertAccountRecord(this.session);
    if (isSyntheticEmail(this.session.userEmail)) {
      void this.recoverAccountEmail();
    }
    this.scheduleHeartbeat();
    this.connectRuntimeRelay();
    void this.refreshSessionIfNeeded()
      .then(() => this.sendHeartbeat())
      .catch((error) => {
        // Network loss must not destroy an otherwise valid local runtime
        // identity. A definitive 401/403 is handled by refresh/heartbeat.
        log.warn('[Auth] Cloud runtime validation deferred:', error);
      });
  }

  /**
   * Compatibility shim for older startup code. Pairing always uses the fixed
   * Allternit Cloud API and platform URL; it never follows the local/static UI.
   */
  setOAuthBaseUrl(_baseUrl: string): void {
    this.notifySplash('auth:ready', 'Allternit account pairing is ready');
  }

  isOAuthReady(): boolean {
    return true;
  }

  /**
   * Connector encryption is owned by Electron main, not the frequently rebuilt
   * Rust API. Packaged builds protect this key with the signed application's
   * `safeStorage`; development uses the existing authenticated local envelope
   * so rebuilds cannot trigger recurring Keychain ACL prompts.
   */
  getPlatformEncryptionEnvironment(): Record<string, string> {
    let key: string | null = null;
    if (fs.existsSync(this.platformKeyPath)) {
      try {
        key = this.decodeSecret(fs.readFileSync(this.platformKeyPath));
        if (!/^[a-f0-9]{64}$/i.test(key)) throw new Error('Connector key has an invalid format');
      } catch (error) {
        log.warn('[Auth] Connector encryption key is unreadable; replacing it:', error);
        this.quarantineCorruptFile(this.platformKeyPath);
        key = null;
      }
    }
    if (!key) {
      key = randomBytes(32).toString('hex');
      fs.mkdirSync(path.dirname(this.platformKeyPath), { recursive: true });
      fs.writeFileSync(this.platformKeyPath, this.encodeSecret(key), { mode: 0o600 });
    }
    return { ALLTERNIT_ENCRYPTION_KEY: key };
  }

  /**
   * Bearer tokens shared between allternit-api and the open-connector
   * connector sidecar (see cmd/allternit-api/src/open_connector_proxy.rs and
   * services/open-connector/PROVENANCE.md). Same persistence pattern as
   * getPlatformEncryptionEnvironment() above — generated once, protected by
   * the same authenticated local envelope / OS safeStorage in packaged
   * builds, reused across restarts so existing connector connections don't
   * silently invalidate every relaunch.
   */
  getConnectorSidecarEnvironment(): Record<string, string> {
    let tokens: { admin: string; runtime: string } | null = null;
    if (fs.existsSync(this.connectorSidecarTokensPath)) {
      try {
        const parsed = JSON.parse(this.decodeSecret(fs.readFileSync(this.connectorSidecarTokensPath)));
        if (
          typeof parsed?.admin === 'string' && /^[a-f0-9]{48}$/i.test(parsed.admin) &&
          typeof parsed?.runtime === 'string' && /^[a-f0-9]{48}$/i.test(parsed.runtime)
        ) {
          tokens = parsed;
        } else {
          throw new Error('Connector sidecar tokens have an invalid format');
        }
      } catch (error) {
        log.warn('[Auth] Connector sidecar tokens are unreadable; replacing them:', error);
        this.quarantineCorruptFile(this.connectorSidecarTokensPath);
        tokens = null;
      }
    }
    if (!tokens) {
      tokens = { admin: randomBytes(24).toString('hex'), runtime: randomBytes(24).toString('hex') };
      fs.mkdirSync(path.dirname(this.connectorSidecarTokensPath), { recursive: true });
      fs.writeFileSync(this.connectorSidecarTokensPath, this.encodeSecret(JSON.stringify(tokens)), { mode: 0o600 });
    }
    return {
      // The sidecar binds an ephemeral port and announces it at start; ask
      // the manager for the live URL (falls back to the default loopback
      // URL before the sidecar has started, which callers treat as
      // "connector unavailable" anyway).
      ALLTERNIT_CONNECTOR_SIDECAR_URL: connectorSidecarManager.getUrl(),
      ALLTERNIT_CONNECTOR_SIDECAR_ADMIN_TOKEN: tokens.admin,
      ALLTERNIT_CONNECTOR_SIDECAR_RUNTIME_TOKEN: tokens.runtime,
    };
  }

  /** Main-process-only snapshot for narrow request brokering. */
  getSessionSnapshot(): DesktopAuthSession | null {
    return this.session ? { ...this.session, capabilities: [...this.session.capabilities] } : null;
  }

  hasSession(): boolean {
    return this.session !== null;
  }

  async getSession(): Promise<DesktopAuthSession | null> {
    if (!this.session) return null;
    await this.refreshSessionIfNeeded();
    return this.session;
  }

  async waitForStartupSignIn(window?: BrowserWindow): Promise<DesktopAuthSession> {
    const existing = await this.getSession();
    if (existing) return existing;

    if (process.env.ALLTERNIT_SKIP_PAIRING === '1') {
      // Local-testing escape hatch: passes the startup gate and gives the
      // renderer's desktop auth broker an in-memory stub session. Nothing is
      // persisted, so the next launch without the env var returns to the
      // normal pairing flow. The far-future expiry keeps refreshSessionIfNeeded
      // from ever calling the cloud rotate endpoint for this fake identity.
      log.warn('[Auth] ALLTERNIT_SKIP_PAIRING=1 — skipping runtime pairing for local testing');
      this.session = {
        accessToken: '',
        refreshToken: '',
        tokenType: 'Bearer',
        scope: '',
        expiresAt: Date.now() + 365 * 24 * 60 * 60 * 1000,
        userId: 'local-test',
        userEmail: 'local-test@localhost',
        clientId: RUNTIME_CLIENT_ID,
        runtimeId: 'local-test',
        capabilities: [],
      };
      return this.session;
    }

    this.splashWindow = window || BrowserWindow.getAllWindows().find((candidate) => !candidate.isDestroyed()) || null;
    this.notifySplash('auth:ready', 'Allternit account pairing is ready');
    log.info('[Auth] Startup gate is waiting for runtime pairing');
    return new Promise((resolve) => {
      this.startupGateResolver = resolve;
    });
  }

  async ensureAuthenticated(window?: BrowserWindow): Promise<DesktopAuthSession> {
    const existing = await this.getSession();
    if (existing) return existing;
    this.splashWindow = window || BrowserWindow.getAllWindows().find((candidate) => !candidate.isDestroyed()) || null;
    this.notifySplash('auth:ready', 'Allternit account pairing is ready');
    this.notifySplash('auth-required', 'Pair this runtime with your Allternit account');
    return new Promise((resolve, reject) => {
      this.signInResolver = resolve;
      this.signInRejecter = reject;
    });
  }

  async signOut(): Promise<void> {
    const current = this.session;
    const token = current?.accessToken;
    const runtimeId = current?.runtimeId;
    this.clearPendingPairing(new Error('Pairing interrupted by sign-out'));

    if (token && runtimeId) {
      try {
        await fetch(`${cloudApiBaseUrl()}/api/v1/runtime-devices/${encodeURIComponent(runtimeId)}/revoke-self`, {
          method: 'POST',
          headers: { Authorization: `Bearer ${token}` },
        });
      } catch (error) {
        log.warn('[Auth] Could not notify cloud of runtime sign-out:', error);
      }
    }

    this.clearSession();
    await this.clearCurrentAccountFlag();
  }

  /** Handles the optional deep link used to return browser focus to desktop. */
  async handleCallbackUrl(callbackUrl: string): Promise<boolean> {
    const url = new URL(callbackUrl);
    if (url.protocol !== 'allternit:' || url.hostname !== 'pairing' || url.pathname !== '/complete') {
      return false;
    }
    const pairingId = url.searchParams.get('pairing_id');
    if (pairingId && this.pendingPairing?.pairing.pairingId === pairingId) {
      this.notifySplash('auth:login-started', 'Approved — securely pairing this runtime…');
      void this.exchangePendingPairing(this.pendingPairing);
    }
    return true;
  }

  async listAccounts(): Promise<DesktopAccountRecord[]> {
    const accounts = this.readAccountsFromDisk();
    return accounts.sort((left, right) => {
      if (left.current !== right.current) return left.current ? -1 : 1;
      return new Date(right.lastSeenAt).getTime() - new Date(left.lastSeenAt).getTime();
    });
  }

  async forgetAccount(userId: string): Promise<void> {
    if (this.session?.userId === userId) return;
    this.writeAccountsToDisk(this.readAccountsFromDisk().filter((account) => account.userId !== userId));
  }

  async updateBackendProfile(profile: DesktopBackendProfile): Promise<void> {
    if (!this.session) return;
    const accounts = this.readAccountsFromDisk();
    const next = accounts.map((account) => account.userId === this.session?.userId
      ? {
          ...account,
          backend: profile.mode === 'remote'
            ? { mode: profile.mode, remoteUrl: profile.remoteUrl }
            : { mode: profile.mode },
          lastSeenAt: new Date().toISOString(),
        }
      : account);
    this.writeAccountsToDisk(next);
  }

  private async handleLoginRequest(): Promise<void> {
    const visibleWindow = this.splashWindow
      || BrowserWindow.getAllWindows().find((candidate) => candidate.isVisible() && !candidate.isDestroyed());
    visibleWindow?.setAlwaysOnTop(false);

    try {
      const session = await this.startPairing();
      this.notifySplash('auth:login-success', 'Runtime paired — starting Allternit…');
      this.signInResolver?.(session);
      this.signInResolver = null;
      this.signInRejecter = null;
      this.startupGateResolver?.(session);
      this.startupGateResolver = null;
    } catch (error) {
      const failure = error instanceof Error ? error : new Error('Runtime pairing failed');
      this.notifySplash('auth:login-failed', failure.message);
      this.signInRejecter?.(failure);
      this.signInResolver = null;
      this.signInRejecter = null;
    }
  }

  private async startPairing(): Promise<DesktopAuthSession> {
    if (this.pendingPairing) return this.pendingPairing.promise;

    const { publicKey, privateKey } = generateKeyPairSync('ed25519');
    const publicKeyDer = publicKey.export({ format: 'der', type: 'spki' });
    const publicKeyRaw = publicKeyDer.subarray(publicKeyDer.length - 32).toString('base64url');
    const privateKeyPem = privateKey.export({ format: 'pem', type: 'pkcs8' }).toString();
    const response = await fetch(`${cloudApiBaseUrl()}/api/v1/runtime-pairings`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: `${os.hostname()} Desktop`,
        runtimeType: 'desktop',
        hostname: os.hostname(),
        platform: `${process.platform}-${process.arch}`,
        version: app.getVersion(),
        publicKey: publicKeyRaw,
      }),
    });
    if (!response.ok) {
      throw new Error(`Allternit pairing is unavailable (${response.status})`);
    }
    const pairing = await response.json() as PairingStartResponse;

    let resolvePairing!: (session: DesktopAuthSession) => void;
    let rejectPairing!: (error: Error) => void;
    const promise = new Promise<DesktopAuthSession>((resolve, reject) => {
      resolvePairing = resolve;
      rejectPairing = reject;
    });
    const timeout = setTimeout(() => {
      this.clearPendingPairing(new Error('Pairing expired. Choose Get started to try again.'));
    }, PAIRING_TIMEOUT_MS);
    const pending: PendingPairing = {
      pairing,
      privateKeyPem,
      publicKey: publicKeyRaw,
      resolve: resolvePairing,
      reject: rejectPairing,
      promise,
      timeout,
      exchangeInFlight: false,
    };
    this.pendingPairing = pending;

    // Native Clerk auth: open a dedicated auth window with context isolation,
    // load the React/Clerk renderer, and wait for the user to sign in.
    void (async () => {
      try {
        const clerk = await this.requestClerkSession({ hidden: false });
        this.closeAuthWindow();
        this.pendingClerkEmail = preferredEmail(clerk.email, emailFromJwt(clerk.token));
        this.cacheClerkToken(clerk.token);
        await this.completePairingWithClerkToken(clerk.token, pending);
      } catch (error) {
        log.warn('[Auth] Pairing with Clerk token failed:', error);
        this.closeAuthWindow();
        this.clearPendingPairing(error instanceof Error ? error : new Error('Clerk sign-in failed'));
      }
    })();

    return promise;
  }

  private async openAuthWindow(options: { hidden?: boolean; preserveSession?: boolean } = {}): Promise<BrowserWindow> {
    const hidden = options.hidden === true;
    const preserveSession = options.preserveSession === true || hidden;
    if (this.authWindow && !this.authWindow.isDestroyed()) {
      if (!hidden) this.authWindow.focus();
      return this.authWindow;
    }

    const authDir = join(__dirname, '../renderer/auth');
    const servingDomain = this.clerkAuthServingDomain();
    if (!servingDomain) {
      throw new Error('Clerk auth serving domain is not configured. Rebuild with NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY set.');
    }
    const authSession = this.prepareAuthSession(authDir, servingDomain);
    this.authWindowBaseUrl = `https://${servingDomain}${AUTH_WINDOW_PATH_PREFIX}/`;

    const window = new BrowserWindow({
      width: 560,
      height: 680,
      show: !hidden,
      skipTaskbar: hidden,
      resizable: false,
      alwaysOnTop: !hidden,
      titleBarStyle: 'hiddenInset',
      title: 'Allternit — Sign in',
      parent: hidden ? undefined : this.splashWindow ?? undefined,
      modal: hidden ? false : Boolean(this.splashWindow),
      webPreferences: {
        preload: join(__dirname, '../preload/auth.js'),
        nodeIntegration: false,
        contextIsolation: true,
        session: authSession,
      },
    });

    window.webContents.on('console-message', (event) => {
      log.info(`[AuthRenderer] ${event.message} (${event.sourceId}:${event.lineNumber})`);
    });

    // Fresh pairing starts from a clean Clerk client. Email/token refresh
    // must keep the signed-in cookies in this partition.
    if (!preserveSession) {
      await authSession.clearStorageData();
    }

    // Intercept Clerk OAuth navigation so Google/GitHub sign-in opens in a modal
    // popup instead of navigating away from the isolated auth renderer. Also
    // block any other navigation away from the auth page (e.g. Clerk redirecting
    // to the instance home_url after sign-in). Stay on this document so
    // TokenBridge can finish; reloading remounts SignIn as signed-out.
    window.webContents.on('will-navigate', (event, url) => {
      if (this.isOAuthProviderUrl(url)) {
        event.preventDefault();

        // clerk-js retries the external verification navigation whenever the
        // attempt stays pending — without this guard every retry opens another
        // popup on top of the previous one.
        if (this.oauthPopupInFlight) {
          log.info('[Auth] OAuth popup already open; ignoring duplicate navigation to:', url);
          return;
        }
        this.oauthPopupInFlight = true;
        log.info('[Auth] Opening OAuth popup for:', url);
        void openClerkOAuthPopup(url, {
          clerkSession: authSession,
          redirectOrigin: `https://${servingDomain}`,
          redirectPathPrefix: AUTH_WINDOW_PATH_PREFIX,
        })
          .then(async (result) => {
            // The exchange completed in the popup's real cookie jar; move the
            // signed-in Clerk client into the auth partition and reload the
            // renderer so clerk-js (and TokenBridge) see the session.
            for (const cookie of result.cookies) {
              await setCookieOnSession(authSession, cookie);
            }
            if (!window.isDestroyed() && this.authWindowBaseUrl) {
              window.loadURL(this.authWindowBaseUrl).catch((err) => {
                log.error('[Auth] Failed to reload auth renderer after OAuth:', err);
              });
            }
          })
          .catch((err) => {
            log.warn('[Auth] OAuth popup failed:', err);
            // Don't fail the whole pairing when the user closes the popup.
            // Reset the auth renderer to a clean client instead, so clerk-js
            // abandons the pending external verification and stops
            // re-navigating to the provider (which would reopen the popup).
            if (!window.isDestroyed() && this.authWindowBaseUrl) {
              const baseUrl = this.authWindowBaseUrl;
              void authSession.clearStorageData().then(() => {
                if (!window.isDestroyed()) {
                  window.loadURL(baseUrl).catch((loadErr) => {
                    log.error('[Auth] Failed to reset auth renderer:', loadErr);
                  });
                }
              });
            }
          })
          .finally(() => {
            this.oauthPopupInFlight = false;
          });
        return;
      }

      const authBaseUrl = this.authWindowBaseUrl;
      if (authBaseUrl && !isDesktopAuthNavigation(url, authBaseUrl)) {
        event.preventDefault();
        log.warn('[Auth] Blocking navigation away from auth renderer to:', url);
        // Bounce back to the auth renderer. Clerk's default after-sign-in URL
        // is the Account Portal root; the session cookies now live in this
        // partition, so a reload lets TokenBridge see the signed-in user.
        if (!window.isDestroyed()) {
          window.loadURL(authBaseUrl).catch((err) => {
            log.error('[Auth] Failed to reload auth renderer:', err);
          });
        }
      }
    });
    window.webContents.on('will-redirect', (event, url) => {
      const authBaseUrl = this.authWindowBaseUrl;
      if (!authBaseUrl) return;
      if (this.isOAuthProviderUrl(url) || isDesktopAuthNavigation(url, authBaseUrl)) return;
      event.preventDefault();
      log.warn('[Auth] Blocking redirect away from auth renderer to:', url);
      if (!window.isDestroyed()) {
        window.loadURL(authBaseUrl).catch((err) => {
          log.error('[Auth] Failed to reload auth renderer after redirect:', err);
        });
      }
    });

    window.loadURL(this.authWindowBaseUrl).catch((err) => {
      log.error('[Auth] Failed to load auth renderer:', err);
      this.clerkTokenRejecter?.(err instanceof Error ? err : new Error('Auth renderer failed to load'));
    });

    window.on('closed', () => {
      this.authWindow = null;
      if (this.clerkTokenRejecter) {
        this.clerkTokenRejecter(new Error('Sign-in window was closed.'));
        this.clerkTokenResolver = null;
        this.clerkTokenRejecter = null;
      }
    });

    this.authWindow = window;
    return window;
  }

  private closeAuthWindow(): void {
    log.info('[Auth] closeAuthWindow called; authWindow =', this.authWindow ? 'present' : 'null');
    if (this.authWindow && !this.authWindow.isDestroyed()) {
      this.authWindow.close();
    }
    this.authWindow = null;
  }

  /**
   * Decodes the domain the auth window should be served from, based on the
   * publishable key baked into clerk-config.json at build time
   * (pk_(test|live)_<base64(host)$>).
   *
   * We serve from the instance's Account Portal domain (accounts.<domain>)
   * rather than the bare instance domain: Clerk manages its Turnstile
   * CAPTCHA sitekey for the hosted-pages domain, so the widget only renders
   * there (elsewhere it fails with error 600010 "missing hostname in widget
   * configuration"). The Frontend API also accepts subdomains of the instance
   * domain as a valid origin for production keys.
   *
   * clerk.platform.example.com -> accounts.platform.example.com
   * foo-123.clerk.accounts.dev -> foo-123.accounts.dev
   */
  private clerkAuthServingDomain(): string | null {
    try {
      const configPath = join(__dirname, '../renderer/auth/clerk-config.json');
      const { publishableKey } = JSON.parse(fs.readFileSync(configPath, 'utf8')) as { publishableKey?: string };
      const encoded = publishableKey?.split('_')[2];
      if (!encoded) return null;
      const frontendApiHost = Buffer.from(encoded, 'base64').toString('utf8').replace(/\$+$/, '');
      if (!frontendApiHost) return null;
      return frontendApiHost.endsWith('.clerk.accounts.dev')
        ? frontendApiHost.replace('.clerk.accounts.dev', '.accounts.dev')
        : frontendApiHost.replace(/^clerk\./, 'accounts.');
    } catch (error) {
      log.warn('[Auth] Could not decode Clerk auth serving domain:', error);
      return null;
    }
  }

  /**
   * Prepares the dedicated session for the auth window. The auth renderer is
   * served from https://accounts.<instance-domain>/__desktop_auth__/ via
   * protocol interception, which makes Clerk's browser checks pass
   * legitimately: production keys accept subdomains of the instance domain as
   * request origin, and Clerk's Turnstile CAPTCHA sitekey is configured for
   * the hosted-pages (accounts.*) domain. All other https traffic passes
   * through untouched.
   */
  private prepareAuthSession(authDir: string, instanceDomain: string): Electron.Session {
    const authSession = session.fromPartition(AUTH_SESSION_PARTITION);
    if (!this.authSessionProtocolRegistered) {
      authSession.protocol.handle('https', (request) => {
        const url = new URL(request.url);
        if (url.host !== instanceDomain || !url.pathname.startsWith(AUTH_WINDOW_PATH_PREFIX)) {
          return this.forwardThroughAuthSession(authSession, request);
        }
        return this.serveAuthFile(authDir, url.pathname.slice(AUTH_WINDOW_PATH_PREFIX.length) || '/');
      });
      this.authSessionProtocolRegistered = true;
    }
    return authSession;
  }

  /**
   * Forward intercepted https (Clerk FAPI, captcha, anything not the local
   * auth renderer). `protocol.handle('https')` owns the whole scheme, so
   * Chromium does not attach partition cookies to the Request we receive, and
   * session.fetch() on this partition re-enters the handler (Clerk JS then
   * times out). net.fetch + bypassCustomProtocolHandlers hits the real
   * network; we copy Cookie / Set-Cookie against the auth jar ourselves.
   */
  private async forwardThroughAuthSession(
    authSession: Electron.Session,
    request: Request,
  ): Promise<Response> {
    try {
      const cookies = await authSession.cookies.get({ url: request.url });
      const init = await buildAuthForwardInit(request, cookies);
      const response = await net.fetch(request.url, init);
      for (const raw of setCookiesFromResponse(response)) {
        try {
          await setCookieOnSession(authSession, cookieFromSetCookieHeader(raw, request.url));
        } catch (error) {
          log.warn('[Auth] Failed to persist forwarded Set-Cookie:', error);
        }
      }
      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers: response.headers,
      });
    } catch (error) {
      log.error(`[Auth] https forward failed for ${request.url}:`, error);
      return new Response('Bad gateway', { status: 502 });
    }
  }

  private serveAuthFile(authDir: string, requestPath: string): Response {
    const target = requestPath === '/' ? '/index.html' : requestPath;
    const filePath = path.normalize(path.join(authDir, target));
    if (!filePath.startsWith(path.normalize(authDir + path.sep))) {
      return new Response('Forbidden', { status: 403 });
    }
    try {
      const data = fs.readFileSync(filePath);
      return new Response(data, {
        headers: {
          'Content-Type': AUTH_FILE_MIME_TYPES[path.extname(filePath).toLowerCase()] || 'application/octet-stream',
          'Cache-Control': 'no-cache',
        },
      });
    } catch {
      return new Response('Not found', { status: 404 });
    }
  }

  private isOAuthProviderUrl(url: string): boolean {
    try {
      const parsed = new URL(url);
      const hosts = new Set(['accounts.google.com', 'github.com']);
      return hosts.has(parsed.hostname) || parsed.hostname.endsWith('.google.com');
    } catch {
      return false;
    }
  }

  private requestClerkSession(options: { hidden?: boolean; timeoutMs?: number } = {}): Promise<ClerkSessionPayload> {
    return new Promise((resolve, reject) => {
      const timeoutMs = options.timeoutMs ?? PAIRING_TIMEOUT_MS;
      const timeout = setTimeout(() => {
        if (this.clerkTokenRejecter !== rejectWithCleanup) return;
        this.clerkTokenResolver = null;
        this.clerkTokenRejecter = null;
        if (options.hidden) this.closeAuthWindow();
        reject(new Error('Clerk session timed out'));
      }, timeoutMs);
      const resolveWithCleanup = (payload: ClerkSessionPayload) => {
        clearTimeout(timeout);
        resolve(payload);
      };
      const rejectWithCleanup = (error: Error) => {
        clearTimeout(timeout);
        reject(error);
      };
      this.clerkTokenResolver = resolveWithCleanup;
      this.clerkTokenRejecter = rejectWithCleanup;
      void this.openAuthWindow({
        hidden: options.hidden,
        preserveSession: options.hidden === true,
      }).catch(rejectWithCleanup);
    });
  }

  private cacheClerkToken(token: string): void {
    this.clerkTokens.remember(token);
  }

  /**
   * The Clerk session JWT without stalling on a refresh: a cached token comes
   * back at once (a near-expiry one refreshes in the background); with none,
   * wait at most `maxWaitMs`. One-shot flows that need a real token (pairing
   * approval, quit-time handoff) pass a longer wait.
   */
  async getClerkToken(maxWaitMs = REQUEST_CLERK_WAIT_MS): Promise<string | null> {
    return this.clerkTokens.get(maxWaitMs);
  }

  private async refreshClerkToken(): Promise<string> {
    if (this.pendingPairing) {
      const cached = this.clerkTokens.cachedToken();
      if (cached) return cached;
      throw new Error('Pairing in progress');
    }
    try {
      const clerk = await this.requestClerkSession({ hidden: true, timeoutMs: 20_000 });
      this.closeAuthWindow();
      const email = preferredEmail(clerk.email, emailFromJwt(clerk.token));
      if (email) this.applyAccountEmail(email);
      return clerk.token;
    } catch (error) {
      this.closeAuthWindow();
      throw error;
    }
  }

  private applyAccountEmail(email: string): void {
    if (isSyntheticEmail(email) || !this.runtimeIdentity || !this.session) return;
    if (this.session.userEmail === email) return;
    log.info('[Auth] Updating account email to', email);
    this.persistIdentity({ ...this.runtimeIdentity, userEmail: email });
    void this.upsertAccountRecord(this.session);
    for (const window of BrowserWindow.getAllWindows()) {
      if (window.isDestroyed() || window === this.authWindow) continue;
      window.webContents.send('auth:session-updated', {
        userId: this.session.userId,
        userEmail: email,
      });
    }
  }

  private async recoverAccountEmail(): Promise<void> {
    const verified = await this.emailFromVerifyToken();
    if (verified) {
      this.applyAccountEmail(verified);
      return;
    }
    try {
      const clerk = await this.requestClerkSession({ hidden: true, timeoutMs: 20_000 });
      this.closeAuthWindow();
      this.cacheClerkToken(clerk.token);
      const email = preferredEmail(clerk.email, emailFromJwt(clerk.token));
      if (email) this.applyAccountEmail(email);
    } catch (error) {
      log.warn('[Auth] Could not recover account email from Clerk:', error);
      this.closeAuthWindow();
    }
  }

  private async emailFromVerifyToken(): Promise<string | null> {
    if (!this.session?.accessToken) return null;
    try {
      const response = await fetch(`${cloudApiBaseUrl()}/api/v1/runtime-devices/verify-token`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${this.session.accessToken}` },
      });
      if (!response.ok) return null;
      const body = await response.json() as { email?: string };
      return preferredEmail(body.email);
    } catch (error) {
      log.warn('[Auth] verify-token email lookup failed:', error);
      return null;
    }
  }

  private async completePairingWithClerkToken(clerkToken: string, pending: PendingPairing): Promise<void> {
    const code = pending.pairing.userCode;
    const normalized = code.toUpperCase().replace(/[^A-Z2-9]/g, '').replace(/(.{4})(?=.)/, '$1-');
    log.info('[Auth] Completing pairing with Clerk token; looking up pairing code');

    // Verify the pairing exists.
    const lookup = await fetch(
      `${cloudApiBaseUrl()}/api/v1/runtime-pairings/code/${encodeURIComponent(normalized)}`,
      { headers: { Authorization: `Bearer ${clerkToken}` } },
    );
    log.info('[Auth] Pairing lookup status:', lookup.status);
    if (!lookup.ok) {
      throw new Error('This pairing request is invalid or expired. Please try again.');
    }

    // Approve it with the Clerk-authenticated user.
    const approve = await fetch(
      `${cloudApiBaseUrl()}/api/v1/runtime-pairings/code/${encodeURIComponent(normalized)}/approve`,
      {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${clerkToken}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({}),
      },
    );
    log.info('[Auth] Pairing approve status:', approve.status);
    if (!approve.ok) {
      const payload = await approve.json().catch(() => ({})) as { message?: string; error?: string };
      throw new Error(payload.message || payload.error || 'Allternit could not approve this runtime.');
    }

    // Exchange for the device credential. The exchange may return 428 if the
    // approve transaction hasn't committed yet, so start polling after the first
    // attempt instead of giving up and leaving the pending pairing stranded.
    log.info('[Auth] Pairing approved; exchanging for device credential');
    const exchanged = await this.exchangePendingPairing(pending);
    if (!exchanged) {
      log.info('[Auth] Exchange not ready; starting pairing poll');
      void this.pollPairing(pending);
    }
  }



  private async pollPairing(pending: PendingPairing): Promise<void> {
    const interval = Math.max(pending.pairing.pollIntervalSeconds || 2, 2) * 1000;
    while (this.pendingPairing === pending) {
      const completed = await this.exchangePendingPairing(pending);
      if (completed || this.pendingPairing !== pending) return;
      await sleep(interval);
    }
  }

  private async exchangePendingPairing(pending: PendingPairing): Promise<boolean> {
    if (pending.exchangeInFlight) {
      log.info('[Auth] Skipping exchange; another exchange is already in flight');
      return false;
    }
    if (this.pendingPairing !== pending) {
      log.info('[Auth] Skipping exchange; pending pairing changed');
      return false;
    }
    pending.exchangeInFlight = true;
    try {
      const message = pairingSignatureMessage(pending.pairing.pairingId, pending.pairing.challenge);
      const signature = sign(null, Buffer.from(message, 'utf8'), pending.privateKeyPem).toString('base64url');
      const response = await fetch(`${cloudApiBaseUrl()}/api/v1/runtime-pairings/exchange`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          pairingId: pending.pairing.pairingId,
          deviceCode: pending.pairing.deviceCode,
          signature,
        }),
      });
      log.info('[Auth] Pairing exchange status:', response.status);
      if (response.status === 428) return false;
      if (!response.ok) {
        const payload = await response.json().catch(() => null) as { error?: string; message?: string } | null;
        log.warn('[Auth] Pairing exchange failed:', response.status, payload);
        if (response.status >= 500) return false;
        throw new Error(payload?.message || payload?.error || `Pairing failed (${response.status})`);
      }

      const exchange = await response.json() as PairingExchangeResponse;
      const identity: PersistedRuntimeIdentity = {
        accessToken: exchange.deviceToken,
        refreshToken: '',
        tokenType: exchange.tokenType || 'Bearer',
        scope: exchange.capabilities.join(' '),
        expiresAt: new Date(exchange.expiresAt).getTime(),
        userId: exchange.userId,
        userEmail: preferredEmail(this.pendingClerkEmail, exchange.userEmail) ?? exchange.userEmail,
        clientId: RUNTIME_CLIENT_ID,
        runtimeId: exchange.runtimeId,
        organizationId: exchange.organizationId,
        capabilities: exchange.capabilities,
        privateKeyPem: pending.privateKeyPem,
        publicKey: pending.publicKey,
        savedAt: new Date().toISOString(),
      };
      this.persistIdentity(identity);
      await this.upsertAccountRecord(identity);
      this.pendingClerkEmail = null;
      clearTimeout(pending.timeout);
      this.pendingPairing = null;
      pending.resolve(this.toSession(identity));
      this.scheduleHeartbeat();
      this.connectRuntimeRelay();
      log.info('[Auth] Pairing exchange completed; identity saved for user', identity.userId);
      return true;
    } catch (error) {
      if (error instanceof TypeError) {
        log.warn('[Auth] Pairing poll could not reach Allternit Cloud; retrying');
        return false;
      }
      this.clearPendingPairing(error instanceof Error ? error : new Error('Pairing failed'));
      return true;
    } finally {
      pending.exchangeInFlight = false;
    }
  }

  private async refreshSessionIfNeeded(): Promise<void> {
    if (!this.session || this.session.expiresAt - Date.now() > ROTATION_SKEW_MS) return;
    if (this.refreshInFlight) return this.refreshInFlight;
    this.refreshInFlight = this.rotateCredential();
    try {
      await this.refreshInFlight;
    } finally {
      this.refreshInFlight = null;
    }
  }

  private async rotateCredential(): Promise<void> {
    if (!this.session || !this.runtimeIdentity) return;
    const response = await fetch(
      `${cloudApiBaseUrl()}/api/v1/runtime-devices/${encodeURIComponent(this.session.runtimeId)}/rotate`,
      { method: 'POST', headers: { Authorization: `Bearer ${this.session.accessToken}` } },
    );
    if (response.status === 401 || response.status === 403) {
      this.clearSession();
      await this.clearCurrentAccountFlag();
      throw new Error('This runtime was revoked. Pair it with Allternit again.');
    }
    if (!response.ok) throw new Error(`Runtime credential rotation failed (${response.status})`);
    const rotated = await response.json() as { deviceToken: string; expiresAt: string };
    this.persistIdentity({
      ...this.runtimeIdentity,
      accessToken: rotated.deviceToken,
      expiresAt: new Date(rotated.expiresAt).getTime(),
      savedAt: new Date().toISOString(),
    });
    this.reconnectRuntimeRelay();
  }

  private scheduleHeartbeat(): void {
    if (this.heartbeatTimer) clearInterval(this.heartbeatTimer);
    if (!this.session) return;
    this.heartbeatTimer = setInterval(() => {
      void this.sendHeartbeat().catch((error) => log.warn('[Auth] Runtime heartbeat deferred:', error));
    }, HEARTBEAT_INTERVAL_MS);
  }

  private async sendHeartbeat(): Promise<void> {
    if (!this.session) return;
    const response = await fetch(
      `${cloudApiBaseUrl()}/api/v1/runtime-devices/${encodeURIComponent(this.session.runtimeId)}/heartbeat`,
      { method: 'POST', headers: { Authorization: `Bearer ${this.session.accessToken}` } },
    );
    if (response.status === 401 || response.status === 403) {
      this.clearSession();
      await this.clearCurrentAccountFlag();
      this.notifySplash('auth:login-failed', 'This runtime was revoked. Pair it again to continue.');
      return;
    }
    if (response.status === 429) {
      log.warn('[Auth] Runtime heartbeat rate-limited; will retry on the next interval');
      return;
    }
    if (!response.ok) throw new Error(`Runtime heartbeat failed (${response.status})`);
  }

  private persistIdentity(identity: PersistedRuntimeIdentity): void {
    this.runtimeIdentity = identity;
    this.session = this.toSession(identity);
    fs.mkdirSync(path.dirname(this.identityPath), { recursive: true });
    fs.writeFileSync(this.identityPath, this.encodeSecret(JSON.stringify(identity)));
    this.persistDaemonIdentity(identity);
  }

  /**
   * The node daemon (cmd/allternit-node, `node.core`) reads a plain-JSON copy
   * of the runtime identity from ~/.config/allternit/runtime-identity.json.
   * The desktop app is the single writer: pair/rotate rewrites this file, and
   * clearSession removes it so a revoked runtime leaves no creds the daemon
   * can adopt. Daemon serde field names are camelCase runtimeId/deviceToken/
   * userId/expiresAt (expiresAt RFC3339 — a numeric epoch is rejected).
   */
  private daemonIdentityPath = path.join(os.homedir(), '.config', 'allternit', 'runtime-identity.json');

  private persistDaemonIdentity(identity: PersistedRuntimeIdentity): void {
    try {
      const payload = {
        runtimeId: identity.runtimeId,
        deviceToken: identity.accessToken,
        userId: identity.userId,
        expiresAt: new Date(identity.expiresAt).toISOString(),
      };
      fs.mkdirSync(path.dirname(this.daemonIdentityPath), { recursive: true });
      fs.writeFileSync(this.daemonIdentityPath, JSON.stringify(payload), { mode: 0o600 });
    } catch (error) {
      log.warn('[Auth] Failed to write daemon identity file (desktop remains the source of truth):', error);
    }
  }

  private removeDaemonIdentity(): void {
    try {
      fs.rmSync(this.daemonIdentityPath, { force: true });
    } catch (error) {
      log.warn('[Auth] Failed to remove daemon identity file:', error);
    }
  }

  private readIdentityFromDisk(): PersistedRuntimeIdentity | null {
    if (!fs.existsSync(this.identityPath)) return null;
    try {
      const parsed = JSON.parse(this.decodeSecret(fs.readFileSync(this.identityPath))) as PersistedRuntimeIdentity;
      if (!parsed.runtimeId || !parsed.accessToken || !parsed.privateKeyPem || !parsed.userId) {
        throw new Error('Runtime identity is incomplete');
      }
      return parsed;
    } catch (error) {
      log.warn('[Auth] Runtime identity is unreadable:', error);
      this.quarantineCorruptFile(this.identityPath);
      return null;
    }
  }

  private toSession(identity: PersistedRuntimeIdentity): DesktopAuthSession {
    const { privateKeyPem: _privateKeyPem, publicKey: _publicKey, savedAt: _savedAt, ...session } = identity;
    return session;
  }

  private clearSession(): void {
    this.session = null;
    this.runtimeIdentity = null;
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
    if (this.relayReconnectTimer) {
      clearTimeout(this.relayReconnectTimer);
      this.relayReconnectTimer = null;
    }
    this.stopRelayWatchdog();
    if (this.relaySocket) {
      this.relaySocket.close();
      this.relaySocket = null;
    }
    fs.rmSync(this.identityPath, { force: true });
    this.removeDaemonIdentity();
  }

  private connectRuntimeRelay(): void {
    if (!this.session || this.relaySocket) return;
    const runtimeId = this.session.runtimeId;
    const relayUrl = new URL(`${cloudApiBaseUrl()}/api/v1/runtime-relay/connect/${encodeURIComponent(runtimeId)}`);
    relayUrl.protocol = relayUrl.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(relayUrl.toString());
    this.relaySocket = socket;
    this.relayLastMessageAt = Date.now();
    this.startRelayWatchdog();

    socket.addEventListener('open', () => {
      if (!this.session || this.session.runtimeId !== runtimeId) return socket.close();
      socket.send(JSON.stringify({
        type: 'authenticate',
        runtime_id: runtimeId,
        device_token: this.session.accessToken,
      }));
    });
    socket.addEventListener('message', (event: { data: unknown }) => {
      this.relayLastMessageAt = Date.now();
      const text = typeof event.data === 'string' ? event.data : Buffer.from(event.data as ArrayBuffer).toString('utf8');
      let message: any;
      try { message = JSON.parse(text); } catch { return; }
      if (message.type === 'authenticated') {
        this.relayReconnectDelayMs = 1_000;
        log.info('[Auth] Paired runtime relay connected');
      } else if (message.type === 'request') {
        void this.handleRelayRequest(socket, message);
      } else if (message.type === 'socket_open') {
        this.handleRelaySocketOpen(socket, message);
      } else if (message.type === 'socket_data') {
        this.handleRelaySocketData(message);
      } else if (message.type === 'socket_close') {
        this.handleRelaySocketClose(message);
      } else if (message.type === 'ping') {
        socket.send(JSON.stringify({ type: 'pong' }));
      }
    });
    socket.addEventListener('close', () => {
      if (this.relaySocket === socket) this.relaySocket = null;
      this.stopRelayWatchdog();
      for (const local of this.relayLocalSockets.values()) local.close(1012, 'Relay disconnected');
      this.relayLocalSockets.clear();
      if (this.session) this.scheduleRelayReconnect();
    });
    socket.addEventListener('error', (error: unknown) => {
      log.warn('[Auth] Runtime relay connection error:', error);
    });
  }

  /**
   * Heartbeat watchdog for the runtime relay socket. The cloud relay pings
   * every 25s; a silently dead connection (laptop sleep without a TCP reset,
   * network change, NAT timeout) never fires `close`, so without this the
   * node could stay dark until an app restart. Closing the stale socket
   * triggers the `close` handler, which schedules the normal reconnect.
   */
  private startRelayWatchdog(): void {
    this.stopRelayWatchdog();
    this.relayWatchdogTimer = setInterval(() => {
      if (!this.relaySocket) {
        this.stopRelayWatchdog();
        return;
      }
      if (Date.now() - this.relayLastMessageAt > 75_000) {
        log.warn('[Auth] Runtime relay heartbeat timed out; reconnecting');
        this.relaySocket.close(4000, 'Relay heartbeat timeout');
      }
    }, 30_000);
    this.relayWatchdogTimer.unref?.();
  }

  private stopRelayWatchdog(): void {
    if (this.relayWatchdogTimer) {
      clearInterval(this.relayWatchdogTimer);
      this.relayWatchdogTimer = null;
    }
  }

  private async handleRelayRequest(socket: any, message: any): Promise<void> {
    const requestId = typeof message.request_id === 'string' ? message.request_id : '';
    const method = typeof message.method === 'string' ? message.method.toUpperCase() : 'GET';
    const requestPath = typeof message.path === 'string' ? message.path : '';
    if (!requestId || !requestPath.startsWith('/') || requestPath.includes('..') || requestPath.includes('://')) return;
    const allowedPrefixes = [
      '/api/', '/viz', '/sandbox', '/vm-session', '/rails', '/stream',
      '/terminal', '/mcp', '/platform', '/metrics', '/alabs', '/cowork',
      '/webhooks', '/status', '/health',
      '/ws', '/panes',
      '/v1/',
    ];
    if (!allowedPrefixes.some((prefix) => requestPath.startsWith(prefix))) return;

    try {
      const headers = new Headers(message.headers || {});
      const session = this.session;
      if (!session) throw new Error('Runtime is no longer paired');
      headers.set('Authorization', `Bearer ${session.accessToken}`);
      headers.set('X-Allternit-Desktop-Access-Token', session.accessToken);
      // Another device's request: the API checks that device, not this one.
      headers.set('X-Allternit-Relayed', '1');
      headers.set('X-Allternit-User-Id', session.userId);
      headers.set('X-Allternit-User-Email', session.userEmail);
      if (session.organizationId) headers.set('X-Allternit-Tenant-Id', session.organizationId);
      const body = method === 'GET' || method === 'HEAD' || !message.body
        ? undefined
        : message.body_encoding === 'base64'
          ? Buffer.from(message.body, 'base64')
          : Buffer.from(message.body, 'utf8');

      const fabricPayload = this.fabricRelayPayload(method, requestPath, body);
      if (fabricPayload) {
        this.sendRelayJson(socket, requestId, 200, fabricPayload);
        return;
      }
      if (requestPath === '/api/v1/session-worker/invoke' && method === 'POST') {
        const payload = JSON.parse(body?.toString('utf8') || '{}') as { capability?: string; inputs?: Record<string, unknown> };
        const result = await this.invokeNodeHarness(payload.capability || '', payload.inputs || {});
        this.sendRelayJson(socket, requestId, 200, { result });
        return;
      }

      const localUrl = this.relayLocalUrl(requestPath);
      const response = await fetch(localUrl, { method, headers, body });
      const responseHeaders: Record<string, string> = {};
      for (const name of ['content-type', 'cache-control', 'content-disposition', 'etag', 'last-modified', 'x-request-id']) {
        const value = response.headers.get(name);
        if (value) responseHeaders[name] = value;
      }
      socket.send(JSON.stringify({
        type: 'response_start',
        request_id: requestId,
        status: response.status,
        headers: responseHeaders,
      }));
      if (response.body) {
        const reader = response.body.getReader();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          if (value?.byteLength) {
            socket.send(JSON.stringify({
              type: 'response_chunk',
              request_id: requestId,
              body: Buffer.from(value).toString('base64'),
              body_encoding: 'base64',
            }));
          }
        }
      }
      socket.send(JSON.stringify({ type: 'response_end', request_id: requestId }));
    } catch (error) {
      socket.send(JSON.stringify({
        type: 'response_start',
        request_id: requestId,
        status: 502,
        headers: { 'content-type': 'application/json' },
      }));
      socket.send(JSON.stringify({
        type: 'response_chunk',
        request_id: requestId,
        body: Buffer.from(JSON.stringify({ error: 'runtime_proxy_error', message: String(error) })).toString('base64'),
        body_encoding: 'base64',
      }));
      socket.send(JSON.stringify({ type: 'response_end', request_id: requestId }));
    }
  }

  private sendRelayJson(socket: any, requestId: string, status: number, payload: unknown): void {
    const body = Buffer.from(JSON.stringify(payload));
    socket.send(JSON.stringify({
      type: 'response_start',
      request_id: requestId,
      status,
      headers: { 'content-type': 'application/json' },
    }));
    socket.send(JSON.stringify({
      type: 'response_chunk',
      request_id: requestId,
      body: body.toString('base64'),
      body_encoding: 'base64',
    }));
    socket.send(JSON.stringify({ type: 'response_end', request_id: requestId }));
  }

  private localFabricPeer(): Record<string, unknown> {
    const hostname = os.hostname() || 'Allternit Desktop';
    return {
      id: 'local-desktop',
      nodeId: 'local-desktop',
      name: hostname,
      hostname,
      status: 'online',
      runtimeType: 'desktop',
      platform: process.platform,
      endpoints: [{ transport: 'loopback', url: URLS.GIZZI, priority: 0 }],
      capabilities: [
        'harness.session',
        'harness.session.get',
        'harness.session.create',
        'harness.session.message',
        'harness.session.abort',
        'runtime:connect',
        'runtime:remote_control',
      ],
      resources: [],
    };
  }

  private fabricRelayPayload(method: string, requestPath: string, body?: Buffer): unknown | null {
    const pathOnly = requestPath.split('?')[0];
    if (pathOnly === '/api/v1/fabric/leases' && method === 'POST') {
      const payload = JSON.parse(body?.toString('utf8') || '{}') as { capabilityId?: string; grantee?: string; ttlSeconds?: number };
      return {
        id: 'lease-desktop-local',
        capabilityId: payload.capabilityId ?? 'harness.session',
        grantee: payload.grantee ?? 'web-client',
        ttlSeconds: payload.ttlSeconds ?? 300,
        status: 'active',
        signature: 'desktop-local',
        issuedAt: new Date().toISOString(),
      };
    }
    if (method !== 'GET') return null;
    const peer = this.localFabricPeer();
    if (pathOnly === '/api/v1/fabric/peers/local') return peer;
    if (pathOnly === '/api/v1/fabric/peers') return [peer];
    if (pathOnly === '/api/v1/fabric/directory') return { local: peer, peers: [peer] };
    if (pathOnly === '/api/v1/fabric/workers/self') {
      return {
        name: 'desktop-session-worker',
        version: app.getVersion(),
        capabilities: peer.capabilities,
      };
    }
    return null;
  }

  private relayLocalUrl(requestPath: string): string {
    const desktopPrefix = '/v1/remote-control/desktop';
    if (requestPath.startsWith(desktopPrefix)) {
      return `http://127.0.0.1:8477${requestPath.slice(desktopPrefix.length) || '/'}`;
    }
    if (requestPath.startsWith('/v1/browser-skills')) {
      return `${URLS.ACU}${requestPath}`;
    }
    if (requestPath.startsWith('/api/v1/remote-control/')) {
      return `${URLS.GIZZI}${requestPath.replace('/api/v1/remote-control/', '/v1/remote-control/')}`;
    }
    if (requestPath.startsWith('/v1/remote-control/')) return `${URLS.GIZZI}${requestPath}`;
    if (requestPath.startsWith('/api/v1/providers')) return `${URLS.GIZZI}/v1/provider`;
    if (requestPath.startsWith('/api/v1/agents')) return `${URLS.GIZZI}/v1/agent/list`;
    if (
      requestPath.startsWith('/v1/provider')
      || requestPath.startsWith('/v1/agent')
      || requestPath.startsWith('/v1/permission')
      || requestPath.startsWith('/v1/question')
      || requestPath === '/v1/session'
      || requestPath.startsWith('/v1/session/')
    ) {
      return `${URLS.GIZZI}${requestPath}`;
    }
    return `${URLS.API}${requestPath}`;
  }

  private async invokeNodeHarness(capability: string, inputs: Record<string, unknown>): Promise<unknown> {
    const gizzi = URLS.GIZZI;
    const sessionId = typeof inputs.sessionID === 'string' ? inputs.sessionID : '';
    const getJson = async (path: string, init?: RequestInit) => {
      const res = await fetch(`${gizzi}${path}`, init);
      const text = await res.text();
      if (!res.ok) throw new Error(`${res.status} ${path}: ${text.slice(0, 240)}`);
      return text ? JSON.parse(text) : null;
    };
    switch (capability) {
      case 'harness.session':
        return getJson('/v1/remote-control/sessions');
      case 'harness.session.get':
        return getJson(`/v1/remote-control/sessions/${encodeURIComponent(sessionId)}`);
      case 'harness.session.message':
        return getJson(`/v1/remote-control/sessions/${encodeURIComponent(sessionId)}/messages`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            text: inputs.text,
            attachments: inputs.attachments,
            agent: inputs.agent,
            model: inputs.model,
          }),
        });
      case 'harness.session.abort':
        return getJson(`/v1/remote-control/sessions/${encodeURIComponent(sessionId)}/abort`, { method: 'POST' });
      case 'harness.session.create':
        return getJson('/v1/session', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(inputs),
        });
      case 'harness.session.permissions.list':
        return getJson('/v1/permission');
      case 'harness.session.questions.list':
        return getJson('/v1/question');
      case 'harness.session.permissions.reply':
        return getJson(`/v1/permission/${encodeURIComponent(String(inputs.requestID))}/reply`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ reply: inputs.reply, message: inputs.message }),
        });
      case 'harness.session.questions.reply':
        return getJson(`/v1/question/${encodeURIComponent(String(inputs.requestID))}/reply`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ answers: inputs.answers }),
        });
      case 'harness.session.questions.reject':
        return getJson(`/v1/question/${encodeURIComponent(String(inputs.requestID))}/reject`, { method: 'POST' });
      default:
        throw new Error(`unsupported harness capability ${capability}`);
    }
  }

  private handleRelaySocketOpen(relay: WebSocket, message: any): void {
    const socketId = typeof message.socket_id === 'string' ? message.socket_id : '';
    const requestPath = typeof message.path === 'string' ? message.path : '';
    const allowedPrefixes = [
      '/api/', '/viz', '/sandbox', '/vm-session', '/rails', '/stream',
      '/terminal', '/mcp', '/platform', '/metrics', '/alabs', '/cowork',
      '/webhooks', '/ws', '/panes', '/status', '/health',
      '/v1/',
    ];
    if (!socketId || !requestPath.startsWith('/') || requestPath.includes('..')
      || requestPath.includes('://') || !allowedPrefixes.some((prefix) => requestPath.startsWith(prefix))) return;
    this.relayLocalSockets.get(socketId)?.close();
    const session = this.session;
    if (!session) return;
    const local = new WebSocket(`${URLS.API.replace(/^http/, 'ws')}${requestPath}`, {
      headers: {
        Authorization: `Bearer ${session.accessToken}`,
        'X-Allternit-Desktop-Access-Token': session.accessToken,
        'X-Allternit-Relayed': '1',
        'X-Allternit-User-Id': session.userId,
        'X-Allternit-User-Email': session.userEmail,
        ...(session.organizationId ? { 'X-Allternit-Tenant-Id': session.organizationId } : {}),
      },
    });
    this.relayLocalSockets.set(socketId, local);
    local.on('open', () => {
      if (relay.readyState === WebSocket.OPEN) relay.send(JSON.stringify({ type: 'socket_ready', socket_id: socketId }));
    });
    local.on('message', (data: any, isBinary: boolean) => {
      if (relay.readyState !== WebSocket.OPEN) return;
      relay.send(JSON.stringify({
        type: 'socket_data',
        socket_id: socketId,
        body: isBinary ? Buffer.from(data).toString('base64') : data.toString(),
        body_encoding: isBinary ? 'base64' : 'utf8',
      }));
    });
    local.on('close', (code: number, reason: Buffer) => {
      this.relayLocalSockets.delete(socketId);
      if (relay.readyState === WebSocket.OPEN) {
        relay.send(JSON.stringify({ type: 'socket_close', socket_id: socketId, code, reason: reason.toString() }));
      }
    });
    local.on('error', (error: Error) => {
      log.warn('[Auth] Local runtime socket error:', error.message);
      local.close(1011, 'Local runtime socket failed');
    });
  }

  private handleRelaySocketData(message: any): void {
    const local = this.relayLocalSockets.get(message.socket_id);
    if (!local || local.readyState !== WebSocket.OPEN) return;
    local.send(message.body_encoding === 'base64'
      ? Buffer.from(String(message.body || ''), 'base64')
      : String(message.body || ''));
  }

  private handleRelaySocketClose(message: any): void {
    const local = this.relayLocalSockets.get(message.socket_id);
    if (!local) return;
    this.relayLocalSockets.delete(message.socket_id);
    const code = Number(message.code) >= 1000 && Number(message.code) <= 4999 ? Number(message.code) : 1000;
    local.close(code, String(message.reason || ''));
  }

  private scheduleRelayReconnect(): void {
    if (this.relayReconnectTimer || !this.session) return;
    const delay = this.relayReconnectDelayMs;
    this.relayReconnectDelayMs = Math.min(this.relayReconnectDelayMs * 2, 30_000);
    this.relayReconnectTimer = setTimeout(() => {
      this.relayReconnectTimer = null;
      this.connectRuntimeRelay();
    }, delay);
  }

  private reconnectRuntimeRelay(): void {
    if (this.relaySocket) {
      this.relaySocket.close();
      this.relaySocket = null;
    }
    for (const socket of this.relayLocalSockets.values()) socket.close(1012, 'Relay reconnecting');
    this.relayLocalSockets.clear();
    this.scheduleRelayReconnect();
  }

  private clearPendingPairing(error?: Error): void {
    const pending = this.pendingPairing;
    if (!pending) return;
    clearTimeout(pending.timeout);
    this.pendingPairing = null;
    if (error) pending.reject(error);
  }

  private async upsertAccountRecord(session: DesktopAuthSession): Promise<void> {
    const accounts = this.readAccountsFromDisk();
    const now = new Date().toISOString();
    let found = false;
    const next = accounts.map((account) => {
      if (account.userId !== session.userId) return { ...account, current: false };
      found = true;
      return {
        ...account,
        userEmail: preferredEmail(session.userEmail, account.userEmail) ?? session.userEmail,
        clientId: session.runtimeId,
        current: true,
        lastSeenAt: now,
        lastSignedInAt: account.lastSignedInAt || now,
      };
    });
    if (!found) {
      next.push({
        userId: session.userId,
        userEmail: preferredEmail(session.userEmail) ?? session.userEmail,
        clientId: session.runtimeId,
        lastSignedInAt: now,
        lastSeenAt: now,
        current: true,
      });
    }
    this.writeAccountsToDisk(next);
  }

  private async clearCurrentAccountFlag(): Promise<void> {
    this.writeAccountsToDisk(this.readAccountsFromDisk().map((account) => (
      account.current ? { ...account, current: false } : account
    )));
  }

  private readAccountsFromDisk(): DesktopAccountRecord[] {
    if (!fs.existsSync(this.accountsPath)) return [];
    try {
      const raw = fs.readFileSync(this.accountsPath);
      let json: string;
      try {
        json = raw.toString('utf8');
        JSON.parse(json);
      } catch {
        json = this.decodeLegacyLocalEncryption(raw);
      }
      const parsed = JSON.parse(json);
      return Array.isArray(parsed) ? parsed.filter((account) => (
        account && typeof account.userId === 'string' && typeof account.userEmail === 'string'
      )) : [];
    } catch (error) {
      log.warn('[Auth] Account registry is unreadable:', error);
      this.quarantineCorruptFile(this.accountsPath);
      return [];
    }
  }

  private writeAccountsToDisk(accounts: DesktopAccountRecord[]): void {
    fs.mkdirSync(path.dirname(this.accountsPath), { recursive: true });
    fs.writeFileSync(this.accountsPath, JSON.stringify(accounts, null, 2), { mode: 0o600 });
  }

  private encodeSecret(value: string): Buffer {
    // In production Electron's signed main process is the sole Keychain owner.
    // Dev builds use authenticated local encryption so rebuilding the API or
    // Electron does not cause repeated macOS Keychain prompts.
    if (app.isPackaged && safeStorage.isEncryptionAvailable()) {
      return Buffer.from(`${SAFE_STORAGE_HEADER}${safeStorage.encryptString(value).toString('base64')}`, 'utf8');
    }
    const key = this.localHardwareKey();
    const iv = randomBytes(12);
    const cipher = createCipheriv('aes-256-gcm', key, iv);
    const ciphertext = Buffer.concat([cipher.update(value, 'utf8'), cipher.final()]);
    const payload = Buffer.concat([iv, cipher.getAuthTag(), ciphertext]).toString('base64');
    return Buffer.from(`${LOCAL_STORAGE_HEADER}${payload}`, 'utf8');
  }

  private decodeSecret(raw: Buffer): string {
    const value = raw.toString('utf8');
    if (value.startsWith(SAFE_STORAGE_HEADER)) {
      if (!safeStorage.isEncryptionAvailable()) throw new Error('macOS credential storage is unavailable');
      return safeStorage.decryptString(Buffer.from(value.slice(SAFE_STORAGE_HEADER.length), 'base64'));
    }
    if (value.startsWith(LOCAL_STORAGE_HEADER)) {
      const payload = Buffer.from(value.slice(LOCAL_STORAGE_HEADER.length), 'base64');
      const iv = payload.subarray(0, 12);
      const tag = payload.subarray(12, 28);
      const ciphertext = payload.subarray(28);
      const decipher = createDecipheriv('aes-256-gcm', this.localHardwareKey(), iv);
      decipher.setAuthTag(tag);
      return Buffer.concat([decipher.update(ciphertext), decipher.final()]).toString('utf8');
    }
    throw new Error('Unknown runtime identity format');
  }

  private localHardwareKey(): Buffer {
    // Deliberately excludes os.hostname(): on macOS it flips between the mDNS
    // form ("foo.local") and the DHCP/ISP-assigned form ("foo.hsd1.mn.comcast.net")
    // depending on network state, which changed this key across relaunches,
    // failed GCM auth-tag verification on otherwise-valid secrets, and caused
    // readIdentityFromDisk() to quarantine a good runtime-identity.json as
    // "corrupt" — silently forcing re-pairing on every network change.
    return createHash('sha256')
      .update(os.userInfo().username)
      .update(os.platform())
      .update(os.arch())
      .update('allternit-desktop-runtime-v2')
      .digest();
  }

  private decodeLegacyLocalEncryption(raw: Buffer): string {
    const key = createHash('sha256')
      .update(os.hostname())
      .update(os.userInfo().username)
      .update(os.platform())
      .update(os.arch())
      .update('allternit-desktop-salt-v1')
      .digest();
    const decipher = createDecipheriv('aes-256-cbc', key, raw.subarray(0, 16));
    return Buffer.concat([decipher.update(raw.subarray(16)), decipher.final()]).toString('utf8');
  }

  private quarantineLegacyOAuthSession(): void {
    if (!fs.existsSync(this.legacySessionPath)) return;
    try {
      fs.renameSync(this.legacySessionPath, `${this.legacySessionPath}.legacy-oauth-${Date.now()}`);
      log.info('[Auth] Retired legacy desktop OAuth session; runtime pairing is required once');
    } catch (error) {
      log.warn('[Auth] Could not retire legacy OAuth session:', error);
    }
  }

  private quarantineCorruptFile(filePath: string): void {
    if (!fs.existsSync(filePath)) return;
    try {
      fs.renameSync(filePath, `${filePath}.corrupt-${Date.now()}`);
    } catch {
      fs.rmSync(filePath, { force: true });
    }
  }

  private notifySplash(channel: string, message: string): void {
    const window = this.splashWindow
      || BrowserWindow.getAllWindows().find((candidate) => candidate.isVisible() && !candidate.isDestroyed());
    if (window && !window.isDestroyed()) window.webContents.send(channel, message);
  }
}

export const authManager = new DesktopAuthManager();
