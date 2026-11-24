import { createHash } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const provider = vi.hoisted(() => ({
  approval: 1, submissions: 0, privatePartition: '', phase: 0,
  saved: {} as Record<string, unknown>,
  downloaded: { base64: '', bytes: 0, mime: 'image/png' },
  decoded: { width: 512, height: 512 },
}));
vi.mock('node:fs/promises', () => ({ mkdir: vi.fn(async () => {}), writeFile: vi.fn(async () => {}), readFile: vi.fn(), stat: vi.fn() }));
vi.mock('electron-store', () => ({ default: class {
  get() { return provider.saved; }
  set(_key: string, value: Record<string, unknown>) { provider.saved = value; }
} }));
vi.mock('electron', () => {
  class Window {
    static fromWebContents() { return { isVisible: () => true }; }
    constructor(options: { webPreferences: { partition: string } }) { provider.privatePartition = options.webPreferences.partition; }
    isDestroyed() { return false; }
    show() {} hide() {} destroy() {} on() {}
    async loadURL() {}
    focus() {} once() {}
    webContents = {
      getURL: () => 'https://chatgpt.com/c/current', setWindowOpenHandler() {}, stop() {}, on() {},
      async executeJavaScript(script: string) {
        if (script.includes('const controls')) return 'selected';
        if (script.includes('const visible')) {
          return provider.phase ? { ready: true, user: 'Create an image of a cat', users: 1, turns: 1, image: 'https://chatgpt.com/generated.png', width: 512, height: 512 }
            : { ready: true, users: 0, turns: 0 };
        }
        if (script.includes('e.closest')) return true;
        if (script.includes("editor.focus()")) return true;
        if (script.includes('button.click()')) { provider.submissions++; provider.phase = 1; return true; }
        if (script.includes('const response = await fetch')) return provider.downloaded;
        return undefined;
      },
    };
  }
  return { app: { on() {}, isReady: () => true, getPath: () => '/scratch/aci-test' }, BrowserWindow: Window,
    powerMonitor: { on() {}, removeListener() {} },
    dialog: { showMessageBox: async () => ({ response: provider.approval }) },
    session: { fromPartition: () => ({ clearStorageData: async () => {} }) },
    nativeImage: { createFromBuffer: () => ({ getSize: () => provider.decoded, isEmpty: () => false,
      toPNG: () => Buffer.from(provider.downloaded.base64, 'base64'), resize: () => ({ toDataURL: () => 'data:image/png;base64,aQ==' }) }) },
  };
});
const owner = { isDestroyed: () => false, once() {}, removeListener() {}, send() {} };
const attemptKey = createHash('sha256').update('user-1\0image-1').digest('hex');
const request = { requestId: 'image-1', sessionId: 'ses-1', prompt: 'Create an image of a cat', timeoutMs: 10000 };
async function run() {
  const { runAciImage } = await import('./aci-image.js');
  const promise = runAciImage(owner as never, request, () => 'user-1');
  await vi.advanceTimersByTimeAsync(4100);
  return promise;
}
beforeEach(() => {
  vi.resetModules(); vi.useFakeTimers(); provider.approval = 1; provider.phase = 0; provider.saved = {};
  provider.submissions = 0; provider.decoded = { width: 512, height: 512 };
  // Header is enough for the native-codec boundary stub; real decoder acceptance is train/device validation.
  const png = Buffer.alloc(33); Buffer.from([137,80,78,71,13,10,26,10]).copy(png); png.writeUInt32BE(13, 8); png.write('IHDR', 12); png.writeUInt32BE(512, 16); png.writeUInt32BE(512, 20);
  provider.downloaded = { base64: png.toString('base64'), bytes: png.length, mime: 'image/png' };
});
afterEach(() => { vi.useRealTimers(); });

describe('local ACI image executor', () => {
  it('a declined exact-prompt confirmation never submits or records an attempt', async () => {
    provider.approval = 0;
    expect(await run()).toMatchObject({ ok: false, code: 'cancelled' });
    expect(provider.submissions).toBe(0);
    expect(provider.saved).toEqual({});
  });
  it('returns downloaded PNG bytes from the private ACI session and rejects a replay', async () => {
    expect(await run()).toMatchObject({ ok: true, width: 512, height: 512, image: `data:image/png;base64,${provider.downloaded.base64}` });
    expect(provider.privatePartition).toBe('allternit-aci-private');
    expect(provider.saved[attemptKey]).toMatchObject({ sessionId: 'ses-1', phase: 'complete' });
    expect(await run()).toMatchObject({ ok: false, code: 'ambiguous_submission' });
    expect(provider.submissions).toBe(1);
  });
  it('does not resubmit an ambiguous persisted job after a process restart', async () => {
    provider.saved[attemptKey] = { phase: 'submission_pending', sessionId: 'ses-1' };
    expect(await run()).toMatchObject({ ok: false, code: 'ambiguous_submission' });
    expect(provider.submissions).toBe(0);
  });
  it('rejects an image whose actual decoded dimensions exceed the limit', async () => {
    provider.decoded = { width: 5000, height: 512 };
    expect(await run()).toMatchObject({ ok: false, code: 'invalid_image' });
  });
  it('rejects a MIME/bytes mismatch instead of treating it as a generated image', async () => {
    provider.downloaded.mime = 'image/jpeg';
    expect(await run()).toMatchObject({ ok: false, code: 'invalid_image' });
  });
  it('rejects oversized prompts before opening a confirmation', async () => {
    const { runAciImage } = await import('./aci-image.js');
    expect(await runAciImage(owner as never, { ...request, prompt: 'x'.repeat(16001) }, () => 'user-1')).toMatchObject({ ok: false, code: 'invalid_request' });
  });
});

describe('header dimensions before native allocation', () => {
  it('rejects truncated and mismatched formats', async () => {
    const { imageDimensions } = await import('./aci-image.js');
    expect(imageDimensions(Buffer.alloc(24), 'image/png')).toBeUndefined();
    expect(imageDimensions(Buffer.from(provider.downloaded.base64, 'base64'), 'image/jpeg')).toBeUndefined();
  });
  it('reads JPEG dimensions without decoding image pixels', async () => {
    const { imageDimensions } = await import('./aci-image.js');
    const jpeg = Buffer.from([255,216,255,192,0,11,8,0x20,0,0x20,0,1,1,0x11,0]);
    expect(imageDimensions(jpeg, 'image/jpeg')).toEqual({ width: 8192, height: 8192 });
  });
  it('refuses WebP extended canvases and malformed chunks', async () => {
    const { imageDimensions } = await import('./aci-image.js');
    const webp = Buffer.alloc(30); webp.write('RIFF'); webp.writeUInt32LE(22,4); webp.write('WEBPVP8X',8); webp.writeUInt32LE(10,16);
    expect(imageDimensions(webp, 'image/webp')).toBeUndefined();
  });
});
