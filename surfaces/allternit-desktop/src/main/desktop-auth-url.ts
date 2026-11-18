/**
 * True when `url` is still the desktop auth renderer. Clerk often redirects
 * to the same path without a trailing slash (`/__desktop_auth__` vs
 * `/__desktop_auth__/`) or with a hash (`#/factor-one`). A naive
 * `url.startsWith(base + '/')` treats those as foreign navigations, we
 * reload the window, and Clerk aborts the in-flight sign-in with
 * "You are signed out".
 */
export function isDesktopAuthNavigation(url: string, authBaseUrl: string): boolean {
  try {
    const dest = new URL(url);
    const base = new URL(authBaseUrl);
    if (dest.protocol !== base.protocol || dest.host !== base.host) return false;
    const basePath = base.pathname.replace(/\/+$/, '') || '/';
    const destPath = dest.pathname.replace(/\/+$/, '') || '/';
    return destPath === basePath || destPath.startsWith(`${basePath}/`);
  } catch {
    return false;
  }
}

/**
 * A Clerk token reached main with nothing waiting for it (the pairing it was
 * for expired, or the wait was dropped). If this desktop isn't paired and no
 * pairing is running, the person is signed in: pair with the token.
 */
export function shouldPairWithLateClerkToken(state: {
  waiting: boolean;
  token: string;
  paired: boolean;
  pairingInFlight: boolean;
}): boolean {
  return !state.waiting && state.token.length > 0 && !state.paired && !state.pairingInFlight;
}

/**
 * What the sign-in window says when the pairing-code lookup fails. Only a
 * missing or expired code is the person's to retry; a server error or a
 * rejected sign-in must not be reported as "invalid or expired" (a cloud-api
 * 500 on every lookup hid behind that wording on 2026-11-18).
 */
export function pairingLookupFailureMessage(status: number, errorCode?: string): string {
  if (status === 404 || status === 410 || errorCode === 'TOKEN_EXPIRED') {
    return 'This pairing request is invalid or expired. Please try again.';
  }
  if (status === 401 || status === 403) {
    return 'Allternit could not confirm your sign-in. Please sign in again.';
  }
  return `Allternit could not check this pairing right now (error ${status}). Please try again in a minute.`;
}
