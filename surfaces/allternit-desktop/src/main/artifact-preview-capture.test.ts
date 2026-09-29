import { describe, expect, it } from 'vitest';
import { captureArtifactPreview, type CaptureWindow } from './artifact-preview-capture.js';

function fakeWindow(opts: { scrollHeight?: number; bytes?: number; empty?: boolean } = {}) {
  const log: string[] = [];
  let destroyed = false;
  let size = { width: 0, height: 0 };
  const win: CaptureWindow = {
    webContents: {
      on: (event) => log.push(`on:${event}`),
      setWindowOpenHandler: () => log.push('deny-open'),
      session: {
        setPermissionRequestHandler: (h) => h(null, 'camera', (g) => log.push(`perm:${g}`)),
        on: (event) => log.push(`session:${event}`),
      },
      executeJavaScript: async () => opts.scrollHeight ?? 0,
      capturePage: async () => ({
        isEmpty: () => Boolean(opts.empty),
        toPNG: () => Buffer.alloc(opts.bytes ?? 10, 1),
        toJPEG: () => Buffer.alloc(5, 2),
        getSize: () => size,
      }),
    },
    loadURL: async (url) => { log.push(url.slice(0, 30)); },
    setContentSize: (w, h) => { size = { width: w, height: h }; log.push(`size:${w}x${h}`); },
    isDestroyed: () => destroyed,
    destroy: () => { destroyed = true; log.push('destroyed'); },
  };
  return { win, log, create: (s: { width: number; height: number }) => { size = s; return win; } };
}

describe('captureArtifactPreview', () => {
  it('locks the window down, loads the page, captures a PNG, and closes', async () => {
    const f = fakeWindow();
    const r = await captureArtifactPreview({ html: '<h1>Hi</h1>' }, f.create, { settleMs: 0 });
    expect(r).toMatchObject({ ok: true, width: 1280, height: 800 });
    expect(r.image).toMatch(/^data:image\/png;base64,/);
    expect(f.log).toEqual(expect.arrayContaining(['on:will-navigate', 'deny-open', 'perm:false', 'session:will-download', 'destroyed']));
    expect(f.log.some((l) => l.startsWith('data:text/html;charset=utf-8;'))).toBe(true);
  });

  it('grows to the full page height, capped', async () => {
    const f = fakeWindow({ scrollHeight: 99999 });
    const r = await captureArtifactPreview({ html: '<p>x</p>', width: 1000, height: 700, fullPage: true }, f.create, { settleMs: 0 });
    expect(r).toMatchObject({ ok: true, width: 1000, height: 6000 });
  });

  it('switches to JPEG for a huge PNG and refuses empty or missing pages', async () => {
    const big = fakeWindow({ bytes: 4 * 1024 * 1024 });
    expect((await captureArtifactPreview({ html: '<p>x</p>' }, big.create, { settleMs: 0 })).image).toMatch(/^data:image\/jpeg;base64,/);
    expect((await captureArtifactPreview({ html: '<p>x</p>' }, fakeWindow({ empty: true }).create, { settleMs: 0 })).ok).toBe(false);
    expect((await captureArtifactPreview({ html: '  ' }, fakeWindow().create)).error).toMatch(/no HTML/);
  });
});
