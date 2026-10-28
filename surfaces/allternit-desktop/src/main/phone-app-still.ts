/**
 * phone.screenshot for iPhones with the Allternit app: the screen arrives as
 * H.264 (phone-app-store.ts keeps the last keyframe and the frames after it),
 * and Node has no video decoder. Chromium does: a hidden, sandboxed window
 * with its own in-memory session runs WebCodecs' VideoDecoder on the GOP
 * (Annex B, no avcC description), draws the last picture upright (the same
 * turn the live view uses, allternit-ai rotation.ts) and hands back a JPEG.
 * One window per still, destroyed right after.
 */

import { BrowserWindow } from 'electron';
import type { StillDecoder, StillInput } from './phone-app-store.js';

const DECODE_TIMEOUT_MS = 15_000;
const JPEG_QUALITY = 0.8;

/** Runs in the hidden window. Kept as plain source text: it never touches Node. */
const DECODE_SOURCE = `async (codec, units, rotation, quality) => {
  if (typeof VideoDecoder === 'undefined') throw new Error('This computer can not decode video here.');
  const support = await VideoDecoder.isConfigSupported({ codec });
  if (!support.supported) throw new Error('The iPhone video format is not supported (' + codec + ').');
  let last = null;
  let failure = null;
  const decoder = new VideoDecoder({
    output: (frame) => { if (last) last.close(); last = frame; },
    error: (e) => { failure = e; },
  });
  decoder.configure({ codec, optimizeForLatency: true });
  units.forEach((b64, i) => {
    const bin = atob(b64);
    const data = new Uint8Array(bin.length);
    for (let j = 0; j < bin.length; j++) data[j] = bin.charCodeAt(j);
    decoder.decode(new EncodedVideoChunk({ type: i === 0 ? 'key' : 'delta', timestamp: i * 33333, data }));
  });
  await decoder.flush();
  decoder.close();
  if (!last) throw new Error(failure ? String(failure.message || failure) : 'No picture came out of the video.');
  const w = last.displayWidth, h = last.displayHeight;
  const side = rotation === 90 || rotation === 270;
  const canvas = new OffscreenCanvas(side ? h : w, side ? w : h);
  const ctx = canvas.getContext('2d');
  const m = rotation === 90 ? [0, -1, 1, 0, 0, w] : rotation === 180 ? [-1, 0, 0, -1, w, h] : rotation === 270 ? [0, 1, -1, 0, h, 0] : [1, 0, 0, 1, 0, 0];
  ctx.setTransform(m[0], m[1], m[2], m[3], m[4], m[5]);
  ctx.drawImage(last, 0, 0);
  last.close();
  const blob = await canvas.convertToBlob({ type: 'image/jpeg', quality });
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  return { jpeg: btoa(s), width: canvas.width, height: canvas.height };
}`;

export const decodeStillInWindow: StillDecoder = async (input: StillInput) => {
  const win = new BrowserWindow({
    show: false,
    width: 64,
    height: 64,
    webPreferences: {
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      partition: `phone-app-still-${Date.now()}`,
      backgroundThrottling: false,
    },
  });
  try {
    win.webContents.on('will-navigate', (e) => e.preventDefault());
    win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    win.webContents.session.setPermissionRequestHandler((_wc, _perm, cb) => cb(false));
    await win.loadURL('about:blank');
    const args = [input.codec, input.units.map((u) => u.toString('base64')), input.rotation, JPEG_QUALITY];
    const run = win.webContents.executeJavaScript(`(${DECODE_SOURCE})(...${JSON.stringify(args)})`) as Promise<{ jpeg: string; width: number; height: number }>;
    const timeout = new Promise<never>((_, reject) => setTimeout(() => reject(new Error('decoding took too long')), DECODE_TIMEOUT_MS));
    const out = await Promise.race([run, timeout]);
    return { jpeg: Buffer.from(out.jpeg, 'base64'), width: out.width, height: out.height };
  } finally {
    if (!win.isDestroyed()) win.destroy();
  }
};
