/**
 * Contract for the document templates. A request names a template, a theme and
 * the data; templates turn the data into a flat list of layout blocks; the HTML
 * and PDF renderers both draw those same blocks, so a document looks the same
 * in the preview, in Desktop's print-to-PDF and in the server-rendered PDF.
 */

export const PDF_TEMPLATES = ['invoice', 'quote', 'contract', 'report', 'ticket', 'minutes', 'receipt'] as const;
export type PdfTemplateId = (typeof PDF_TEMPLATES)[number];

export const PDF_THEMES = ['professional', 'minimal', 'executive'] as const;
export type PdfThemeId = (typeof PDF_THEMES)[number];

/** Masthead style. Only the invoice offers all three; other templates use classic or minimal. */
export const PDF_VARIANTS = ['classic', 'minimal', 'modern'] as const;
export type PdfVariant = (typeof PDF_VARIANTS)[number];

export type PdfPageSize = 'letter' | 'a4';

export interface PdfRenderRequest {
  template: PdfTemplateId;
  theme?: PdfThemeId;
  variant?: PdfVariant;
  pageSize?: PdfPageSize;
  /** Template data. Unknown keys are ignored; missing keys render empty. */
  data: Record<string, unknown>;
}

export interface PdfTemplateInfo {
  id: PdfTemplateId;
  label: string;
  description: string;
  /** Variants this template offers (first is the default). */
  variants: PdfVariant[];
  /** A filled-in example, shown in the picker preview and used by `gizzi pdf render --sample`. */
  sample: Record<string, unknown>;
}

export interface PdfThemeTokens {
  id: PdfThemeId;
  label: string;
  ink: string;
  muted: string;
  /** Hairlines and table rules. */
  line: string;
  /** Solid stand-in for --neutral-fill (table headers, stat tiles). White page, never tan. */
  fill: string;
  accent: string;
  /** Masthead band (modern variant) and its text. */
  band: string;
  bandInk: string;
}

export type PdfBlock =
  | { t: 'masthead'; title: string; subtitle?: string; brand?: string; meta: [string, string][]; style: PdfVariant }
  | { t: 'cols'; cols: { label: string; lines: string[] }[] }
  | { t: 'table'; head: string[]; rows: string[][]; align: ('l' | 'r')[]; widths: number[] }
  | { t: 'totals'; rows: { label: string; value: string; strong?: boolean }[] }
  | { t: 'stats'; items: { label: string; value: string; note?: string }[] }
  | { t: 'heading'; text: string }
  | { t: 'text'; text: string }
  | { t: 'list'; items: string[]; numbered?: boolean }
  | { t: 'note'; text: string }
  | { t: 'ticket'; event: string; lines: { label: string; value: string }[]; code: string; holder?: string }
  | { t: 'signatures'; note?: string; slots: PdfSignatureSlot[] }
  | { t: 'rule' }
  | { t: 'space'; h: number };

/** One signer's place in a `signatures` block. `signed_on` prints "Signed <date>" under the line. */
export interface PdfSignatureSlot {
  id: string;
  label: string;
  name?: string;
  signed_on?: string;
}

/**
 * Where the PDF writer drew a signing area: PDF points, origin bottom-left,
 * `page` 0-based. Same shape as the sign-requests API `SignSlot`.
 */
export interface SignSlot {
  id: string;
  label: string;
  page: number;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface PdfDocModel {
  title: string;
  template: PdfTemplateId;
  theme: PdfThemeTokens;
  pageSize: PdfPageSize;
  blocks: PdfBlock[];
  footer?: string;
}
