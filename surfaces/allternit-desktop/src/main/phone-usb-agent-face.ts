/**
 * The Allternit Link face: one pure renderer for the 200 x 200 1-bit e-paper
 * canvas (contract hardware/link/README.md section 3).
 *
 * The same function is written in C++ in
 * hardware/usb-agent/firmware/usb_agent/face.h with the same integer rules,
 * and phone-usb-agent-face.test.ts compiles the C++ on the host and checks
 * that both make the same 5000 bytes for every face, prog and armed value.
 * With no panel on hand, that twin is what proves the faces.
 *
 * Buffer: row-major, 25 bytes per row, most significant bit first,
 * 1 = black, 0 = white. Gizzi has two solid square eyes, no nose and no
 * mouth; the coral `A://` is printed on the window, so the keep-out area
 * under the eyes is always white.
 */

import type { FaceName } from './phone-usb-agent-protocol.js';

export const FACE_W = 200;
export const FACE_H = 200;
export const FACE_BYTES = 5000;
const ROW_BYTES = FACE_W / 8;

type Rect = readonly [x: number, y: number, w: number, h: number];

const EYES_OPEN: readonly Rect[] = [
  [46, 62, 36, 36],
  [118, 62, 36, 36],
];

/** Rectangles per face, from contract section 3.2. */
export const FACE_RECTS: Readonly<Record<FaceName, readonly Rect[]>> = {
  default: EYES_OPEN,
  wink: [
    [46, 62, 36, 36],
    [118, 76, 36, 8],
  ],
  working: [
    [46, 72, 36, 16],
    [118, 72, 36, 16],
  ],
  needs_you: [...EYES_OPEN, [96, 10, 8, 28], [96, 42, 8, 8]],
  stopped: [
    [46, 77, 36, 6],
    [118, 77, 36, 6],
  ],
};

export const PROG_SQUARE_X = [26, 58, 90, 122, 154] as const;
export const PROG_SQUARE_Y = 164;
export const PROG_SQUARE_SIZE = 20;
export const ARMED_MARK: Rect = [184, 8, 8, 8];
/** Keep-out for the printed `A://`: x 60 to 139, y 118 to 152, inclusive. */
export const KEEP_OUT: Rect = [60, 118, 80, 35];

function paint(out: Uint8Array, [x, y, w, h]: Rect, black: boolean): void {
  const x0 = Math.max(0, x);
  const y0 = Math.max(0, y);
  const x1 = Math.min(FACE_W, x + w);
  const y1 = Math.min(FACE_H, y + h);
  for (let yy = y0; yy < y1; yy += 1) {
    for (let xx = x0; xx < x1; xx += 1) {
      const i = yy * ROW_BYTES + (xx >> 3);
      const mask = 0x80 >> (xx & 7);
      out[i] = black ? out[i] | mask : out[i] & ~mask;
    }
  }
}

export function renderFace(face: FaceName, prog: number, armed: boolean): Uint8Array {
  const out = new Uint8Array(FACE_BYTES);
  const rects = FACE_RECTS[face] ?? FACE_RECTS.default;
  for (const r of rects) paint(out, r, true);
  const p = Math.max(0, Math.min(5, Math.trunc(prog)));
  if (p > 0 || face === 'working') {
    for (let i = 0; i < 5; i += 1) {
      const x = PROG_SQUARE_X[i];
      const y = PROG_SQUARE_Y;
      const s = PROG_SQUARE_SIZE;
      if (i + 1 <= p) {
        paint(out, [x, y, s, s], true);
      } else {
        paint(out, [x, y, s, 2], true);
        paint(out, [x, y + s - 2, s, 2], true);
        paint(out, [x, y, 2, s], true);
        paint(out, [x + s - 2, y, 2, s], true);
      }
    }
  }
  if (armed) paint(out, ARMED_MARK, true);
  paint(out, KEEP_OUT, false);
  return out;
}

export function pixel(bits: Uint8Array, x: number, y: number): 0 | 1 {
  return (bits[y * ROW_BYTES + (x >> 3)] & (0x80 >> (x & 7))) !== 0 ? 1 : 0;
}

/** P1 (plain PBM) text, for test output and review. */
export function toPbm(bits: Uint8Array): string {
  const rows: string[] = [];
  for (let y = 0; y < FACE_H; y += 1) {
    let row = '';
    for (let x = 0; x < FACE_W; x += 1) row += pixel(bits, x, y) ? '1' : '0';
    rows.push(row);
  }
  return `P1\n${FACE_W} ${FACE_H}\n${rows.join('\n')}\n`;
}

// ── display interface (Desktop twin of display.h) ───────────────────────────

export interface LinkDisplay {
  readonly width: number;
  readonly height: number;
  busy(): boolean;
  show(bits: Uint8Array, full: boolean): void;
  sleep(): void;
}

/** A panel that records every frame it is asked to show. Never busy. */
export class FakeDisplay implements LinkDisplay {
  readonly width = FACE_W;
  readonly height = FACE_H;
  readonly frames: Array<{ bits: Uint8Array; full: boolean }> = [];
  asleep = false;

  busy(): boolean {
    return false;
  }

  show(bits: Uint8Array, full: boolean): void {
    this.asleep = false;
    this.frames.push({ bits: Uint8Array.from(bits), full });
  }

  sleep(): void {
    this.asleep = true;
  }

  last(): Uint8Array | null {
    return this.frames.length ? this.frames[this.frames.length - 1].bits : null;
  }
}
