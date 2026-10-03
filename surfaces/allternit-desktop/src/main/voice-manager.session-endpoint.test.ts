import { describe, expect, it, vi } from 'vitest';

const handlers = vi.hoisted(() => new Map<string, (...args: unknown[]) => unknown>());
const spawned = vi.hoisted(() => ({ env: null as Record<string, string> | null }));

vi.mock('electron', () => ({
  app: { isPackaged: false, getPath: () => '/tmp/allternit-voice-test', getAppPath: () => '/tmp' },
  BrowserWindow: { getAllWindows: () => [] },
  ipcMain: { handle: (channel: string, fn: (...args: unknown[]) => unknown) => handlers.set(channel, fn) },
}));
vi.mock('electron-log', () => ({ default: { info: vi.fn(), warn: vi.fn(), error: vi.fn() } }));
vi.mock('./process-lifeline.js', () => ({
  spawnSidecar: (_file: string, _args: string[], opts: { env: Record<string, string> }) => {
    spawned.env = opts.env;
    return { stdout: { on: vi.fn() }, stderr: { on: vi.fn() }, on: vi.fn(), kill: vi.fn() };
  },
}));

import { voiceManager } from './voice-manager.js';

describe('voice session endpoint bridge', () => {
  it('serves the loopback Voice Session URL over IPC, without a token for a sidecar this run did not spawn', async () => {
    voiceManager.registerIpcHandlers();
    const endpoint = await handlers.get('voice:session-endpoint')!();
    expect(endpoint).toEqual({
      httpUrl: 'http://127.0.0.1:8001',
      wsUrl: 'ws://127.0.0.1:8001/v1/voice/session',
      port: 8001,
    });
  });

  it('passes a per-run token to the sidecar it spawns and hands the same token to the renderer', async () => {
    const vm = voiceManager as unknown as {
      resolveCommand: () => { file: string; args: string[] };
      isHealthy: () => Promise<boolean>;
    };
    vm.resolveCommand = () => ({ file: '/bin/voice', args: [] });
    let healthy = false;
    vm.isHealthy = async () => { const was = healthy; healthy = true; return was; };
    await voiceManager.start().catch(() => undefined);
    const token = spawned.env?.ALLTERNIT_VOICE_TOKEN;
    expect(token).toMatch(/^[A-Za-z0-9_-]{40,}$/);
    expect(voiceManager.getSessionEndpoint().token).toBe(token);
  });
});
