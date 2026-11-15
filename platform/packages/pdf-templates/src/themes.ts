import type { PdfThemeId, PdfThemeTokens } from './types';

// Allternit tokens, flattened for print: white paper, graphite ink, and a
// single accent used for rules only. `fill` stands in for --neutral-fill.
export const PDF_THEME_TOKENS: Record<PdfThemeId, PdfThemeTokens> = {
  professional: {
    id: 'professional', label: 'Professional',
    ink: '#1F2328', muted: '#6B7078', line: '#D9DBDF', fill: '#F3F3F2',
    accent: '#B08D6E', band: '#1F2328', bandInk: '#FFFFFF',
  },
  minimal: {
    id: 'minimal', label: 'Minimal',
    ink: '#111111', muted: '#777777', line: '#DDDDDD', fill: '#F5F5F5',
    accent: '#111111', band: '#111111', bandInk: '#FFFFFF',
  },
  executive: {
    id: 'executive', label: 'Executive',
    ink: '#15202B', muted: '#5B6773', line: '#D3D9E0', fill: '#EEF1F5',
    accent: '#1F2A44', band: '#1F2A44', bandInk: '#FFFFFF',
  },
};

export function resolveTheme(id: unknown): PdfThemeTokens {
  return PDF_THEME_TOKENS[(typeof id === 'string' && id in PDF_THEME_TOKENS ? id : 'professional') as PdfThemeId];
}

export function hexToRgb01(hex: string): [number, number, number] {
  const n = parseInt(hex.replace('#', ''), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}
