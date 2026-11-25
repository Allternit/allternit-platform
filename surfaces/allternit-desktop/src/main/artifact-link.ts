/**
 * allternit://artifact/<id> opens an artifact (a plan, page or app output) in
 * the main window at /?artifact=<id>, the same param the web app reads.
 * unified-main handleProtocolCallback loads artifactLinkTarget(). Only the id
 * survives: query strings, extra path parts, escapes and dot segments are
 * dropped, and anything that is not a valid id is ignored.
 */
const ARTIFACT_ID = /^art_[A-Za-z0-9_-]{1,64}$/;

export function isArtifactLink(url: string): boolean {
  return /^allternit(-dev)?:\/\/artifact(\/|$|\?)/.test(url);
}

export function artifactIdFromLink(url: string): string | null {
  // Dot segments and escapes would be normalized away by URL; refuse them.
  if (url.includes('..') || url.includes('%')) return null;
  try {
    const u = new URL(url);
    if (u.protocol !== 'allternit:' && u.protocol !== 'allternit-dev:') return null;
    if (u.hostname !== 'artifact') return null;
    const parts = u.pathname.split('/').filter(Boolean);
    if (parts.length !== 1) return null;
    const id = parts[0]!;
    return ARTIFACT_ID.test(id) ? id : null;
  } catch {
    return null;
  }
}

/** The app URL to load for a link: <origin>/?artifact=<id>, or null if rejected. */
export function artifactLinkTarget(platformUrl: string, url: string): string | null {
  const id = artifactIdFromLink(url);
  if (!id) return null;
  try {
    const target = new URL('/', platformUrl);
    target.searchParams.set('artifact', id);
    return target.toString();
  } catch {
    return null;
  }
}
