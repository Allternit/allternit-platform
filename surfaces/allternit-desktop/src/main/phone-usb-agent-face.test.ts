import { execFileSync } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { FACE_BYTES, FACE_H, FACE_W, KEEP_OUT, PROG_SQUARE_X, PROG_SQUARE_Y, pixel, renderFace, toPbm } from './phone-usb-agent-face.js';
import { FACE_NAMES, type FaceName } from './phone-usb-agent-protocol.js';

const firmware = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../../hardware/usb-agent/firmware');
let dir = '';
let dump = '';

beforeAll(() => {
  // The firmware's face.h, compiled for this computer. If the C++ and the
  // TypeScript renderers ever disagree by one pixel, the comparison fails.
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'face-dump-'));
  dump = path.join(dir, 'face_dump');
  execFileSync('c++', ['-std=c++17', '-Wall', '-Werror', '-O1', '-o', dump, path.join(firmware, 'host/face_dump.cpp')], { stdio: 'pipe' });
});

afterAll(() => {
  if (dir) fs.rmSync(dir, { recursive: true, force: true });
});

const cpp = (face: FaceName, prog: number, armed: boolean) =>
  Buffer.from(execFileSync(dump, [face, String(prog), armed ? '1' : '0'], { encoding: 'utf8' }).trim(), 'hex');

const blackIn = (bits: Uint8Array, x0: number, y0: number, w: number, h: number) => {
  let n = 0;
  for (let y = y0; y < y0 + h; y += 1) for (let x = x0; x < x0 + w; x += 1) n += pixel(bits, x, y);
  return n;
};

describe('Link face renderer', () => {
  it('the firmware C++ and the Desktop TypeScript make the same bytes for every face, prog 0 to 5, armed and not', () => {
    let compared = 0;
    for (const face of FACE_NAMES) {
      for (let prog = 0; prog <= 5; prog += 1) {
        for (const armed of [false, true]) {
          const ts = renderFace(face, prog, armed);
          const c = cpp(face, prog, armed);
          expect(c.length).toBe(FACE_BYTES);
          expect(Buffer.from(ts).equals(c), `${face} prog=${prog} armed=${armed}`).toBe(true);
          compared += 1;
        }
      }
    }
    expect(compared).toBe(60);
  });

  it('keeps the A:// keep-out area white on every frame', () => {
    const [kx, ky, kw, kh] = KEEP_OUT;
    expect([kx, ky, kx + kw - 1, ky + kh - 1]).toEqual([60, 118, 139, 152]);
    for (const face of FACE_NAMES) {
      for (let prog = 0; prog <= 5; prog += 1) {
        expect(blackIn(renderFace(face, prog, true), kx, ky, kw, kh)).toBe(0);
      }
    }
  });

  it('prog 3 on working draws three solid squares and two outlines', () => {
    const bits = renderFace('working', 3, false);
    for (let i = 0; i < 5; i += 1) {
      const x = PROG_SQUARE_X[i];
      const filled = blackIn(bits, x, PROG_SQUARE_Y, 20, 20);
      // Solid: 400 pixels. Outline 2 px wide: 400 - 16 * 16 = 144.
      expect(filled, `square ${i + 1}`).toBe(i < 3 ? 400 : 144);
    }
    // The row is blank when prog is 0 and the face is not `working`.
    expect(blackIn(renderFace('default', 0, false), 0, PROG_SQUARE_Y, FACE_W, 20)).toBe(0);
    expect(blackIn(renderFace('working', 0, false), 0, PROG_SQUARE_Y, FACE_W, 20)).toBe(5 * 144);
  });

  it('draws two square eyes and nothing else on default; the armed mark only while armed', () => {
    const idle = renderFace('default', 0, false);
    expect(blackIn(idle, 0, 0, FACE_W, FACE_H)).toBe(2 * 36 * 36);
    expect(blackIn(idle, 46, 62, 36, 36)).toBe(36 * 36);
    expect(blackIn(idle, 118, 62, 36, 36)).toBe(36 * 36);
    const armed = renderFace('default', 0, true);
    expect(blackIn(armed, 184, 8, 8, 8)).toBe(64);
    expect(blackIn(armed, 0, 0, FACE_W, FACE_H)).toBe(2 * 36 * 36 + 64);
    // needs_you adds the exclamation mark above the eyes.
    expect(blackIn(renderFace('needs_you', 0, false), 96, 0, 8, 60)).toBe(8 * 28 + 8 * 8);
  });

  it('writes a plain PBM for review', () => {
    const pbm = toPbm(renderFace('wink', 0, false)).split('\n');
    expect(pbm.slice(0, 2)).toEqual(['P1', '200 200']);
    expect(pbm[2 + 80].slice(118, 154)).toBe('1'.repeat(36)); // the wink bar, row 80
    expect(pbm[2 + 70].slice(118, 154)).toBe('0'.repeat(36)); // above the bar: white
  });
});
