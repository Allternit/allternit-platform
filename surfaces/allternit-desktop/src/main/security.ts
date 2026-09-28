/**
 * Allternit Desktop — main-process security helpers
 *
 * One guarded path for the security-sensitive surface:
 *  - openExternalAllowlisted(): scheme allowlist for shell.openExternal
 *  - assertTrustedSender(): sender-frame origin validation for sensitive IPC
 *  - installNavigationGuards(): will-navigate allowlist + guarded window.open
 *  - installSessionSecurityHandlers(): CSP via onHeadersReceived and a
 *    default-deny permission handler for app-owned origins.
 *
 * Renderer content that is NOT app-owned (Browser Mode webviews, mini-app
 * sandbox windows, Clerk/oauth popups) loads in the same default session, so
 * the session-wide handlers scope their policy by origin and leave every
 * other origin untouched.
 */

import {
  desktopCapturer,
  screen,
  shell,
  type DesktopCapturerSource,
  type DisplayMediaRequestHandlerHandlerRequest,
  type IpcMainEvent,
  type IpcMainInvokeEvent,
  type Session,
  type Streams,
  type WebContents,
} from 'electron';
import log from 'electron-log';

/** Schemes a renderer-initiated openExternal is allowed to use. */
const ALLOWED_EXTERNAL_SCHEMES = new Set(['https:', 'http:', 'mailto:']);

/** Custom protocol documents are first-party app content. */
const ALLOWED_APP_SCHEMES = new Set(['allternit-api:', 'allternit-gizzi:']);

export interface SecurityConfig {
  isDev: boolean;
  /**
   * Origins that host first-party app renderers (platform UI, local static
   * UI, HUD/office/design windows, and in dev the Vite dev server). Evaluated
   * per call because the platform URL is resolved during initialization.
   */
  getAppOrigins: () => string[];
}

let config: SecurityConfig | null = null;

export function configureSecurity(next: SecurityConfig): void {
  config = next;
}

/**
 * Open an external URL in the system browser, gated by a scheme allowlist.
 * Returns true when the URL was handed to the OS. Denials are logged with the
 * raw URL so abuse attempts leave an audit trail.
 */
export function openExternalAllowlisted(rawUrl: string): boolean {
  try {
    const url = new URL(rawUrl);
    if (!ALLOWED_EXTERNAL_SCHEMES.has(url.protocol)) {
      log.warn(`[Security] Blocked openExternal: disallowed scheme "${url.protocol}" for ${rawUrl}`);
      return false;
    }
    void shell.openExternal(rawUrl);
    return true;
  } catch {
    log.warn(`[Security] Blocked openExternal: malformed URL ${rawUrl}`);
    return false;
  }
}

function trustedOrigins(): Set<string> {
  const origins = new Set<string>();
  for (const origin of config?.getAppOrigins() ?? []) {
    try {
      if (origin) origins.add(new URL(origin).origin);
    } catch {
      // Ignore malformed configured origins.
    }
  }
  return origins;
}

/** True when the URL belongs to a first-party app origin (or app scheme). */
export function isTrustedAppUrl(rawUrl: string): boolean {
  try {
    const url = new URL(rawUrl);
    if (ALLOWED_APP_SCHEMES.has(url.protocol)) return true;
    return trustedOrigins().has(url.origin);
  } catch {
    return false;
  }
}

/**
 * Validate that an IPC message arrived from a first-party renderer frame.
 * Throws with a non-enumerable-channel-safe message so ipcMain.handle
 * surfaces the rejection to the caller instead of executing the handler.
 */
export function assertTrustedSender(
  event: IpcMainEvent | IpcMainInvokeEvent,
  channel: string,
): void {
  const frameUrl = event.senderFrame?.url ?? '';
  if (isTrustedAppUrl(frameUrl)) return;
  log.warn(`[Security] Rejected IPC "${channel}" from untrusted sender frame: ${frameUrl || '(unknown)'}`);
  throw new Error(`Untrusted IPC sender for channel "${channel}"`);
}

/**
 * will-navigate: keep each app window on its own current origin or another
 * first-party origin. External/top-level navigations are cancelled and
 * logged; they cannot hijack an app window into attacker content.
 */
export function installWillNavigateGuard(webContents: WebContents): void {
  webContents.on('will-navigate', (event, url) => {
    if (isTrustedAppUrl(url)) return;
    const current = webContents.getURL();
    try {
      if (current && new URL(url).origin === new URL(current).origin) return;
    } catch {
      // Fall through to deny.
    }
    log.warn(`[Security] Blocked navigation to untrusted URL: ${url}`);
    event.preventDefault();
  });
}

/**
 * window.open for windows that have no more specific handler: in-app targets
 * stay in-app; everything else is denied in-window and routed through the
 * allowlisted external opener.
 */
export function installWindowOpenGuard(webContents: WebContents): void {
  webContents.setWindowOpenHandler(({ url }) => {
    if (isTrustedAppUrl(url)) {
      return { action: 'allow' };
    }
    openExternalAllowlisted(url);
    return { action: 'deny' };
  });
}

/**
 * Session-wide response-header policy, scoped to first-party app origins.
 * CSP: the platform UI is a Next.js build that needs inline styles/scripts
 * (styled-jsx, __NEXT_DATA__); connect-src covers the loopback backend, the
 * cloud API, the local Vite dev server (dev) and the custom protocols the
 * main process proxies. Dev mode additionally allows eval (HMR/dev builds)
 * and ws: for the Vite HMR socket. Every non-app origin passes through
 * untouched so Browser Mode webviews keep working.
 */
export function installSessionSecurityHandlers(session: Session): void {
  const csp = buildContentSecurityPolicy();

  session.webRequest.onHeadersReceived((details, callback) => {
    if (!isTrustedAppUrl(details.url)) {
      callback({});
      return;
    }
    callback({
      responseHeaders: {
        ...details.responseHeaders,
        'Content-Security-Policy': [csp],
      },
    });
  });

  // Default-deny permission requests. First-party origins may use the
  // microphone (voice/dictation) and capture the screen (this Mac's live
  // screen in the Computer view, composer screen share).
  session.setPermissionRequestHandler((_webContents, permission, callback, details) => {
    const requestingUrl = details.requestingUrl ?? '';
    const granted = (permission === 'media' || permission === 'display-capture') && isTrustedAppUrl(requestingUrl);
    if (!granted) {
      log.warn(`[Security] Denied permission "${permission}" for ${requestingUrl || '(unknown origin)'}`);
    }
    callback(granted);
  });

  session.setPermissionCheckHandler((_webContents, permission, requestingOrigin) => {
    return permission === 'media' && isTrustedAppUrl(requestingOrigin);
  });

  session.setDisplayMediaRequestHandler(
    displayMediaHandler({
      getSources: () => desktopCapturer.getSources({ types: ['screen'] }),
      primaryDisplayId: () => String(screen.getPrimaryDisplay().id),
    }),
  );
}

/**
 * getDisplayMedia() for app-owned pages: the primary screen, no picker
 * (macOS still requires the Screen Recording permission). Web pages in
 * Browser Mode share this session and get nothing.
 */
export function displayMediaHandler(deps: {
  getSources: () => Promise<DesktopCapturerSource[]>;
  primaryDisplayId: () => string;
}): (request: DisplayMediaRequestHandlerHandlerRequest, callback: (streams: Streams) => void) => void {
  return (request, callback) => {
    const origin = request.securityOrigin || request.frame?.url || '';
    if (!isTrustedAppUrl(origin)) {
      log.warn(`[Security] Denied screen capture for ${origin || '(unknown origin)'}`);
      callback({});
      return;
    }
    deps
      .getSources()
      .then((sources) => {
        const primary = deps.primaryDisplayId();
        const source = sources.find((s) => s.display_id === primary) ?? sources[0];
        callback(source ? { video: source } : {});
      })
      .catch((err) => {
        log.warn('[Security] Screen capture unavailable:', err);
        callback({});
      });
  };
}

function buildContentSecurityPolicy(): string {
  const dev = config?.isDev ?? false;
  const directives: Record<string, string[]> = {
    'default-src': ["'self'"],
    'script-src': ["'self'", "'unsafe-inline'", ...(dev ? ["'unsafe-eval'"] : [])],
    'style-src': ["'self'", "'unsafe-inline'"],
    'img-src': ["'self'", 'data:', 'blob:', 'https:'],
    'font-src': ["'self'", 'data:'],
    'connect-src': [
      "'self'",
      'https:', 'wss:',
      'http://127.0.0.1:*', 'http://localhost:*',
      'ws://127.0.0.1:*', 'ws://localhost:*',
      'allternit-api:', 'allternit-gizzi:',
    ],
    'media-src': ["'self'", 'blob:'],
    'worker-src': ["'self'", 'blob:'],
    'frame-src': ["'self'", 'https:', 'http://127.0.0.1:*', 'http://localhost:*'],
    'object-src': ["'none'"],
    'base-uri': ["'self'"],
  };
  return Object.entries(directives)
    .map(([name, values]) => `${name} ${[...new Set(values)].join(' ')}`)
    .join('; ');
}
