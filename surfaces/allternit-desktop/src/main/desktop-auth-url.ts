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
