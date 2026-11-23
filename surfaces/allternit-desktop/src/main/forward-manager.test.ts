import { describe, expect, it } from 'vitest';

// forward-manager registers IPC on import; stub electron so the pure helpers load.
import { vi } from 'vitest';
vi.mock('electron', () => ({ ipcMain: { handle: vi.fn() }, app: { on: vi.fn() }, BrowserWindow: { getAllWindows: () => [] } }));
vi.mock('electron-log', () => ({ default: { warn: vi.fn() } }));
vi.mock('./auth-manager.js', () => ({ authManager: { getSession: async () => null } }));

const { pickLocalPort, failureFromStatus } = await import('./forward-manager.js');

describe('local port choice', () => {
  it('slides to the next free port when the default is taken', async () => {
    const taken = new Set([3000, 3001]);
    expect(await pickLocalPort(async (p) => !taken.has(p), 3000, false)).toBe(3002);
  });
  it('takes a typed port exactly, or says it is busy', async () => {
    expect(await pickLocalPort(async () => false, 8080, true)).toBeNull();
    expect(await pickLocalPort(async () => true, 8080, true)).toBe(8080);
  });
});

describe('relay answers', () => {
  it('409 is asleep and 503 is offline, so a tunnel never wakes a computer', () => {
    expect(failureFromStatus(409, 'asleep')).toBe('asleep');
    expect(failureFromStatus(503)).toBe('offline');
    expect(failureFromStatus(200)).toBeNull();
  });
});
