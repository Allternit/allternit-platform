import { BrowserWindow, session } from 'electron';
import type { Cookie, Session } from 'electron';
import log from 'electron-log';
import { onWebRequest } from './web-request-hub.js';

const OAUTH_POPUP_WIDTH = 480;
const OAUTH_POPUP_HEIGHT = 640;
const OAUTH_TIMEOUT_MS = 5 * 60 * 1000;

const OAUTH_PROVIDER_HOSTS = new Set([
  'accounts.google.com',
  'github.com',
  'api.github.com',
]);

export interface OAuthPopupOptions {
  /**
   * Partition session that owns the Clerk client (`__client`) the OAuth
   * attempt was created under. Its allternit.com cookies are copied into the
   * popup session so the callback exchange binds to the same Clerk client.
   */
  clerkSession: Session;
  /** Origin of the auth renderer, i.e. Clerk's redirect_url origin. */
  redirectOrigin: string;
  /** Path prefix marking a successful exchange (the auth renderer's mount path). */
  redirectPathPrefix: string;
}

export interface OAuthPopupResult {
  /** Clerk cookies in the popup session after a successful exchange. */
  cookies: Cookie[];
}

function isOAuthProviderUrl(url: string): boolean {
  try {
    const parsed = new URL(url);
    return OAUTH_PROVIDER_HOSTS.has(parsed.hostname) || parsed.hostname.endsWith('.google.com');
  } catch {
    return false;
  }
}

function sameSiteFor(cookie: Cookie): 'unspecified' | 'no_restriction' | 'lax' | 'strict' {
  switch (cookie.sameSite) {
    case 'no_restriction':
    case 'lax':
    case 'strict':
      return cookie.sameSite;
    default:
      return 'unspecified';
  }
}

function cookieUrl(cookie: Cookie): string {
  const host = (cookie.domain ?? '').replace(/^\./, '') || 'allternit.com';
  return `https://${host}${cookie.path ?? '/'}`;
}

export async function setCookieOnSession(target: Session, cookie: Cookie): Promise<void> {
  // __Host- prefixed cookies must be set host-only (no domain attribute) with
  // path "/" and secure; passing an explicit domain makes Chromium reject them.
  const hostPrefixed = cookie.name.startsWith('__Host-');
  const prefixed = hostPrefixed || cookie.name.startsWith('__Secure-');
  await target.cookies.set({
    url: cookieUrl(cookie),
    name: cookie.name,
    value: cookie.value,
    ...(hostPrefixed ? {} : { domain: cookie.domain }),
    path: hostPrefixed ? '/' : (cookie.path ?? '/'),
    secure: prefixed ? true : cookie.secure,
    httpOnly: cookie.httpOnly,
    sameSite: sameSiteFor(cookie),
    ...(cookie.expirationDate ? { expirationDate: cookie.expirationDate } : {}),
  });
}

async function copyCookies(from: Session, to: Session, url: string): Promise<void> {
  const seen = new Set<string>();
  for (const cookie of await from.cookies.get({ url })) {
    const key = `${cookie.name}|${cookie.domain}`;
    if (seen.has(key)) continue;
    seen.add(key);
    try {
      await setCookieOnSession(to, cookie);
    } catch (err) {
      // Provider jars contain cookies with strict prefix rules or expired
      // markers; a single un-copyable cookie must not sink the flow.
      log.warn(`[OAuthPopup] skipping cookie ${cookie.name}:`, err);
    }
  }
}

/**
 * Opens a modal OAuth popup in the DEFAULT session and lets the provider's
 * redirect chain run to completion inside it — including Clerk's
 * /v1/oauth_callback exchange, which Clerk only accepts as a real browser
 * navigation (fetch-style replays are rejected with authorization_invalid).
 *
 * The default session is load-bearing here: it keeps the user's existing
 * provider session (no re-login), its cookie jar flows on real navigations,
 * and webRequest.onBeforeRequest reliably observes its navigation requests —
 * webContents will-redirect does not fire for the cross-origin 302 from
 * Clerk's exchange endpoint, and custom-partition webRequest does not observe
 * these requests at all on this Electron version.
 *
 * Resolves with the session's Clerk cookies once the exchange redirects to
 * the auth renderer (created_session_id present). Rejects on an err_code
 * redirect, timeout, or if the user closes the popup.
 */
export function openClerkOAuthPopup(
  startUrl: string,
  options: OAuthPopupOptions,
): Promise<OAuthPopupResult> {
  return new Promise((resolve, reject) => {
    if (!isOAuthProviderUrl(startUrl)) {
      reject(new Error('Refusing to open non-OAuth URL in auth popup'));
      return;
    }

    const popupSession = session.defaultSession;
    const popup = new BrowserWindow({
      width: OAUTH_POPUP_WIDTH,
      height: OAUTH_POPUP_HEIGHT,
      show: true,
      modal: true,
      parent: BrowserWindow.getFocusedWindow() || undefined,
      title: 'Allternit — Sign in',
      webPreferences: {
        nodeIntegration: false,
        contextIsolation: true,
      },
    });

    let settled = false;
    let removeExchangeWatch: (() => void) | null = null;

    const cleanup = async () => {
      removeExchangeWatch?.();
      removeExchangeWatch = null;
      if (!popup.isDestroyed()) popup.destroy();
    };

    const finish = async (url: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      const cookies = [
        ...(await popupSession.cookies.get({ url: options.redirectOrigin })),
        ...(await popupSession.cookies.get({ url: 'https://allternit.com/' })),
      ];
      await cleanup();
      resolve({ cookies });
      log.info('[OAuthPopup] OAuth exchange completed, redirect target:', url.slice(0, 110));
    };

    const fail = async (error: Error, detailUrl?: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      await cleanup();
      if (detailUrl) {
        log.warn('[OAuthPopup] OAuth failed:', error.message, '— at:', detailUrl.slice(0, 140));
      }
      reject(error);
    };

    const timeout = setTimeout(() => {
      void fail(new Error('OAuth sign-in timed out. Please try again.'));
    }, OAUTH_TIMEOUT_MS);

    // A redirect to the auth renderer's redirect_url is the success signal:
    // failures are redirected to /v1/oauth_callback?err_code instead. Note
    // FAPI does NOT append created_session_id for clerk-js-created attempts
    // (only for API-created ones), so the query must not be part of the check.
    const isSuccessUrl = (url: string): boolean => {
      try {
        const parsed = new URL(url);
        return (
          parsed.origin === options.redirectOrigin &&
          parsed.pathname.startsWith(options.redirectPathPrefix)
        );
      } catch {
        return false;
      }
    };

    // Detect finish states pre-dispatch: the success URL must never load in
    // the popup (only the protocol-intercepted auth partition can serve it —
    // the hosted portal answers 404 for this path).
    removeExchangeWatch = onWebRequest(
      popupSession,
      'onBeforeRequest',
      (details, callback) => {
        if (details.url.includes('err_code=')) {
          callback({ cancel: true });
          void fail(new Error('Clerk rejected the OAuth exchange.'), details.url);
          return;
        }
        if (isSuccessUrl(details.url)) {
          callback({ cancel: true });
          void finish(details.url);
          return;
        }
        // Clerk's exchange endpoint itself (no err_code): let it load so it
        // can 302 onward to the success URL above.
        callback({ cancel: false });
      },
      {
        urls: [
          `${options.redirectOrigin}${options.redirectPathPrefix}*`,
          '*://allternit.com/v1/oauth_callback*',
          '*://clerk.allternit.com/v1/oauth_callback*',
          '*://accounts.allternit.com/v1/oauth_callback*',
        ],
      },
    );

    popup.webContents.on('did-navigate', (_event, url) => {
      if (isSuccessUrl(url)) void finish(url);
    });

    popup.on('closed', () => {
      void fail(new Error('Sign-in window was closed before completing.'));
    });

    void (async () => {
      try {
        // Bind the popup's exchange to the Clerk client that owns the attempt.
        await copyCookies(options.clerkSession, popupSession, options.redirectOrigin);
        await copyCookies(options.clerkSession, popupSession, 'https://allternit.com/');
        await popup.loadURL(startUrl);
      } catch (err) {
        if (settled) return; // popup destroyed mid-flight by finish/fail
        log.error('[OAuthPopup] Failed to start OAuth flow:', err);
        await fail(err instanceof Error ? err : new Error(String(err)));
      }
    })();
  });
}
