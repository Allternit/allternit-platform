// Entry for the iOS JavaScriptCore bundle (no DOM, no pdf-lib): request JSON in, print-ready HTML out.
import { renderHtml } from './html';
import { buildDocument, validateRequest } from './templates';
import type { PdfRenderRequest } from './types';

(globalThis as unknown as { allternitPdfHtml: (json: string) => string }).allternitPdfHtml = (json: string): string => {
  const req = JSON.parse(json) as PdfRenderRequest;
  const bad = validateRequest(req);
  if (bad) throw new Error(bad);
  return renderHtml(buildDocument(req));
};
