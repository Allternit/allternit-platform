/**
 * Model deep links from outside the app (a Freeweights model page, a terminal):
 *   allternit://open?model=<id>                    new chat with that Cloud model
 *   allternit://open?view=model-lab&model=<id>     Freeweights Studio, that model
 * They map to the same in-app URL params as the web links on ai.allternit.com
 * (see allternit-ai src/lib/model-deep-link.ts). Only the id and the one known
 * view survive; a raw URL or any other parameter is never passed through.
 */
const MODEL_ID = /^[A-Za-z0-9][A-Za-z0-9._-]*(\/[A-Za-z0-9][A-Za-z0-9._-]*)?$/;
const MAX_ID_LENGTH = 200;

export function isModelLink(url: string): boolean {
  return /^allternit(-dev)?:\/\/open\?/.test(url) && /[?&]model=/.test(url);
}

/** The in-app query ("model=x" or "view=model-lab&model=x"), or null if rejected. */
export function modelLinkQuery(url: string): URLSearchParams | null {
  try {
    const u = new URL(url);
    if (u.protocol !== 'allternit:' && u.protocol !== 'allternit-dev:') return null;
    if (u.hostname !== 'open') return null;
    const id = u.searchParams.get('model')?.trim() ?? '';
    if (!id || id.length > MAX_ID_LENGTH || !MODEL_ID.test(id) || id.includes('..')) return null;
    const view = u.searchParams.get('view');
    const out = new URLSearchParams();
    if (view !== null) {
      if (view !== 'model-lab') return null;
      out.set('view', 'model-lab');
    }
    out.set('model', id);
    return out;
  } catch {
    return null;
  }
}

/** The app URL to load for a link: the platform URL plus the mapped params. */
export function modelLinkTarget(platformUrl: string, url: string): string | null {
  const query = modelLinkQuery(url);
  if (!query) return null;
  const target = new URL(platformUrl);
  target.searchParams.delete('view');
  target.searchParams.delete('model');
  query.forEach((value, key) => target.searchParams.set(key, value));
  return target.toString();
}
