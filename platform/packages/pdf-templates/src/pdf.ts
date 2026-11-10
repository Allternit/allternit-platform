import fontkit from '@pdf-lib/fontkit';
import { PDFDocument, rgb, type PDFFont, type PDFPage } from 'pdf-lib';
import { PDF_PAGE_POINTS } from './templates';
import { hexToRgb01 } from './themes';
import type { PdfBlock, PdfDocModel } from './types';

/**
 * Native PDF writer: draws the same block model the HTML renderer draws, with
 * an embedded, subset Noto Sans (regular and bold) covering Latin, Latin
 * Extended, Vietnamese, Greek and Cyrillic. No browser is involved, so it runs
 * anywhere Node, Bun or a browser runs. Limit: scripts the font lacks (CJK,
 * Arabic, Hebrew, Thai, Indic) print as "?" rather than as empty boxes.
 */

const MARGIN = 48;

const REPLACE: Record<string, string> = { '\u00A0': ' ' };
/** Glyphs the embedded font has; set at the start of each render. */
let glyphs: Set<number> | null = null;

/** Keep what the font can draw; the rest becomes "?". Newlines pass through. */
export function sanitizeText(s: string): string {
  let out = '';
  for (const ch of s) {
    const c = ch.codePointAt(0)!;
    if (REPLACE[ch]) out += REPLACE[ch];
    else if (c === 10) out += '\n';
    else if (c < 0x20) out += ' ';
    else out += !glyphs || glyphs.has(c) ? ch : '?';
  }
  return out;
}
export const winAnsi = sanitizeText;

export interface PdfFontBytes {
  regular: Uint8Array;
  bold: Uint8Array;
}

interface Ctx {
  pdf: PDFDocument;
  page: PDFPage;
  y: number; // distance from the top
  W: number;
  H: number;
  reg: PDFFont;
  bold: PDFFont;
  mono: PDFFont;
  doc: PdfDocModel;
}

const col = (hex: string) => {
  const [r, g, b] = hexToRgb01(hex);
  return rgb(r, g, b);
};

function wrap(font: PDFFont, text: string, size: number, maxW: number): string[] {
  const out: string[] = [];
  for (const para of winAnsi(text).split('\n')) {
    let line = '';
    for (const word of para.split(/\s+/).filter(Boolean)) {
      let w = word;
      // Break a word that is wider than the column.
      while (font.widthOfTextAtSize(w, size) > maxW && w.length > 1) {
        let n = w.length - 1;
        while (n > 1 && font.widthOfTextAtSize(w.slice(0, n), size) > maxW) n--;
        if (line) { out.push(line); line = ''; }
        out.push(w.slice(0, n));
        w = w.slice(n);
      }
      const next = line ? `${line} ${w}` : w;
      if (font.widthOfTextAtSize(next, size) <= maxW) line = next;
      else { out.push(line); line = w; }
    }
    out.push(line);
  }
  return out;
}

function newPage(c: Ctx): void {
  c.page = c.pdf.addPage([c.W, c.H]);
  c.y = MARGIN;
}
function ensure(c: Ctx, h: number): void {
  if (c.y + h > c.H - MARGIN) newPage(c);
}
/** Draw one line of text whose top is at `top` (distance from page top). */
function text(c: Ctx, s: string, x: number, top: number, size: number, font: PDFFont, color: string, align: 'l' | 'r' = 'l', rightEdge = 0): void {
  const t = winAnsi(s);
  const w = font.widthOfTextAtSize(t, size);
  c.page.drawText(t, { x: align === 'r' ? rightEdge - w : x, y: c.H - top - size * 0.86, size, font, color: col(color) });
}
function hline(c: Ctx, top: number, color: string, thickness: number, x1 = MARGIN, x2 = c.W - MARGIN): void {
  c.page.drawLine({ start: { x: x1, y: c.H - top }, end: { x: x2, y: c.H - top }, thickness, color: col(color) });
}
function rect(c: Ctx, x: number, top: number, w: number, h: number, fill: string): void {
  c.page.drawRectangle({ x, y: c.H - top - h, width: w, height: h, color: col(fill) });
}

function drawBlock(c: Ctx, b: PdfBlock): void {
  const t = c.doc.theme;
  const inner = c.W - MARGIN * 2;
  switch (b.t) {
    case 'masthead': {
      const modern = b.style === 'modern';
      const ink = modern ? t.bandInk : t.ink;
      const muted = modern ? t.bandInk : t.muted;
      const titleSize = b.style === 'minimal' ? 15 : 24;
      const metaH = b.meta.length * 14;
      const headH = Math.max((b.brand ? 16 : 0) + titleSize * 1.25 + (b.subtitle ? 18 : 0), metaH);
      if (modern) {
        rect(c, 0, 0, c.W, MARGIN + headH + 20, t.band);
        c.y = 36;
      }
      let y = c.y;
      if (b.brand) { text(c, winAnsi(b.brand).toUpperCase(), MARGIN, y, 9, c.reg, muted); y += 16; }
      text(c, b.style === 'minimal' ? winAnsi(b.title).toUpperCase() : b.title, MARGIN, y, titleSize, b.style === 'minimal' ? c.bold : c.bold, ink);
      if (b.subtitle) text(c, b.subtitle, MARGIN, y + titleSize * 1.25, 11, c.reg, muted);
      let my = c.y;
      for (const [k, v] of b.meta) {
        text(c, v, 0, my, 9.5, c.reg, ink, 'r', c.W - MARGIN);
        text(c, k, 0, my, 9.5, c.reg, muted, 'r', c.W - MARGIN - c.reg.widthOfTextAtSize(winAnsi(v), 9.5) - 8);
        my += 14;
      }
      if (modern) {
        c.y = MARGIN + headH + 20 + 18;
      } else {
        c.y += headH + 12;
        if (b.style === 'minimal') hline(c, c.y, t.line, 0.5);
        else hline(c, c.y, t.accent, 1.5);
        c.y += 18;
      }
      return;
    }
    case 'cols': {
      if (!b.cols.length) return;
      const gap = 28;
      const w = (inner - gap * (b.cols.length - 1)) / b.cols.length;
      const wrapped = b.cols.map((cl) => cl.lines.flatMap((l) => wrap(c.reg, l, 10, w)));
      const h = 14 + Math.max(...wrapped.map((l) => l.length)) * 14 + 8;
      ensure(c, h);
      b.cols.forEach((cl, i) => {
        const x = MARGIN + i * (w + gap);
        text(c, cl.label.toUpperCase(), x, c.y, 8, c.bold, t.muted);
        wrapped[i].forEach((l, j) => text(c, l, x, c.y + 14 + j * 14, 10, j === 0 && i < 2 ? c.bold : c.reg, t.ink));
      });
      c.y += h;
      return;
    }
    case 'table': {
      const widths = b.widths.map((f) => f * inner);
      const pad = 7;
      const drawHead = () => {
        ensure(c, 26);
        rect(c, MARGIN, c.y, inner, 22, t.fill);
        let x = MARGIN;
        b.head.forEach((h, i) => {
          const hx = x + pad;
          if (b.align[i] === 'r') text(c, h.toUpperCase(), 0, c.y + 7, 8, c.bold, t.muted, 'r', x + widths[i] - pad);
          else text(c, h.toUpperCase(), hx, c.y + 7, 8, c.bold, t.muted);
          x += widths[i];
        });
        c.y += 22;
      };
      drawHead();
      for (const row of b.rows) {
        const cells = row.map((cell, i) => wrap(c.reg, cell, 10, widths[i] - pad * 2));
        const rh = Math.max(...cells.map((l) => l.length)) * 14 + pad * 2 - 2;
        if (c.y + rh > c.H - MARGIN) { newPage(c); drawHead(); }
        let x = MARGIN;
        cells.forEach((ls, i) => {
          ls.forEach((l, j) => {
            const font = j === 0 || ls.length === 1 ? c.reg : c.reg;
            const color = j > 0 ? t.muted : t.ink;
            if (b.align[i] === 'r') text(c, l, 0, c.y + pad + j * 14, 10, font, color, 'r', x + widths[i] - pad);
            else text(c, l, x + pad, c.y + pad + j * 14, 10, font, color);
          });
          x += widths[i];
        });
        c.y += rh;
        hline(c, c.y, t.line, 0.5);
      }
      c.y += 10;
      return;
    }
    case 'totals': {
      const w = inner * 0.5;
      const x = c.W - MARGIN - w;
      ensure(c, b.rows.length * 20 + 16);
      for (const r of b.rows) {
        if (r.strong) { hline(c, c.y + 2, t.accent, 1.5, x, c.W - MARGIN); c.y += 8; }
        const size = r.strong ? 12 : 10;
        const font = r.strong ? c.bold : c.reg;
        text(c, r.label, x + 8, c.y, size, font, r.strong ? t.ink : t.muted);
        text(c, r.value, 0, c.y, size, font, t.ink, 'r', c.W - MARGIN - 8);
        c.y += r.strong ? 22 : 18;
      }
      c.y += 8;
      return;
    }
    case 'stats': {
      const gap = 10;
      const w = (inner - gap * (b.items.length - 1)) / b.items.length;
      ensure(c, 62);
      b.items.forEach((s, i) => {
        const x = MARGIN + i * (w + gap);
        rect(c, x, c.y, w, 54, t.fill);
        text(c, s.label.toUpperCase(), x + 10, c.y + 9, 8, c.reg, t.muted);
        text(c, s.value, x + 10, c.y + 21, 16, c.bold, t.ink);
        if (s.note) text(c, s.note, x + 10, c.y + 41, 8.5, c.reg, t.muted);
      });
      c.y += 66;
      return;
    }
    case 'heading':
      ensure(c, 44);
      c.y += 10;
      text(c, b.text, MARGIN, c.y, 12.5, c.bold, t.ink);
      c.y += 22;
      return;
    case 'text':
    case 'note': {
      const size = b.t === 'note' ? 9 : 10.5;
      const color = b.t === 'note' ? t.muted : t.ink;
      if (b.t === 'note') c.y += 8;
      for (const l of wrap(c.reg, b.text, size, inner)) {
        ensure(c, size * 1.5);
        text(c, l, MARGIN, c.y, size, c.reg, color);
        c.y += size * 1.5;
      }
      c.y += 6;
      return;
    }
    case 'list': {
      b.items.forEach((item, i) => {
        const lines = wrap(c.reg, item, 10.5, inner - 18);
        ensure(c, lines.length * 15);
        text(c, b.numbered ? `${i + 1}.` : '*', MARGIN + 2, c.y, 10.5, c.reg, t.muted);
        lines.forEach((l, j) => text(c, l, MARGIN + 18, c.y + j * 15, 10.5, c.reg, t.ink));
        c.y += lines.length * 15 + 2;
      });
      c.y += 6;
      return;
    }
    case 'ticket': {
      const stubW = 150;
      const mainW = inner - stubW;
      const rows = Math.ceil(b.lines.length / 2);
      const h = 64 + rows * 32 + (b.holder ? 20 : 0);
      ensure(c, h + 8);
      const top = c.y;
      c.page.drawRectangle({ x: MARGIN, y: c.H - top - h, width: inner, height: h, borderColor: col(t.line), borderWidth: 1 });
      rect(c, MARGIN + mainW, top + 0.5, stubW - 0.5, h - 1, t.fill);
      c.page.drawLine({ start: { x: MARGIN + mainW, y: c.H - top }, end: { x: MARGIN + mainW, y: c.H - top - h }, thickness: 1.2, color: col(t.line), dashArray: [4, 3] });
      let y = top + 18;
      wrap(c.bold, b.event, 18, mainW - 40).forEach((l) => { text(c, l, MARGIN + 20, y, 18, c.bold, t.ink); y += 22; });
      if (b.holder) { text(c, b.holder, MARGIN + 20, y, 10.5, c.reg, t.muted); y += 20; }
      y += 6;
      const cw = (mainW - 40) / 2;
      b.lines.forEach((l, i) => {
        const x = MARGIN + 20 + (i % 2) * (cw + 0);
        const yy = y + Math.floor(i / 2) * 32;
        text(c, l.label.toUpperCase(), x, yy, 8, c.reg, t.muted);
        text(c, wrap(c.reg, l.value, 10.5, cw - 8)[0] ?? '', x, yy + 12, 10.5, c.reg, t.ink);
      });
      const sx = MARGIN + mainW + stubW / 2;
      const label = 'TICKET CODE';
      text(c, label, sx - c.reg.widthOfTextAtSize(label, 8) / 2, top + h / 2 - 14, 8, c.reg, t.muted);
      const code = winAnsi(b.code);
      const size = c.mono.widthOfTextAtSize(code, 11) > stubW - 20 ? 8.5 : 11;
      text(c, code, sx - c.mono.widthOfTextAtSize(code, size) / 2, top + h / 2 + 2, size, c.mono, t.ink);
      c.y += h + 10;
      return;
    }
    case 'rule':
      ensure(c, 20);
      c.y += 6;
      hline(c, c.y, t.line, 0.5);
      c.y += 10;
      return;
    case 'space':
      c.y += b.h;
  }
}

export async function renderPdf(doc: PdfDocModel, fontBytes: PdfFontBytes): Promise<Uint8Array> {
  const pdf = await PDFDocument.create();
  pdf.registerFontkit(fontkit);
  pdf.setTitle(winAnsi(doc.title));
  pdf.setProducer('Allternit');
  pdf.setCreator('Allternit PDF templates');
  const [W, H] = PDF_PAGE_POINTS[doc.pageSize];
  const c: Ctx = {
    pdf, doc, W, H, y: MARGIN,
    page: pdf.addPage([W, H]),
    reg: await pdf.embedFont(fontBytes.regular, { subset: true }),
    bold: await pdf.embedFont(fontBytes.bold, { subset: true }),
    mono: null as unknown as PDFFont,
  };
  c.mono = c.reg;
  glyphs = new Set(c.reg.getCharacterSet());
  for (const b of doc.blocks) drawBlock(c, b);
  if (doc.footer) {
    ensure(c, 30);
    c.y += 14;
    hline(c, c.y, doc.theme.line, 0.5);
    text(c, doc.footer, MARGIN, c.y + 8, 8.5, c.reg, doc.theme.muted);
  }
  // Page numbers when there is more than one page.
  const pages = pdf.getPages();
  if (pages.length > 1) {
    pages.forEach((p, i) => {
      const s = `${i + 1} / ${pages.length}`;
      p.drawText(s, { x: W - MARGIN - c.reg.widthOfTextAtSize(s, 8), y: 24, size: 8, font: c.reg, color: col(doc.theme.muted) });
    });
  }
  return pdf.save();
}
