/**
 * A design system (OfficeHost.designSystem) as an OOXML document theme: the
 * 12 colour-scheme slots plus heading/body fonts, so Slides and Docs can apply
 * it through their existing theme engines (the saved .pptx/.docx carries it).
 *
 * Slots: dk1 = body text, lt1 = page background, dk2/lt2 = secondary
 * dark/light, accent1..6, hlink/folHlink. Colour names steer the choice
 * ("background", "text", "primary"…); without hints, the lightest neutral is
 * the page, the darkest the text, and the chromatic colours become accents in
 * the order the system lists them.
 */
import type { OfficeDesignSystem } from './types';

export type ThemeSlot =
  | 'dk1' | 'lt1' | 'dk2' | 'lt2'
  | 'accent1' | 'accent2' | 'accent3' | 'accent4' | 'accent5' | 'accent6'
  | 'hlink' | 'folHlink';

export interface DesignSystemTheme {
  name: string;
  /** Hex without '#', uppercase. */
  colors: Record<ThemeSlot, string>;
  majorFont?: string;
  minorFont?: string;
}

interface Swatch {
  name: string;
  hex: string;
  l: number;
  s: number;
}

function parseHex(value: string): string | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})(?:[0-9a-f]{2})?$/i.exec(value.trim());
  if (!m) return null;
  const digits = m[1]!;
  const h = digits.length === 3 ? digits.split('').map((c) => c + c).join('') : digits;
  return h.toUpperCase();
}

function swatch(name: string, hex: string): Swatch {
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255) as [number, number, number];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const s = max === min ? 0 : (max - min) / (1 - Math.abs(2 * l - 1));
  return { name: name.toLowerCase(), hex, l, s };
}

/** WCAG relative luminance, for the text/background contrast check. */
function luminance(hex: string): number {
  const [r, g, b] = [0, 2, 4].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p) as [number, number];
  return (x + 0.05) / (y + 0.05);
}

const NEUTRAL_SAT = 0.18;
const BG_HINT = /background|\bbg\b|canvas|paper|page|surface|base/;
const TEXT_HINT = /\btext\b|\bink\b|foreground|\bfg\b|body|copy/;
const ACCENT_HINT = /primary|accent|brand|cta|highlight/;

export function designSystemTheme(ds: OfficeDesignSystem): DesignSystemTheme | null {
  const seen = new Set<string>();
  const all: Swatch[] = [];
  for (const c of ds.colors) {
    const hex = parseHex(c.value);
    if (!hex || seen.has(hex)) continue;
    seen.add(hex);
    all.push(swatch(c.name, hex));
  }
  if (all.length < 2) return null;

  const neutral = (s: Swatch) => s.s < NEUTRAL_SAT || s.l > 0.94 || s.l < 0.08;
  const byLight = [...all].sort((a, b) => b.l - a.l);
  const lightest = byLight[0]!;
  const darkest = byLight[byLight.length - 1]!;

  let lt1 = all.find((s) => BG_HINT.test(s.name) && s.l > 0.5) ?? lightest;
  let dk1 = all.find((s) => TEXT_HINT.test(s.name) && s.l < 0.5 && s !== lt1) ?? darkest;
  if (contrast(dk1.hex, lt1.hex) < 4.5) {
    lt1 = lightest;
    dk1 = darkest;
  }
  const used = new Set([lt1.hex, dk1.hex]);

  const neutrals = byLight.filter((s) => neutral(s) && !used.has(s.hex));
  const lt2 = neutrals.find((s) => s.l >= 0.5) ?? lt1;
  const dk2 = [...neutrals].reverse().find((s) => s.l < 0.5) ?? dk1;
  used.add(lt2.hex);
  used.add(dk2.hex);

  const chromatic = all.filter((s) => !neutral(s) && !used.has(s.hex));
  const hinted = chromatic.filter((s) => ACCENT_HINT.test(s.name));
  const accents = [...hinted, ...chromatic.filter((s) => !hinted.includes(s))].map((s) => s.hex);
  // Fewer than six: fill with the mid neutrals, then repeat what there is.
  for (const s of neutrals) if (accents.length < 6 && !used.has(s.hex) && !accents.includes(s.hex)) accents.push(s.hex);
  if (!accents.length) accents.push(dk2.hex);
  while (accents.length < 6) accents.push(accents[accents.length % Math.max(1, accents.length)]!);

  const [a1, a2, a3, a4, a5, a6] = accents as [string, string, string, string, string, string];
  const [major, minor] = ds.fonts;
  return {
    name: ds.name,
    colors: {
      dk1: dk1.hex, lt1: lt1.hex, dk2: dk2.hex, lt2: lt2.hex,
      accent1: a1, accent2: a2, accent3: a3,
      accent4: a4, accent5: a5, accent6: a6,
      hlink: a1, folHlink: a2,
    },
    ...(major ? { majorFont: major, minorFont: minor ?? major } : {}),
  };
}
