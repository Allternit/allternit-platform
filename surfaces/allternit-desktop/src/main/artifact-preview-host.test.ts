import { describe, expect, it } from 'vitest';
import { BUILD_PREVIEW_CSP, hostPreview, servePreview } from './artifact-preview-host.js';

describe('artifact preview host', () => {
  it('serves a hosted page with its own CSP that allows CDNs but nothing local', async () => {
    const r = hostPreview('<h1>Harbor</h1>');
    if (!r.ok) throw new Error(r.error);
    expect(r.url).toMatch(/^allternit-preview:\/\/site\/[0-9a-f-]+\/$/);
    const res = servePreview(r.url);
    expect(res.status).toBe(200);
    expect(await res.text()).toBe('<h1>Harbor</h1>');
    const csp = res.headers.get('content-security-policy') ?? '';
    expect(csp).toBe(BUILD_PREVIEW_CSP);
    expect(csp).toContain('script-src');
    expect(csp).toMatch(/script-src [^;]*https:/);
    expect(csp).not.toMatch(/127\.0\.0\.1|localhost|allternit-api/);
  });

  it('404s unknown pages and refuses empty ones', () => {
    expect(servePreview('allternit-preview://site/nope/').status).toBe(404);
    expect(servePreview('allternit-preview://other/x/').status).toBe(404);
    expect(hostPreview('  ').ok).toBe(false);
  });
});
