import { describe, expect, test } from 'bun:test';
import { renderTemplatePdfWithSlots } from './pdf';
import { PDF_PAGE_POINTS, PDF_TEMPLATE_INFO } from './templates';
import type { PdfRenderRequest, SignSlot } from './types';

const overlap = (a: SignSlot, b: SignSlot): boolean =>
  a.page === b.page && a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

const cases: [string, PdfRenderRequest, number][] = [
  [
    'invoice with signatures',
    { template: 'invoice', data: { ...PDF_TEMPLATE_INFO.invoice.sample, signatures: true } },
    2,
  ],
  ['contract', { template: 'contract', pageSize: 'a4', data: PDF_TEMPLATE_INFO.contract.sample }, 2],
];

describe('renderTemplatePdfWithSlots', () => {
  for (const [name, req, count] of cases) {
    test(`${name}: slots lie inside the page and do not overlap`, async () => {
      const { pdf, slots } = await renderTemplatePdfWithSlots(req);
      expect(pdf.byteLength).toBeGreaterThan(1000);
      expect(slots.length).toBe(count);
      const [W, H] = PDF_PAGE_POINTS[req.pageSize ?? 'letter'];
      for (const s of slots) {
        expect(s.page).toBeGreaterThanOrEqual(0);
        expect(s.h).toBe(56);
        expect(s.w).toBeGreaterThan(0);
        expect(s.x).toBeGreaterThanOrEqual(0);
        expect(s.y).toBeGreaterThanOrEqual(0);
        expect(s.x + s.w).toBeLessThanOrEqual(W);
        expect(s.y + s.h).toBeLessThanOrEqual(H);
      }
      expect(new Set(slots.map((s) => s.id)).size).toBe(slots.length);
      for (let i = 0; i < slots.length; i++) for (let j = i + 1; j < slots.length; j++) expect(overlap(slots[i], slots[j])).toBe(false);
    });
  }
});
