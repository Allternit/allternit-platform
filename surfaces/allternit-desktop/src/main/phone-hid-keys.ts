/**
 * Keyboard side of the Phone Use keyboard/mouse core
 * (docs/contracts/CONTRACT-phone-use-2026-11-16.md §6, §9.2).
 *
 * Key combos ("cmd+space"), their USB HID usages (usage page 0x07) and
 * modifier bits, text to key strokes, and the key sequences an iPhone answers
 * from a hardware keyboard: Spotlight open-by-name, Home, Full Keyboard Access
 * navigation. Every sender (Mac, Android app, second Apple device, USB Agent
 * board) drives these through phone-hid-core.ts.
 *
 * Typing limit: a Bluetooth or USB keyboard sends key positions, not
 * characters. The phone turns them into text with its hardware-keyboard
 * layout (Settings > General > Keyboard > Hardware Keyboard). This table is
 * the US English layout, so text comes out right only when the phone uses a
 * US-compatible layout, and only printable ASCII can be sent at all.
 * Non-ASCII typing is UNVERIFIED (contract §3.2: `bad_params` for characters
 * the layout cannot send). Whether iOS answers each sequence below is plan
 * check P3 (not checked on hardware).
 */

/** Modifier bits, as in the contract's hid.key message and the HID boot report's modifier byte (left-hand keys). */
export const MOD = { ctrl: 1, shift: 2, alt: 4, cmd: 8 } as const;
export type ModifierName = keyof typeof MOD;

/** Named keys a combo may use, with their usage page 0x07 codes. */
export const NAMED_KEYS: Readonly<Record<string, number>> = {
  enter: 0x28,
  esc: 0x29,
  backspace: 0x2a,
  tab: 0x2b,
  space: 0x2c,
  right: 0x4f,
  left: 0x50,
  down: 0x51,
  up: 0x52,
};

/** One key press (with modifiers held) and release. `combo` is the canonical text form the USB Agent board takes. */
export interface KeyStroke {
  combo: string;
  /** Usage page 0x07 code; 0 for a combo of modifiers only. */
  usage: number;
  /** MOD bitmask. */
  modifiers: number;
}

/** Usage for a-z and 0-9. */
function charUsage(c: string): number | null {
  if (/^[a-z]$/.test(c)) return 0x04 + c.charCodeAt(0) - 0x61;
  if (/^[1-9]$/.test(c)) return 0x1e + c.charCodeAt(0) - 0x31;
  if (c === '0') return 0x27;
  return null;
}

const isModifier = (k: string): k is ModifierName => Object.prototype.hasOwnProperty.call(MOD, k);

/**
 * "cmd+space" → its stroke, or null when the combo is not one we send. The
 * grammar is the board's (phone-usb-agent-protocol.ts isValidCombo): 1–4
 * parts, each a modifier, a named key, or one of a-z / 0-9, with at most one
 * non-modifier key.
 */
export function parseCombo(combo: string): KeyStroke | null {
  if (typeof combo !== 'string') return null;
  const parts = combo.split('+');
  if (parts.length < 1 || parts.length > 4) return null;
  let modifiers = 0;
  let usage = 0;
  for (const p of parts) {
    if (isModifier(p)) {
      modifiers |= MOD[p];
      continue;
    }
    const u = NAMED_KEYS[p] ?? charUsage(p);
    if (u === null || u === undefined || usage !== 0) return null;
    usage = u;
  }
  return { combo, usage, modifiers };
}

/** US layout: printable ASCII that is not a letter or digit → [usage, needs shift]. */
const US_SYMBOLS: Readonly<Record<string, [number, boolean]>> = {
  ' ': [0x2c, false],
  '-': [0x2d, false],
  _: [0x2d, true],
  '=': [0x2e, false],
  '+': [0x2e, true],
  '[': [0x2f, false],
  '{': [0x2f, true],
  ']': [0x30, false],
  '}': [0x30, true],
  '\\': [0x31, false],
  '|': [0x31, true],
  ';': [0x33, false],
  ':': [0x33, true],
  "'": [0x34, false],
  '"': [0x34, true],
  '`': [0x35, false],
  '~': [0x35, true],
  ',': [0x36, false],
  '<': [0x36, true],
  '.': [0x37, false],
  '>': [0x37, true],
  '/': [0x38, false],
  '?': [0x38, true],
  '!': [0x1e, true],
  '@': [0x1f, true],
  '#': [0x20, true],
  $: [0x21, true],
  '%': [0x22, true],
  '^': [0x23, true],
  '&': [0x24, true],
  '*': [0x25, true],
  '(': [0x26, true],
  ')': [0x27, true],
};

/** One character on the US layout → its stroke, or null when the layout can't send it. */
export function charStroke(c: string): KeyStroke | null {
  if (c.length !== 1) return null;
  const lower = c.toLowerCase();
  const plain = charUsage(lower);
  if (plain !== null) {
    const shift = c !== lower;
    return { combo: shift ? `shift+${lower}` : lower, usage: plain, modifiers: shift ? MOD.shift : 0 };
  }
  const sym = US_SYMBOLS[c];
  if (!sym) return null;
  return { combo: c === ' ' ? 'space' : c, usage: sym[0], modifiers: sym[1] ? MOD.shift : 0 };
}

/** The first character the US layout can't send, or null when all of `text` can be typed. */
export function untypable(text: string): string | null {
  for (const c of text) if (!charStroke(c)) return c;
  return null;
}

/** Text → one stroke per character (for senders that only send key reports). Null when a character can't be sent. */
export function textStrokes(text: string): KeyStroke[] | null {
  const out: KeyStroke[] = [];
  for (const c of text) {
    const s = charStroke(c);
    if (!s) return null;
    out.push(s);
  }
  return out;
}

// ── Sequences an iPhone answers from a hardware keyboard (P3, not checked on hardware) ──

/** One step of a key sequence. */
export type KeyStep = { kind: 'combo'; combo: string } | { kind: 'text'; text: string } | { kind: 'wait'; ms: number };

/** Waits the USB Agent has used since it shipped: Spotlight needs a moment to open and to search. */
export const SPOTLIGHT_OPEN_MS = 600;
export const SPOTLIGHT_SEARCH_MS = 500;

/** Open an app by name: ⌘Space opens Spotlight, type the name, Return opens the top hit. */
export function spotlightOpenSteps(app: string): KeyStep[] {
  return [
    { kind: 'combo', combo: 'cmd+space' },
    { kind: 'wait', ms: SPOTLIGHT_OPEN_MS },
    { kind: 'text', text: app },
    { kind: 'wait', ms: SPOTLIGHT_SEARCH_MS },
    { kind: 'combo', combo: 'enter' },
  ];
}

/** ⌘H goes to the Home Screen from any app. */
export const HOME_COMBO = 'cmd+h';

/**
 * Full Keyboard Access (Settings > Accessibility > Keyboards): Tab and
 * Shift-Tab move between items, the arrows move inside a group, Space
 * activates the focused item, Escape goes back or dismisses.
 */
export const FKA_KEYS = {
  next: 'tab',
  previous: 'shift+tab',
  up: 'up',
  down: 'down',
  left: 'left',
  right: 'right',
  activate: 'space',
  back: 'esc',
} as const;
export type FkaStep = keyof typeof FKA_KEYS;
