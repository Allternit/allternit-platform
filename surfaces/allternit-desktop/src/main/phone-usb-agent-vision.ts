/**
 * Small image checks on mirrored frames: did the screen change, and where is
 * the pointer. Ported from esp32-iphone-agent's vision.py (MIT, Henry Yuan;
 * see hardware/usb-agent/NOTICE) to plain TypeScript over grayscale pixels,
 * so it runs in the Desktop main process with no image library. Decoding a
 * JPEG to grayscale is injected (Electron's nativeImage in production).
 */

export interface Gray {
  width: number;
  height: number;
  /** One byte per pixel, row-major. */
  data: Uint8Array;
}

/** Gray from a BGRA bitmap (Electron's nativeImage.toBitmap() layout). */
export function grayFromBgra(width: number, height: number, bgra: Uint8Array): Gray {
  const data = new Uint8Array(width * height);
  for (let i = 0, p = 0; i < data.length; i += 1, p += 4) {
    data[i] = (bgra[p + 2] * 77 + bgra[p + 1] * 150 + bgra[p] * 29) >> 8;
  }
  return { width, height, data };
}

/** Area-average downscale. */
export function resize(img: Gray, width: number, height: number): Gray {
  const out = new Uint8Array(width * height);
  const sx = img.width / width;
  const sy = img.height / height;
  for (let y = 0; y < height; y += 1) {
    const y0 = Math.floor(y * sy);
    const y1 = Math.max(y0 + 1, Math.floor((y + 1) * sy));
    for (let x = 0; x < width; x += 1) {
      const x0 = Math.floor(x * sx);
      const x1 = Math.max(x0 + 1, Math.floor((x + 1) * sx));
      let sum = 0;
      for (let yy = y0; yy < y1; yy += 1) for (let xx = x0; xx < x1; xx += 1) sum += img.data[yy * img.width + xx];
      out[y * width + x] = Math.round(sum / ((y1 - y0) * (x1 - x0)));
    }
  }
  return { width, height, data: out };
}

function absDiff(a: Gray, b: Gray): Uint8Array {
  const d = new Uint8Array(a.data.length);
  for (let i = 0; i < d.length; i += 1) d[i] = Math.abs(a.data[i] - b.data[i]);
  return d;
}

/**
 * True when the screen content changed, including small things such as a
 * checkmark. `ignore` lists points (fractions) where the pointer was or went;
 * those spots are masked so pointer moves never count. The status bar (clock)
 * is ignored.
 */
export function changed(before: Gray | null, after: Gray | null, ignore: Array<[number, number]> = [], threshold = 1.0): boolean {
  if (!before || !after) return true;
  if (before.width !== after.width || before.height !== after.height) return true;
  const small = absDiff(resize(before, 32, 64), resize(after, 32, 64));
  let total = 0;
  for (const v of small) total += v;
  if (total / small.length > threshold) return true;
  const w = Math.max(1, before.width >> 1);
  const h = Math.max(1, before.height >> 1);
  const diff = absDiff(resize(before, w, h), resize(after, w, h));
  const masks = ignore.map(([px, py]) => [px * w, py * h] as const);
  const r2 = (0.05 * w) ** 2;
  let hits = 0;
  for (let i = 0; i < diff.length; i += 1) {
    if (diff[i] <= 25) continue;
    const x = i % w;
    const y = Math.floor(i / w);
    if (y > 0.06 * h && masks.every(([mx, my]) => (x - mx) ** 2 + (y - my) ** 2 > r2)) hits += 1;
    if (hits >= 6) return true;
  }
  return false;
}

/**
 * Where the pointer is in `before`, given it moved right by `shift` (fraction
 * of width) in `after`. Only the pointer moved between the frames, so the
 * changed pixels near `near` are the pointer at both spots; their centroid
 * sits shift/2 right of the first. Null when unclear.
 */
export function findPointer(
  before: Gray,
  after: Gray,
  near: [number, number],
  shift: number,
  reach: [number, number] = [0.12, 0.08],
): [number, number] | null {
  if (before.width !== after.width || before.height !== after.height) return null;
  const { width: w, height: h } = before;
  const left = Math.max(0, Math.floor((near[0] - reach[0]) * w));
  const top = Math.max(Math.floor(0.05 * h), Math.floor((near[1] - reach[1]) * h)); // skip the status bar
  const right = Math.min(w, Math.floor((near[0] + reach[0]) * w));
  const bottom = Math.min(h, Math.floor((near[1] + reach[1]) * h));
  if (right <= left || bottom <= top) return null;
  let n = 0;
  let sx = 0;
  let sy = 0;
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (let y = top; y < bottom; y += 1) {
    for (let x = left; x < right; x += 1) {
      const i = y * w + x;
      if (Math.abs(before.data[i] - after.data[i]) <= 30) continue;
      n += 1;
      sx += x;
      sy += y;
      if (x < minX) minX = x;
      if (x > maxX) maxX = x;
      if (y < minY) minY = y;
      if (y > maxY) maxY = y;
    }
  }
  const area = (right - left) * (bottom - top);
  if (n < 15 || n > area / 4) return null; // nothing moved, or much more than a pointer
  if (maxX - minX > 0.15 * w || maxY - minY > 0.07 * h) return null; // not one compact spot
  return [sx / n / w - shift / 2, sy / n / h];
}
