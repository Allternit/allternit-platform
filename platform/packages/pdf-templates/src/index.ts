import { renderHtml } from './html';
import { renderPdf } from './pdf';
import { buildDocument, validateRequest } from './templates';
import type { PdfRenderRequest } from './types';

export * from './types';
export { PDF_THEME_TOKENS, resolveTheme } from './themes';
export { PDF_TEMPLATE_INFO, PDF_PAGE_POINTS, buildDocument, validateRequest, money } from './templates';
export { renderHtml } from './html';
export { renderPdf, winAnsi } from './pdf';

/** Request to print-ready HTML (preview, and Desktop's printToPDF input). */
export function renderRequestHtml(req: PdfRenderRequest): string {
  return renderHtml(buildDocument(req));
}

/** Request to PDF bytes. Throws Error with a readable message on a bad request. */
export async function renderRequestPdf(req: PdfRenderRequest): Promise<{ bytes: Uint8Array; title: string }> {
  const bad = validateRequest(req);
  if (bad) throw new Error(bad);
  const doc = buildDocument(req);
  return { bytes: await renderPdf(doc), title: doc.title };
}
