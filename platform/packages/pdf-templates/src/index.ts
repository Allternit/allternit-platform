import { renderHtml } from './html';
import { NOTO_SANS_BOLD_B64, NOTO_SANS_REGULAR_B64, b64ToBytes } from './fonts';
import { renderPdf } from './pdf';
import { buildDocument, validateRequest } from './templates';
import type { PdfRenderRequest } from './types';

export * from './types';
export { PDF_THEME_TOKENS, resolveTheme } from './themes';
export { PDF_TEMPLATE_INFO, PDF_PAGE_POINTS, buildDocument, validateRequest, money } from './templates';
export { renderHtml } from './html';
export { renderPdf, sanitizeText, winAnsi, type PdfFontBytes } from './pdf';
export { b64ToBytes } from './fonts';

/** Request to print-ready HTML (preview, and Desktop's printToPDF input). */
export function renderRequestHtml(req: PdfRenderRequest): string {
  return renderHtml(buildDocument(req));
}

/** Request to PDF bytes. Throws Error with a readable message on a bad request. */
export async function renderRequestPdf(req: PdfRenderRequest): Promise<{ bytes: Uint8Array; title: string }> {
  const bad = validateRequest(req);
  if (bad) throw new Error(bad);
  const doc = buildDocument(req);
  const fonts = { regular: b64ToBytes(NOTO_SANS_REGULAR_B64), bold: b64ToBytes(NOTO_SANS_BOLD_B64) };
  return { bytes: await renderPdf(doc, fonts), title: doc.title };
}
