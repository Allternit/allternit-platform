import { describe, it, expect } from 'vitest';
import { spawn, execFileSync } from 'node:child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { spawnSidecar } from './process-lifeline.js';

const posix = process.platform !== 'win32';
const lifelineSource = path.join(path.dirname(fileURLToPath(import.meta.url)), 'process-lifeline.ts');

/** The helper as plain ESM, loadable by a bare `node` child (CI runs Node 20, no TS). */
function transpiledLifeline(dir: string): string {
  const out = path.join(dir, 'process-lifeline.mjs');
  const { outputText } = ts.transpileModule(fs.readFileSync(lifelineSource, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  });
  fs.writeFileSync(out, outputText);
  return out;
}

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
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lifeline-'));
    const lifelineModule = transpiledLifeline(dir);
    const pidFile = path.join(dir, 'pid');
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
  }, 15_000);

  it('takes the sidecar down when the parent closes the pipe during quit, then exits', async () => {
    // Electron closes its fds during quit a moment before it exits. The
    // watcher sees EOF with the parent still alive; it must wait for the
    // exit, not give up (that leaked every sidecar on a normal quit).
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'lifeline-'));
    const lifelineModule = transpiledLifeline(dir);
    const pidFile = path.join(dir, 'pid');
    const parent = spawn(process.execPath, ['--input-type=module', '-e', `
      const { spawnSidecar } = await import(${JSON.stringify(lifelineModule)});
      const { writeFileSync } = await import('fs');
      const c = spawnSidecar('sleep', ['30'], { stdio: ['ignore', 'ignore', 'ignore'] });
      writeFileSync(${JSON.stringify(pidFile)}, String(c.pid));
      setTimeout(() => c.stdio[3].destroy(), 300);
      setTimeout(() => process.exit(0), 2500);
      setInterval(() => {}, 1000);
    `], { stdio: 'ignore' });
    expect(await until(() => fs.existsSync(pidFile) && fs.readFileSync(pidFile, 'utf8') !== '', 5000)).toBe(true);
    const sidecarPid = Number(fs.readFileSync(pidFile, 'utf8'));
    const alive = () => { try { process.kill(sidecarPid, 0); return true; } catch { return false; } };

    // Pipe closed, parent still running: the sidecar stays up.
    await new Promise((r) => setTimeout(r, 1500));
    expect(alive()).toBe(true);

    // Parent exits: the sidecar goes too.
    await new Promise((resolve) => parent.once('exit', resolve));
    expect(await until(() => !alive(), 8000)).toBe(true);
  }, 20_000);

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
