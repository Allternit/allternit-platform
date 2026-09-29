/**
 * Serves sites an agent built (Build outputs) at allternit-preview://site/<id>/
 * so they render as the user expects: with their CDN scripts, stylesheets,
 * fonts and images. An inline srcdoc frame can't — it inherits the app's CSP,
 * which only allows the app's own scripts (a Tailwind-CDN site showed
 * unstyled).
 *
 * The page gets its own CSP (below) and is framed with sandbox="allow-scripts"
 * — an opaque origin, so it can't reach the app's storage, cookies or APIs.
 */
import { randomUUID } from 'node:crypto';

export const PREVIEW_SCHEME = 'allternit-preview';

/** What a Build page may load: web libraries and assets, nothing local. */
export const BUILD_PREVIEW_CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline' 'unsafe-eval' https:",
  "style-src 'unsafe-inline' https:",
  'img-src data: blob: https:',
  'font-src data: https:',
  'media-src data: blob: https:',
  'connect-src https:',
  "frame-src 'none'",
  "object-src 'none'",
  "form-action 'none'",
  "base-uri 'none'",
].join('; ');

const MAX_PAGES = 30;
const MAX_BYTES = 5 * 1024 * 1024;
const pages = new Map<string, string>();

/** Keep a page and return the URL that serves it (newest MAX_PAGES are kept). */
export function hostPreview(html: unknown): { ok: true; url: string } | { ok: false; error: string } {
  if (typeof html !== 'string' || !html.trim()) return { ok: false, error: 'There is no HTML to preview.' };
  if (Buffer.byteLength(html) > MAX_BYTES) return { ok: false, error: 'The page is too large to preview (over 5 MB).' };
  const id = randomUUID();
  pages.set(id, html);
  while (pages.size > MAX_PAGES) pages.delete(pages.keys().next().value as string);
  return { ok: true, url: `${PREVIEW_SCHEME}://site/${id}/` };
}

/** protocol.handle for allternit-preview: the page, or 404. */
export function servePreview(requestUrl: string): Response {
  let id = '';
  try {
    const url = new URL(requestUrl);
    if (url.host === 'site') id = url.pathname.split('/').filter(Boolean)[0] ?? '';
  } catch {
    // fall through to 404
  }
  const html = id ? pages.get(id) : undefined;
  if (html === undefined) return new Response('Not found', { status: 404, headers: { 'content-type': 'text/plain' } });
  return new Response(html, {
    status: 200,
    headers: {
      'content-type': 'text/html; charset=utf-8',
      'content-security-policy': BUILD_PREVIEW_CSP,
      'cache-control': 'no-store',
      'x-content-type-options': 'nosniff',
    },
  });
}
