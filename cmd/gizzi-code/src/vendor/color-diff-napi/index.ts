/**
 * Production Color Diff NAPI Module
 *
 * Provides syntax highlighting and color diff utilities for terminal output.
 * Implements the full API surface of the original color-diff-napi package.
 */

import {
  ANSI_RESET,
  bgAnsi,
  emitSpans,
  fgAnsi,
  SGR_DIM,
  highlightSpans,
  mapThemeName,
  resolveLanguage,
  wrapSpans,
} from './syntax.js'
import chalk from '@/shared/util/chalk'

/** Minimal shape of a `diff` package StructuredPatchHunk. */
interface StructuredPatchHunkLike {
  oldStart: number
  oldLines: number
  newStart: number
  newLines: number
  lines: string[]
}

// ============================================================================
// Type Definitions
// ============================================================================

export interface RGBColor {
  r: number;
  g: number;
  b: number;
}

export interface LabColor {
  l: number;
  a: number;
  b: number;
}

export interface ColorRange {
  min: number;
  max: number;
}

export interface SyntaxTheme {
  name: string;
  colors: Record<string, string>;
  background?: string;
  foreground?: string;
  tokenColors?: Array<{
    name?: string;
    scope: string | string[];
    settings: {
      foreground?: string;
      background?: string;
      fontStyle?: string;
    };
  }>;
}

// Extra fields surfaced by the theme picker footer (`syntaxTheme.theme`
// / `syntaxTheme.source`); attached by withMeta at load time.
export type SyntaxThemeWithMeta = SyntaxTheme & {
  theme?: string;
  source?: string;
};

export interface DiffResult {
  distance: number;
  similar: boolean;
}

// ============================================================================
// Color Conversion Utilities
// ============================================================================

/**
 * Convert RGB to XYZ color space
 */
function rgbToXyz(rgb: RGBColor): { x: number; y: number; z: number } {
  let r = rgb.r / 255;
  let g = rgb.g / 255;
  let b = rgb.b / 255;

  // Apply gamma correction
  r = r > 0.04045 ? Math.pow((r + 0.055) / 1.055, 2.4) : r / 12.92;
  g = g > 0.04045 ? Math.pow((g + 0.055) / 1.055, 2.4) : g / 12.92;
  b = b > 0.04045 ? Math.pow((b + 0.055) / 1.055, 2.4) : b / 12.92;

  // Convert to XYZ using sRGB matrix
  const x = r * 0.4124564 + g * 0.3575761 + b * 0.1804375;
  const y = r * 0.2126729 + g * 0.7151522 + b * 0.0721750;
  const z = r * 0.0193339 + g * 0.1191920 + b * 0.9503041;

  return { x: x * 100, y: y * 100, z: z * 100 };
}

/**
 * Convert XYZ to Lab color space
 */
function xyzToLab(xyz: { x: number; y: number; z: number }): LabColor {
  // D65 illuminant
  const xRef = 95.047;
  const yRef = 100.000;
  const zRef = 108.883;

  let x = xyz.x / xRef;
  let y = xyz.y / yRef;
  let z = xyz.z / zRef;

  // CIE Epsilon and Kappa
  const epsilon = 0.008856;
  const kappa = 903.3;

  x = x > epsilon ? Math.pow(x, 1 / 3) : (kappa * x + 16) / 116;
  y = y > epsilon ? Math.pow(y, 1 / 3) : (kappa * y + 16) / 116;
  z = z > epsilon ? Math.pow(z, 1 / 3) : (kappa * z + 16) / 116;

  const l = 116 * y - 16;
  const a = 500 * (x - y);
  const b = 200 * (y - z);

  return { l, a, b };
}

/**
 * Convert RGB to Lab color space
 */
export function rgb_to_lab(rgb: RGBColor): LabColor {
  const xyz = rgbToXyz(rgb);
  return xyzToLab(xyz);
}

// ============================================================================
// CIEDE2000 Color Difference
// ============================================================================

/**
 * Calculate CIEDE2000 color difference between two Lab colors
 * This is the industry standard for perceptual color difference
 */
export function ciede2000(lab1: LabColor, lab2: LabColor): number {
  const { l: l1, a: a1, b: b1 } = lab1;
  const { l: l2, a: a2, b: b2 } = lab2;

  // CIE 2000 parameters
  const kl = 1;
  const kc = 1;
  const kh = 1;

  // Calculate C1, C2
  const c1Star = Math.sqrt(a1 * a1 + b1 * b1);
  const c2Star = Math.sqrt(a2 * a2 + b2 * b2);

  // Calculate CBar
  const cBar = (c1Star + c2Star) / 2;

  // Calculate G
  const g = 0.5 * (1 - Math.sqrt(Math.pow(cBar, 7) / (Math.pow(cBar, 7) + Math.pow(25, 7))));

  // Calculate a1', a2'
  const a1Prime = a1 * (1 + g);
  const a2Prime = a2 * (1 + g);

  // Calculate C1', C2'
  const c1Prime = Math.sqrt(a1Prime * a1Prime + b1 * b1);
  const c2Prime = Math.sqrt(a2Prime * a2Prime + b2 * b2);

  // Calculate h1', h2'
  let h1Prime = Math.atan2(b1, a1Prime);
  if (h1Prime < 0) h1Prime += 2 * Math.PI;

  let h2Prime = Math.atan2(b2, a2Prime);
  if (h2Prime < 0) h2Prime += 2 * Math.PI;

  // Calculate DeltaL', DeltaC', Deltah'
  const deltaLPrime = l2 - l1;
  const deltaCPrime = c2Prime - c1Prime;

  let deltahPrime = 0;
  if (c1Prime * c2Prime !== 0) {
    deltahPrime = h2Prime - h1Prime;
    if (deltahPrime > Math.PI) deltahPrime -= 2 * Math.PI;
    if (deltahPrime < -Math.PI) deltahPrime += 2 * Math.PI;
  }

  const deltaHPrime = 2 * Math.sqrt(c1Prime * c2Prime) * Math.sin(deltahPrime / 2);

  // Calculate CIEDE2000
  const lBarPrime = (l1 + l2) / 2;
  const cBarPrime = (c1Prime + c2Prime) / 2;

  let hBarPrime = (h1Prime + h2Prime) / 2;
  if (Math.abs(h1Prime - h2Prime) > Math.PI) {
    hBarPrime += Math.PI;
  }

  const t = 1 -
    0.17 * Math.cos(hBarPrime - Math.PI / 6) +
    0.24 * Math.cos(2 * hBarPrime) +
    0.32 * Math.cos(3 * hBarPrime + Math.PI / 30) -
    0.20 * Math.cos(4 * hBarPrime - 63 * Math.PI / 180);

  const deltaTheta = 30 * Math.PI / 180 * Math.exp(-Math.pow((hBarPrime - 275 * Math.PI / 180) / (25 * Math.PI / 180), 2));
  const rc = 2 * Math.sqrt(Math.pow(cBarPrime, 7) / (Math.pow(cBarPrime, 7) + Math.pow(25, 7)));
  const sl = 1 + (0.015 * Math.pow(lBarPrime - 50, 2)) / Math.sqrt(20 + Math.pow(lBarPrime - 50, 2));
  const sc = 1 + 0.045 * cBarPrime;
  const sh = 1 + 0.015 * cBarPrime * t;
  const rt = -Math.sin(2 * deltaTheta) * rc;

  const deltaL = deltaLPrime / (kl * sl);
  const deltaC = deltaCPrime / (kc * sc);
  const deltaH = deltaHPrime / (kh * sh);

  return Math.sqrt(
    deltaL * deltaL +
    deltaC * deltaC +
    deltaH * deltaH +
    rt * deltaC * deltaH
  );
}

// ============================================================================
// ColorDiff Main Class
// ============================================================================

export class ColorDiff {
  private threshold: number;
  // Diff-render construction (patch, firstLine, filePath, fileContent), used
  // by StructuredDiff's fast path. Shape mirrors the original NAPI binding.
  private renderPatch: unknown;
  private renderFirstLine: string | null;
  private renderFilePath: string;
  private renderFileContent: string | null;

  constructor(threshold?: number);
  constructor(patch: unknown, firstLine?: string | null, filePath?: string, fileContent?: string | null);
  constructor(arg1: number | unknown = 2.3, firstLine: string | null = null, filePath: string = '', fileContent: string | null = null) {
    if (typeof arg1 === 'number') {
      this.threshold = arg1;
      this.renderPatch = null;
    } else {
      // Diff-render mode: threshold is irrelevant, render() does the work.
      this.threshold = 2.3;
      this.renderPatch = arg1;
    }
    this.renderFirstLine = firstLine;
    this.renderFilePath = filePath ?? '';
    this.renderFileContent = fileContent ?? null;
  }

  /**
   * Render a structured diff hunk as ANSI-highlighted lines, wrapped to
   * `width`. Matches Claude Code's layout: a gutter of `right-aligned line
   * number + space + marker + space` (max line-number width + 3), tinted with
   * the row's diff background and the marker in the theme's diff word color;
   * context gutters are dim. Content is syntax-highlighted. Returns null in
   * color-math mode or when the render inputs are unusable, in which case
   * callers fall back to their React fallback renderer.
   */
  render(theme: string, width: number, dim: boolean): string[] | null {
    const patch = this.renderPatch as StructuredPatchHunkLike | null;
    if (patch === null || !Array.isArray(patch.lines) || typeof width !== 'number' || width < 12) {
      return null;
    }

    const { palette } = mapThemeName(theme);
    const lang = resolveLanguage(this.renderFilePath, this.renderFirstLine);

    const maxLineNumber = Math.max(
      patch.oldStart + patch.oldLines - 1,
      patch.newStart + patch.newLines - 1,
      1,
    );
    const digits = maxLineNumber.toString().length;
    const gutterWidth = digits + 3;
    const contentWidth = Math.max(1, width - gutterWidth);

    const addedBg = bgAnsi(dim ? palette.addedDim : palette.added, palette.ansi);
    const removedBg = bgAnsi(dim ? palette.removedDim : palette.removed, palette.ansi);

    const out: string[] = [];
    let oldLine = patch.oldStart;
    let newLine = patch.newStart;

    const addedGutter = addedBg + fgAnsi(palette.addedWord, palette.ansi, '\x1b[92m');
    const removedGutter = removedBg + fgAnsi(palette.removedWord, palette.ansi, '\x1b[91m');
    const blankGutter = ' '.repeat(gutterWidth);
    const gutter = (marker: string, num: number | null, style: string) => {
      const text = (num === null ? ' '.repeat(digits) : String(num).padStart(digits)) + ' ' + marker + ' ';
      return style ? style + text + ANSI_RESET : text;
    };

    for (const raw of patch.lines) {
      if (typeof raw !== 'string') continue;
      // "\ No newline at end of file" — metadata line, no gutter content.
      if (raw.startsWith('\\')) {
        out.push(blankGutter + '\x1b[2m' + raw.trimEnd() + ANSI_RESET);
        continue;
      }
      const marker = raw.charAt(0);
      const code = raw.slice(1);
      let bg: string | undefined;
      let num: number;
      if (marker === '+') {
        bg = addedBg;
        num = newLine++;
      } else if (marker === '-') {
        bg = removedBg;
        num = oldLine++;
      } else {
        num = newLine++;
        if (marker === ' ') oldLine++;
      }
      const spans = highlightSpans(code, lang, palette, bg, false);
      const rows = wrapSpans(spans, contentWidth);
      for (let i = 0; i < rows.length; i++) {
        // Pad tinted rows to full content width so the background runs to the
        // edge, matching the React fallback's Box background behaviour.
        const pad = bg ? contentWidth - rows[i]!.reduce((n, s) => n + s.text.length, 0) : 0;
        const padSpan = pad > 0 && bg ? [{ text: ' '.repeat(pad), bg }] : [];
        const style = marker === '+' ? addedGutter : marker === '-' ? removedGutter : chalk.level > 0 ? SGR_DIM : '';
        const g = gutter(i === 0 && (marker === '+' || marker === '-') ? marker : ' ', i === 0 ? num : null, style);
        out.push(g + emitSpans([...rows[i]!, ...padSpan]));
      }
    }
    return out;
  }

  /**
   * Calculate color difference between two RGB colors
   */
  diff(rgb1: RGBColor, rgb2: RGBColor): number {
    const lab1 = rgb_to_lab(rgb1);
    const lab2 = rgb_to_lab(rgb2);
    return ciede2000(lab1, lab2);
  }

  /**
   * Check if two colors are similar (below threshold)
   */
  similar(rgb1: RGBColor, rgb2: RGBColor): boolean {
    return this.diff(rgb1, rgb2) < this.threshold;
  }

  /**
   * Find the closest color from a palette
   */
  closest(rgb: RGBColor, palette: RGBColor[]): RGBColor | null {
    if (palette.length === 0) return null;

    let closestColor = palette[0];
    let minDistance = this.diff(rgb, palette[0]);

    for (let i = 1; i < palette.length; i++) {
      const distance = this.diff(rgb, palette[i]);
      if (distance < minDistance) {
        minDistance = distance;
        closestColor = palette[i];
      }
    }

    return closestColor;
  }

  /**
   * Map a color to the closest in a palette, returning the palette index
   */
  map(rgb: RGBColor, palette: RGBColor[]): number {
    if (palette.length === 0) return -1;

    let closestIndex = 0;
    let minDistance = this.diff(rgb, palette[0]);

    for (let i = 1; i < palette.length; i++) {
      const distance = this.diff(rgb, palette[i]);
      if (distance < minDistance) {
        minDistance = distance;
        closestIndex = i;
      }
    }

    return closestIndex;
  }
}

// ============================================================================
// ColorFile Class
// ============================================================================

export class ColorFile {
  private colors: RGBColor[];
  // Code-render construction (code, filePath), used by HighlightedCode's fast
  // path. Mirrors the original NAPI binding, whose ColorFile highlighted a
  // whole file. `null` in palette mode.
  private renderCode: string[] | null;
  private renderLang: string;

  constructor(colors?: RGBColor[]);
  constructor(code: string, filePath?: string);
  constructor(arg1: RGBColor[] | string = [], filePath: string = '') {
    if (typeof arg1 === 'string') {
      this.colors = [];
      this.renderCode = arg1.length > 0 ? arg1.split('\n') : [''];
      this.renderLang = resolveLanguage(filePath);
    } else {
      this.colors = arg1;
      this.renderCode = null;
      this.renderLang = 'text';
    }
  }

  /**
   * Highlight the file as ANSI lines, wrapped to `width`, with a
   * `space + line number + space` gutter padded to the line-count width + 2
   * (matching the consumer's gutterWidth calculation). Returns null in
   * palette mode.
   */
  render(theme: string, width: number, dim: boolean): string[] | null {
    if (this.renderCode === null || typeof width !== 'number' || width < 8) {
      return null;
    }
    const { palette } = mapThemeName(theme);
    const digits = Math.max(1, String(this.renderCode.length).length);
    const gutterWidth = digits + 2;
    const contentWidth = Math.max(1, width - gutterWidth);

    const out: string[] = [];
    for (let i = 0; i < this.renderCode.length; i++) {
      const gutter =
        ' ' + String(i + 1).padStart(digits) + ' ';
      const spans = highlightSpans(this.renderCode[i]!, this.renderLang, palette, undefined, dim);
      const rows = wrapSpans(spans, contentWidth);
      for (let j = 0; j < rows.length; j++) {
        out.push((j === 0 ? gutter : ' '.repeat(gutterWidth)) + emitSpans(rows[j]!));
      }
    }
    return out;
  }

  /**
   * Read colors from various formats
   */
  static read(content: string, format: 'hex' | 'rgb' | 'css' = 'hex'): RGBColor[] {
    const colors: RGBColor[] = [];

    if (format === 'hex') {
      const hexRegex = /#?([0-9a-fA-F]{6}|[0-9a-fA-F]{3})/g;
      let match;
      while ((match = hexRegex.exec(content)) !== null) {
        colors.push(ColorFile.hexToRgb(match[1]));
      }
    } else if (format === 'rgb') {
      const rgbRegex = /rgb\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)/g;
      let match;
      while ((match = rgbRegex.exec(content)) !== null) {
        colors.push({
          r: parseInt(match[1], 10),
          g: parseInt(match[2], 10),
          b: parseInt(match[3], 10),
        });
      }
    }

    return colors;
  }

  /**
   * Convert hex color to RGB
   */
  static hexToRgb(hex: string): RGBColor {
    // Expand shorthand form (e.g. "03F") to full form (e.g. "0033FF")
    const fullHex = hex.length === 3
      ? hex.split('').map(c => c + c).join('')
      : hex;

    const int = parseInt(fullHex, 16);
    return {
      r: (int >> 16) & 255,
      g: (int >> 8) & 255,
      b: int & 255,
    };
  }

  /**
   * Convert RGB to hex color
   */
  static rgbToHex(rgb: RGBColor): string {
    const toHex = (n: number) => {
      const hex = Math.round(Math.max(0, Math.min(255, n))).toString(16);
      return hex.length === 1 ? '0' + hex : hex;
    };
    return `#${toHex(rgb.r)}${toHex(rgb.g)}${toHex(rgb.b)}`;
  }

  /**
   * Get all colors
   */
  getColors(): RGBColor[] {
    return [...this.colors];
  }

  /**
   * Add a color
   */
  add(color: RGBColor): void {
    this.colors.push(color);
  }

  /**
   * Clear all colors
   */
  clear(): void {
    this.colors = [];
  }
}

// ============================================================================
// Syntax Themes
// ============================================================================

const BUILT_IN_THEMES: Record<string, SyntaxTheme> = {
  'dark-plus': {
    name: 'Dark+',
    colors: {
      'editor.background': '#1e1e1e',
      'editor.foreground': '#d4d4d4',
      'editor.lineHighlightBackground': '#2d2d2d',
      'editor.selectionBackground': '#264f78',
      'editorCursor.foreground': '#aeafad',
    },
    tokenColors: [
      {
        scope: ['comment', 'punctuation.definition.comment'],
        settings: { foreground: '#6a9955' },
      },
      {
        scope: ['keyword', 'storage.type', 'storage.modifier'],
        settings: { foreground: '#569cd6' },
      },
      {
        scope: ['string', 'string.quoted'],
        settings: { foreground: '#ce9178' },
      },
      {
        scope: ['constant.numeric'],
        settings: { foreground: '#b5cea8' },
      },
      {
        scope: ['entity.name.function', 'support.function'],
        settings: { foreground: '#dcdcaa' },
      },
      {
        scope: ['entity.name.type', 'support.type'],
        settings: { foreground: '#4ec9b0' },
      },
      {
        scope: ['variable', 'identifier'],
        settings: { foreground: '#9cdcfe' },
      },
    ],
  },
  'light-plus': {
    name: 'Light+',
    colors: {
      'editor.background': '#ffffff',
      'editor.foreground': '#000000',
      'editor.lineHighlightBackground': '#f0f0f0',
      'editor.selectionBackground': '#add6ff',
      'editorCursor.foreground': '#000000',
    },
    tokenColors: [
      {
        scope: ['comment', 'punctuation.definition.comment'],
        settings: { foreground: '#008000' },
      },
      {
        scope: ['keyword', 'storage.type', 'storage.modifier'],
        settings: { foreground: '#0000ff' },
      },
      {
        scope: ['string', 'string.quoted'],
        settings: { foreground: '#a31515' },
      },
      {
        scope: ['constant.numeric'],
        settings: { foreground: '#098658' },
      },
      {
        scope: ['entity.name.function', 'support.function'],
        settings: { foreground: '#795e26' },
      },
      {
        scope: ['entity.name.type', 'support.type'],
        settings: { foreground: '#267f99' },
      },
      {
        scope: ['variable', 'identifier'],
        settings: { foreground: '#001080' },
      },
    ],
  },
  monokai: {
    name: 'Monokai',
    colors: {
      'editor.background': '#272822',
      'editor.foreground': '#f8f8f2',
      'editor.lineHighlightBackground': '#3e3d32',
      'editor.selectionBackground': '#49483e',
      'editorCursor.foreground': '#f8f8f0',
    },
    tokenColors: [
      {
        scope: ['comment'],
        settings: { foreground: '#75715e' },
      },
      {
        scope: ['keyword', 'storage'],
        settings: { foreground: '#f92672' },
      },
      {
        scope: ['string'],
        settings: { foreground: '#e6db74' },
      },
      {
        scope: ['constant.numeric'],
        settings: { foreground: '#ae81ff' },
      },
      {
        scope: ['entity.name.function'],
        settings: { foreground: '#a6e22e' },
      },
      {
        scope: ['entity.name.type', 'support.type'],
        settings: { foreground: '#66d9ef' },
      },
      {
        scope: ['variable'],
        settings: { foreground: '#f8f8f2' },
      },
    ],
  },
  'solarized-dark': {
    name: 'Solarized Dark',
    colors: {
      'editor.background': '#002b36',
      'editor.foreground': '#839496',
      'editor.lineHighlightBackground': '#073642',
      'editor.selectionBackground': '#073642',
      'editorCursor.foreground': '#839496',
    },
    tokenColors: [
      {
        scope: ['comment'],
        settings: { foreground: '#586e75', fontStyle: 'italic' },
      },
      {
        scope: ['keyword'],
        settings: { foreground: '#859900' },
      },
      {
        scope: ['string'],
        settings: { foreground: '#2aa198' },
      },
      {
        scope: ['constant.numeric'],
        settings: { foreground: '#d33682' },
      },
      {
        scope: ['entity.name.function'],
        settings: { foreground: '#268bd2' },
      },
      {
        scope: ['entity.name.type'],
        settings: { foreground: '#b58900' },
      },
    ],
  },
};

/**
 * Get a syntax theme by name. Accepts the TUI theme names
 * ('dark', 'light', '*-daltonized', '*-ansi') used by useTheme(), mapping
 * them to the nearest TextMate-style built-in, as well as the built-in
 * names themselves ('dark-plus', 'light-plus', ...).
 */
export function getSyntaxTheme(themeName: string): SyntaxThemeWithMeta | null {
  const direct = BUILT_IN_THEMES[themeName];
  if (direct) return withMeta(direct, themeName, 'built-in');
  // Only recognizable TUI theme names map onto the built-in palettes;
  // anything else is genuinely unknown.
  if (!/^(dark|light)(-(daltonized|ansi))?$/.test((themeName || '').toLowerCase())) {
    return null;
  }
  const { syntaxThemeName } = mapThemeName(themeName);
  const mapped = BUILT_IN_THEMES[syntaxThemeName];
  if (!mapped) return null;
  return withMeta(mapped, syntaxThemeName, 'built-in');
}

function withMeta(theme: SyntaxTheme, name: string, source: string): SyntaxThemeWithMeta {
  return {
    ...theme,
    name: theme.name || name,
    theme: theme.name || name,
    source,
  };
}

/**
 * List available syntax themes
 */
export function listSyntaxThemes(): string[] {
  return Object.keys(BUILT_IN_THEMES);
}

// ============================================================================
// ANSI Color Utilities
// ============================================================================

export interface AnsiColor {
  code: number;
  rgb: RGBColor;
}

// Standard ANSI 256 colors
const ANSI_COLORS: AnsiColor[] = [];

// Generate standard 16 colors
const STANDARD_COLORS: RGBColor[] = [
  { r: 0, g: 0, b: 0 },       // black
  { r: 128, g: 0, b: 0 },     // red
  { r: 0, g: 128, b: 0 },     // green
  { r: 128, g: 128, b: 0 },   // yellow
  { r: 0, g: 0, b: 128 },     // blue
  { r: 128, g: 0, b: 128 },   // magenta
  { r: 0, g: 128, b: 128 },   // cyan
  { r: 192, g: 192, b: 192 }, // white
  { r: 128, g: 128, b: 128 }, // bright black
  { r: 255, g: 0, b: 0 },     // bright red
  { r: 0, g: 255, b: 0 },     // bright green
  { r: 255, g: 255, b: 0 },   // bright yellow
  { r: 0, g: 0, b: 255 },     // bright blue
  { r: 255, g: 0, b: 255 },   // bright magenta
  { r: 0, g: 255, b: 255 },   // bright cyan
  { r: 255, g: 255, b: 255 }, // bright white
];

// Generate 216 color cube (16-231)
for (let r = 0; r < 6; r++) {
  for (let g = 0; g < 6; g++) {
    for (let b = 0; b < 6; b++) {
      STANDARD_COLORS.push({
        r: r === 0 ? 0 : 55 + r * 40,
        g: g === 0 ? 0 : 55 + g * 40,
        b: b === 0 ? 0 : 55 + b * 40,
      });
    }
  }
}

// Generate grayscale (232-255)
for (let i = 0; i < 24; i++) {
  const v = 8 + i * 10;
  STANDARD_COLORS.push({ r: v, g: v, b: v });
}

// Build ANSI_COLORS array
STANDARD_COLORS.forEach((rgb, i) => {
  ANSI_COLORS.push({ code: i, rgb });
});

/**
 * Find the closest ANSI 256 color to an RGB color
 */
export function findClosestAnsiColor(rgb: RGBColor): number {
  const colorDiff = new ColorDiff();
  let closestCode = 0;
  let minDistance = Infinity;

  for (const ansiColor of ANSI_COLORS) {
    const distance = colorDiff.diff(rgb, ansiColor.rgb);
    if (distance < minDistance) {
      minDistance = distance;
      closestCode = ansiColor.code;
    }
  }

  return closestCode;
}

/**
 * Convert RGB to ANSI escape code
 */
export function rgbToAnsi(rgb: RGBColor, background: boolean = false): string {
  const code = findClosestAnsiColor(rgb);
  const prefix = background ? 48 : 38;
  return `\x1b[${prefix};5;${code}m`;
}

/**
 * Convert RGB to true color ANSI escape code
 */
export function rgbToTrueColorAnsi(rgb: RGBColor, background: boolean = false): string {
  const prefix = background ? 48 : 38;
  return `\x1b[${prefix};2;${rgb.r};${rgb.g};${rgb.b}m`;
}

// ============================================================================
// Default Export
// ============================================================================

export default ColorDiff;
