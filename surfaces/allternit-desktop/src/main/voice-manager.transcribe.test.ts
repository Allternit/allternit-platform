import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({
  app: { isPackaged: false, getPath: () => '/tmp/allternit-voice-test', getAppPath: () => '/tmp' },
  BrowserWindow: { getAllWindows: () => [] },
  ipcMain: { handle: vi.fn() },
}));
vi.mock('electron-log', () => ({ default: { info: vi.fn(), warn: vi.fn(), error: vi.fn() } }));
vi.mock('./process-lifeline.js', () => ({ spawnSidecar: vi.fn() }));

import { voiceManager } from './voice-manager.js';

const wav = new Uint8Array([0x52, 0x49, 0x46, 0x46]);

function sidecar(status: number, body: unknown) {
  const fetchMock = vi.fn(async (url: string) => {
    if (String(url).endsWith('/health')) return new Response('{}', { status: 200 });
    return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('voice-manager transcribe (Desktop dictation -> POST /v1/stt)', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('posts the wav to the sidecar /v1/stt and returns the text', async () => {
    const fetchMock = sidecar(200, { text: ' hello world ' });
    const vm = voiceManager as unknown as { isHealthy: () => Promise<boolean> };
    vm.isHealthy = async () => true;
    await expect(voiceManager.transcribe(wav)).resolves.toEqual({ text: 'hello world' });
    const call = fetchMock.mock.calls.find(([u]) => String(u).endsWith('/v1/stt'));
    expect(call).toBeTruthy();
  });

  it('first use: passes the "voice pack downloading" message to the UI, not a generic failure', async () => {
    sidecar(503, {
      error: 'Downloading the voice pack (39 MB)… try again in a moment.',
      code: 'voice_pack_downloading',
      pack: 'small',
    });
    const vm = voiceManager as unknown as { isHealthy: () => Promise<boolean> };
    vm.isHealthy = async () => true;
    const result = await voiceManager.transcribe(wav);
    expect(result.status).toBe('downloading');
    expect(result.error).toBe('Downloading the voice pack (39 MB)… try again in a moment.');
    expect(result.error).not.toMatch(/HTTP 503/);
  });

  it('other sidecar failures still report the HTTP status and detail', async () => {
    sidecar(503, { error: 'model failed to load' });
    const vm = voiceManager as unknown as { isHealthy: () => Promise<boolean> };
    vm.isHealthy = async () => true;
    const result = await voiceManager.transcribe(wav);
    expect(result.error).toBe('Voice sidecar HTTP 503: model failed to load');
    expect(result.status).toBeUndefined();
  });
});
