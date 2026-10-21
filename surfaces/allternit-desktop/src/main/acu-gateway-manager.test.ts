import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { resolveAcuGatewaySpawn, resolveAcuPython, AcuGatewayManager } from './acu-gateway-manager.js';

vi.mock('node:child_process', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:child_process')>();
  // Only spawn is stubbed — execFileSync stays real so the resolve* helpers
  // behave exactly as they do outside tests.
  return { ...actual, spawn: vi.fn() };
});

function checkoutWithLaunch(): string {
  const repoRoot = mkdtempSync(join(tmpdir(), 'allternit-acu-'));
  const core = join(repoRoot, 'domains', 'computer-use', 'core');
  mkdirSync(core, { recursive: true });
  writeFileSync(join(core, 'launch.py'), '# launch');
  return repoRoot;
}

describe('resolveAcuGatewaySpawn', () => {
  it('resolves repo launch.py in development', () => {
    const repoRoot = checkoutWithLaunch();
    const spec = resolveAcuGatewaySpawn({ packaged: false, repoRoot, platform: 'darwin' });
    expect(spec).not.toBeNull();
    expect(spec!.args[0]).toBe(join(repoRoot, 'domains', 'computer-use', 'core', 'launch.py'));
    expect(spec!.cwd).toBe(join(repoRoot, 'domains', 'computer-use', 'core'));
    expect(spec!.extraEnv.ALLTERNIT_ACU_PORT).toBe('8760');
    expect(spec!.extraEnv.ALLTERNIT_VISION_PROVIDER).toBe('allternit');
    expect(spec!.extraEnv.ALLTERNIT_LOCAL_BRAIN_URL).toBe('http://127.0.0.1:4096');
  });

  it('returns null in dev when launch.py is missing', () => {
    const repoRoot = mkdtempSync(join(tmpdir(), 'allternit-acu-empty-'));
    expect(resolveAcuGatewaySpawn({ packaged: false, repoRoot, platform: 'darwin' })).toBeNull();
  });

  it('resolves staged launch.py when packaged', () => {
    const resourcesPath = mkdtempSync(join(tmpdir(), 'allternit-acu-res-'));
    const acu = join(resourcesPath, 'computer-use', 'acu');
    mkdirSync(acu, { recursive: true });
    writeFileSync(join(acu, 'launch.py'), '# launch');
    const spec = resolveAcuGatewaySpawn({
      packaged: true,
      resourcesPath,
      repoRoot: '/nonexistent',
      platform: 'darwin',
      pythonPath: '/usr/bin/python3',
    });
    expect(spec).not.toBeNull();
    expect(spec!.command).toBe('/usr/bin/python3');
    expect(spec!.args).toEqual([join(acu, 'launch.py')]);
  });
});

describe('resolveAcuPython', () => {
  it('uses the bundled driver Python when the gateway packages are staged', () => {
    const resourcesPath = mkdtempSync(join(tmpdir(), 'allternit-acu-bundled-'));
    mkdirSync(join(resourcesPath, 'computer-use', 'acu', 'site-packages', 'uvicorn'), { recursive: true });
    const bin = join(resourcesPath, 'computer-use', 'driver', 'python', 'bin');
    mkdirSync(bin, { recursive: true });
    writeFileSync(join(bin, 'python3'), '');
    expect(resolveAcuPython({ packaged: true, resourcesPath, repoRoot: '/nonexistent', platform: 'darwin' })).toBe(
      join(bin, 'python3'),
    );
  });


  it('prefers ALLTERNIT_ACU_PYTHON then an explicit pythonPath', () => {
    expect(
      resolveAcuPython({ packaged: false, repoRoot: '/tmp', pythonPath: '/opt/custom/python' }),
    ).toBe('/opt/custom/python');
  });
});

describe('AcuGatewayManager.start', () => {
  it('gives up immediately when the spawned child crashes (missing uvicorn)', async () => {
    const { EventEmitter } = await import('node:events');
    const { spawn } = await import('node:child_process');
    const fakeChild = new EventEmitter() as import('node:child_process').ChildProcess;
    (fakeChild as { stdout: InstanceType<typeof EventEmitter> }).stdout = new EventEmitter();
    (fakeChild as { stderr: InstanceType<typeof EventEmitter> }).stderr = new EventEmitter();
    (fakeChild as { kill: () => boolean }).kill = () => true;
    (fakeChild as { killed: boolean }).killed = false;
    vi.mocked(spawn).mockReturnValue(fakeChild);

    const manager = new AcuGatewayManager();
    manager.spawnContextOverride = {
      packaged: false,
      repoRoot: checkoutWithLaunch(),
      pythonPath: '/opt/custom/python',
    };
    // Nothing already listening on the ACU port.
    (manager as unknown as { fetchImpl: typeof fetch }).fetchImpl = (async () => {
      throw new Error('not up');
    }) as unknown as typeof fetch;

    const startedAt = Date.now();
    const resultPromise = manager.start();
    // The python interpreter dies right after spawn, the way the packaged
    // launch.py does when uvicorn is not importable.
    setTimeout(() => fakeChild.emit('exit', 1), 10);
    const result = await resultPromise;

    expect(result).toBeNull();
    // Regression guard: without the childDied fast-fail this burns the full
    // 20s HEALTH_TIMEOUT_MS on every boot.
    expect(Date.now() - startedAt).toBeLessThan(2_000);
  });
});
