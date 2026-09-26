import { describe, it, expect } from 'vitest';
import { spawn, execFileSync } from 'node:child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'node:url';
import { spawnSidecar } from './process-lifeline.js';

const posix = process.platform !== 'win32';
const lifelineModule = path.join(path.dirname(fileURLToPath(import.meta.url)), 'process-lifeline.ts');

function groupMembers(pgid: number): string[] {
  try {
    return execFileSync('ps', ['-o', 'comm=', '-g', String(pgid)], { encoding: 'utf8' })
      .split('\n').map((s) => s.trim()).filter(Boolean);
  } catch {
    return [];
  }
}

async function until(check: () => boolean, timeoutMs = 3000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (check()) return true;
    await new Promise((r) => setTimeout(r, 50));
  }
  return check();
}

describe.runIf(posix)('spawnSidecar', () => {
  it('keeps the real pid and reports the real exit signal', async () => {
    const child = spawnSidecar('sleep', ['30'], { stdio: ['ignore', 'pipe', 'pipe'] });
    await until(() => groupMembers(child.pid!).includes('sleep'));
    expect(execFileSync('ps', ['-o', 'comm=', '-p', String(child.pid)], { encoding: 'utf8' }).trim()).toBe('sleep');
    const exited = new Promise<[number | null, NodeJS.Signals | null]>((resolve) =>
      child.once('exit', (code, signal) => resolve([code, signal])));
    child.kill('SIGTERM');
    expect(await exited).toEqual([null, 'SIGTERM']);
  });

  it('sweeps grandchildren a start script leaves when only the leader is killed', async () => {
    const child = spawnSidecar('bash', ['-c', 'sleep 30 & echo $!; wait'], { stdio: ['ignore', 'pipe', 'pipe'] });
    const grandchild = await new Promise<number>((resolve) =>
      child.stdout!.once('data', (d: Buffer) => resolve(Number(d.toString().trim()))));
    const alive = () => { try { process.kill(grandchild, 0); return true; } catch { return false; } };
    expect(alive()).toBe(true);
    child.kill('SIGTERM');
    expect(await until(() => !alive())).toBe(true);
  });

  it('takes the sidecar down when the parent dies without cleanup', async () => {
    const pidFile = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'lifeline-')), 'pid');
    const parent = spawn(process.execPath, ['--input-type=module', '-e', `
      const { spawnSidecar } = await import(${JSON.stringify(lifelineModule)});
      const { writeFileSync } = await import('fs');
      const c = spawnSidecar('sleep', ['30'], { stdio: ['ignore', 'ignore', 'ignore'] });
      writeFileSync(${JSON.stringify(pidFile)}, String(c.pid));
      setInterval(() => {}, 1000);
    `], { stdio: 'ignore' });
    expect(await until(() => fs.existsSync(pidFile) && fs.readFileSync(pidFile, 'utf8') !== '', 5000)).toBe(true);
    const sidecarPid = Number(fs.readFileSync(pidFile, 'utf8'));
    await until(() => groupMembers(sidecarPid).includes('sleep'));

    parent.kill('SIGKILL');

    const alive = () => { try { process.kill(sidecarPid, 0); return true; } catch { return false; } };
    expect(await until(() => !alive())).toBe(true);
  });

  it('leaves the child\'s own stdin to the caller (MCP stdio protocol)', async () => {
    const child = spawnSidecar('cat', [], { stdio: ['pipe', 'pipe', 'pipe'] });
    const echoed = new Promise<string>((resolve) => child.stdout!.once('data', (d: Buffer) => resolve(d.toString())));
    child.stdin!.write('ping\n');
    expect(await echoed).toBe('ping\n');
    const exited = new Promise((resolve) => child.once('exit', resolve));
    child.stdin!.end();
    await exited;
  });

  it('keeps spawn()\'s native ENOENT for a missing absolute binary', async () => {
    const child = spawnSidecar('/nonexistent/allternit-sidecar', []);
    const err = await new Promise<NodeJS.ErrnoException>((resolve) => child.once('error', resolve));
    expect(err.code).toBe('ENOENT');
  });
});
