/**
 * Location-header rewriting for the Clerk FAPI proxy.
 *
 * Kept separate from index.js so it can be unit tested without the Workers
 * runtime (node --test).
 */

export const FAPI_ORIGIN = 'https://clerk.allternit.com';
export const FAPI_HOST = 'clerk.allternit.com';
export const PROXY_PATH_PREFIX = '/__clerk';

// The marketing site. Clerk's instance-level sign-in / sign-up URLs point
// here, but the pages themselves live on each app origin (ai / platform / m).
const APEX_HOSTS = new Set(['allternit.com', 'www.allternit.com']);
const APP_AUTH_PATH = /^\/sign-(in|up)(\/|$)/;

/**
 * @param {string} location   upstream Location header from Clerk FAPI
 * @param {URL} requestUrl    the proxied request's URL (browser-facing origin)
 * @returns {string}          the Location to send to the browser
 */
export function rewriteLocation(location, requestUrl) {
  let loc;
  try {
    // FAPI sends relative redirects too (e.g. the oauth_callback error hop:
    // "/v1/oauth_callback?err_code=..."). Resolve them against FAPI so they
    // get the proxy prefix below; left relative the browser lands on
    // <origin>/v1/..., outside the proxy, which is a 404 page.
    loc = new URL(location, FAPI_ORIGIN);
  } catch {
    return location;
  }
  const tail = `${loc.pathname}${loc.search}${loc.hash}`;

  // Clerk 307s clerk-js to the configured proxy host (allternit.com/__clerk).
  // Keep the browser on this origin so CSP 'self' and first-party cookies
  // work on m (Allternit Mobile) / ai / platform.
  if (loc.pathname.startsWith(PROXY_PATH_PREFIX)) {
    return `${requestUrl.origin}${tail}`;
  }

  if (loc.hostname === FAPI_HOST) {
    return `${requestUrl.origin}${PROXY_PATH_PREFIX}${tail}`;
  }

  // Redirects to the instance sign-in / sign-up URL (allternit.com/sign-in)
  // are app pages, not FAPI paths: prefixing them with /__clerk yields FAPI's
  // "404 page not found". Send the user to the same page on the app they
  // came from. On the apex itself, leave it alone; the www site redirects it.
  if (APEX_HOSTS.has(loc.hostname) && !APEX_HOSTS.has(requestUrl.hostname)) {
    // The dashboard values carry trailing spaces; drop them.
    const pathname = loc.pathname.replace(/(%20|\s)+$/i, '');
    if (APP_AUTH_PATH.test(pathname)) {
      return `${requestUrl.origin}${pathname}${loc.search}${loc.hash}`;
    }
  }

  // Anything else is an absolute app URL (redirect_url, after-sign-in, ...).
  return location;
}
