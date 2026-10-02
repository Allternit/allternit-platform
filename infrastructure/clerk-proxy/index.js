/**
 * Clerk Frontend API proxy for allternit.com.
 *
 * Proxies all Clerk FAPI traffic through the app domain so session cookies
 * stay first-party and sign-in works on browsers that block third-party
 * cookies across subdomains.
 *
 * Route: *.allternit.com/__clerk/* -> https://clerk.allternit.com/*
 */

import { FAPI_ORIGIN, PROXY_PATH_PREFIX, rewriteLocation } from './location.js';

const SHARED_COOKIE_DOMAIN = '.allternit.com';

// This must match a proxy URL configured in the Clerk dashboard for the
// allternit.com instance. Clerk validates the header and rejects requests
// (host_invalid) when it does not match. Subdomains still proxy through their
// own origin for first-party cookies; the header just tells Clerk which proxy
// endpoint is in use.
const CLERK_PROXY_URL = 'https://allternit.com/__clerk';

function buildProxyUrl(origin) {
  return CLERK_PROXY_URL;
}

/**
 * Rewrite Clerk's FAPI Set-Cookie headers so the browser stores them under the
 * app's own subdomain instead of clerk.allternit.com. Without this the session
 * cookie is invisible to platform.allternit.com / ai.allternit.com and the
 * auth gate bounces back to /sign-in.
 */
function rewriteSetCookie(headerValue) {
  // Lower-case attribute names to handle mixed casing from Clerk.
  let rewritten = headerValue
    .replace(/Domain=\.?clerk\.allternit\.com/gi, `Domain=${SHARED_COOKIE_DOMAIN}`)
    .replace(/domain=\.?clerk\.allternit\.com/gi, `Domain=${SHARED_COOKIE_DOMAIN}`);

  // Clerk's __client cookie defaults to SameSite=Lax. When the sign-in flow
  // does client-side redirects between /sign-in and /sign-in/factor-one the
  // cookie must travel on every request; Lax can be dropped on the AJAX call
  // that follows a same-origin client redirect. Force None with Secure so the
  // client state cookie (which carries the in-flight sign-in attempt) is always
  // sent through the proxy.
  if (rewritten.startsWith('__client=')) {
    rewritten = rewritten.replace(/SameSite=Lax/gi, 'SameSite=None');
  }

  return rewritten;
}

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);

    // Only proxy paths under /__clerk
    if (!url.pathname.startsWith(PROXY_PATH_PREFIX)) {
      return new Response('Not found', { status: 404 });
    }

    const targetPath = url.pathname.slice(PROXY_PATH_PREFIX.length) || '/';
    const targetUrl = `${FAPI_ORIGIN}${targetPath}${url.search}`;

    const proxyHeaders = new Headers(request.headers);
    proxyHeaders.delete('host');
    proxyHeaders.set('Clerk-Proxy-Url', buildProxyUrl(url.origin));
    proxyHeaders.set('Clerk-Secret-Key', env.CLERK_SECRET_KEY);

    const clientIp = request.headers.get('CF-Connecting-IP') || '';
    if (clientIp) {
      proxyHeaders.set('X-Forwarded-For', clientIp);
    }

    const proxyReq = new Request(targetUrl, {
      method: request.method,
      headers: proxyHeaders,
      body: request.body,
      redirect: 'manual',
    });

    const response = await fetch(proxyReq);

    const requestOrigin = request.headers.get('origin') || url.origin;
    const corsHeaders = new Headers();

    // Copy every upstream header except Set-Cookie; we'll rewrite cookies
    // explicitly to avoid losing them when Headers merges multi-value headers.
    for (const [key, value] of response.headers.entries()) {
      if (key.toLowerCase() !== 'set-cookie') {
        corsHeaders.append(key, value);
      }
    }

    const setCookies = response.headers.getSetCookie ? response.headers.getSetCookie() : [];
    for (const cookie of setCookies) {
      corsHeaders.append('Set-Cookie', rewriteSetCookie(cookie));
    }

    corsHeaders.set('Access-Control-Allow-Origin', requestOrigin);
    corsHeaders.set('Access-Control-Allow-Credentials', 'true');

    const location = corsHeaders.get('Location');
    if (location) {
      corsHeaders.set('Location', rewriteLocation(location, url));
    }

    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers: corsHeaders,
    });
  },
};
