import { execFileSync } from 'node:child_process';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({ app: { getPath: () => os.tmpdir() } }));
vi.mock('electron-log', () => ({ default: { info: vi.fn(), warn: vi.fn(), error: vi.fn() } }));

import { removeStaleDaemonFiles, socketAnswers } from './computer-use-driver-manager.js';

const dirs: string[] = [];
function tempDir(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cua-sock-'));
  dirs.push(dir);
  return dir;
}

afterEach(() => {
  for (const dir of dirs.splice(0)) fs.rmSync(dir, { recursive: true, force: true });
});

describe('cua-driver socket liveness', () => {
  it('a listening socket answers', async () => {
    const socketPath = path.join(tempDir(), 'live.sock');
    const server = net.createServer((c) => c.end());
    await new Promise<void>((resolve) => server.listen(socketPath, resolve));
    expect(await socketAnswers(socketPath)).toBe(true);
    await new Promise<void>((resolve) => server.close(() => resolve()));
  });

  it('a leftover socket file from a dead daemon does not, and is cleaned up', async () => {
    const dir = tempDir();
    const socketPath = path.join(dir, 'cua-driver.sock');
    // A process that bound the socket and died without cleaning up, like a
    // killed daemon: the file stays, nobody listens.
    execFileSync('python3', ['-c', 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])', socketPath]);
    fs.writeFileSync(path.join(dir, 'cua-driver.pid'), '12345');
    expect(fs.existsSync(socketPath)).toBe(true);
    expect(await socketAnswers(socketPath)).toBe(false);
    removeStaleDaemonFiles(socketPath);
    expect(fs.existsSync(socketPath)).toBe(false);
    expect(fs.existsSync(path.join(dir, 'cua-driver.pid'))).toBe(false);
  });

  it('a missing socket does not answer', async () => {
    expect(await socketAnswers(path.join(tempDir(), 'none.sock'))).toBe(false);
  });
});
