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
