/**
 * allternit://app/<slug> opens an installed Allternit app (plan v3 lane 38).
 * Windows Start menu shortcuts and Linux .desktop launchers run Allternit
 * with this link (app-launchers.ts); unified-main handleProtocolCallback
 * sends it to openInstalledApp. Only the slug survives: query strings,
 * extra path parts and tokens are never passed on.
 */
import { isValidAppSlug } from './app-launchers.js';

export function isAppLink(url: string): boolean {
  return /^allternit(-dev)?:\/\/app(\/|$|\?)/.test(url);
}

export function appSlugFromLink(url: string): string | null {
  // Dot segments and escapes would be normalized away by URL; refuse them.
  if (url.includes('..') || url.includes('%')) return null;
  try {
    const u = new URL(url);
    if (u.protocol !== 'allternit:' && u.protocol !== 'allternit-dev:') return null;
    if (u.hostname !== 'app') return null;
    const parts = u.pathname.split('/').filter(Boolean);
    if (parts.length !== 1) return null;
    const slug = parts[0]!.toLowerCase();
    return isValidAppSlug(slug) ? slug : null;
  } catch {
    return null;
  }
}

/** The app shell route on the origin the Desktop UI uses: <origin>/app/<slug>. */
export function appShellUrl(platformUrl: string, slug: string): string {
  if (!isValidAppSlug(slug)) throw new Error(`bad app slug: ${slug}`);
  return new URL(`/app/${slug}`, platformUrl).toString();
}

/** The hosted app domain (APP_DOMAIN in the web client); apps live at https://<slug>.<this>. */
export const APP_DOMAIN = 'allternitusercontent.com';

/**
 * The renderer hands us the app link (web client appLink()); only https links
 * on the app domain or the Desktop UI's own host are kept. Anything else is
 * dropped and the caller falls back to <platform>/app/<slug>.
 */
export function trustedAppUrl(raw: unknown, platformUrl: string, slug: string): string | null {
  if (typeof raw !== 'string' || raw.length > 2048) return null;
  try {
    const u = new URL(raw);
    if (u.protocol !== 'https:' || u.username || u.password) return null;
    const host = u.hostname.toLowerCase();
    const platformHost = new URL(platformUrl).hostname.toLowerCase();
    const onAppDomain = host === `${slug}.${APP_DOMAIN}` || host === `${slug}.live.${APP_DOMAIN}`;
    if (!onAppDomain && host !== platformHost) return null;
    u.hash = '';
    return u.toString();
  } catch {
    return null;
  }
}

/** The URL stored with an installed app: the app link when trusted, else the old /app/<slug>. */
export function resolveAppUrl(raw: unknown, platformUrl: string, slug: string): string {
  return trustedAppUrl(raw, platformUrl, slug) ?? appShellUrl(platformUrl, slug);
}

/**
 * Asks cloud-api for a one-time handoff (POST /api/v1/apps/:slug/handoff) and
 * returns its open_url (url?handoff=token), or null when it fails (offline,
 * signed out, bad answer). The open_url must stay on the app's own origin.
 */
export async function requestAppHandoff(opts: {
  apiBase: string;
  slug: string;
  appUrl: string;
  token: string | null;
  fetchImpl?: typeof fetch;
  timeoutMs?: number;
}): Promise<string | null> {
  if (!opts.token || !isValidAppSlug(opts.slug)) return null;
  const doFetch = opts.fetchImpl ?? fetch;
  try {
    const res = await doFetch(new URL(`/api/v1/apps/${opts.slug}/handoff`, opts.apiBase).toString(), {
      method: 'POST',
      headers: { authorization: `Bearer ${opts.token}`, 'content-type': 'application/json' },
      body: '{}',
      signal: AbortSignal.timeout(opts.timeoutMs ?? 8000),
    });
    if (!res.ok) return null;
    const body = (await res.json()) as { open_url?: unknown };
    if (typeof body.open_url !== 'string') return null;
    const next = new URL(body.open_url);
    const base = new URL(opts.appUrl);
    if (next.origin !== base.origin) return null;
    return next.toString();
  } catch {
    return null;
  }
}
