import { EventEmitter } from 'node:events';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  LAYA_PINNED_REVISION,
  LAYA_VERSION,
  SystemOneManager,
  SystemOneProgress,
  UV_INSTALL_HINT,
  type SystemOneDeps,
} from './system-one-manager.js';

class FakeChild extends EventEmitter {
  stdout = new EventEmitter();
  stderr = new EventEmitter();
  kill = vi.fn(() => true);
}

interface Harness {
  manager: SystemOneManager;
  home: string;
  root: string;
  resources: string;
  uv: string;
  spawn: ReturnType<typeof vi.fn<any[], FakeChild>>;
  spawnSidecar: ReturnType<typeof vi.fn<any[], FakeChild>>;
  children: FakeChild[];
  progress: SystemOneProgress[];
  healthy: { s1: boolean; laya: boolean };
}

let tmp: string;

function touch(file: string, content = ''): void {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
}

function harness(opts: { uv?: boolean; scripts?: boolean; binary?: boolean; env?: NodeJS.ProcessEnv; platform?: NodeJS.Platform } = {}): Harness {
  const home = path.join(tmp, 'home');
  const resources = path.join(tmp, 'Resources');
  const uv = path.join(tmp, 'tools', 'uv');
  if (opts.uv !== false) touch(uv);
  if (opts.scripts !== false) {
    touch(path.join(resources, 'laya', 'serve-laya.sh'));
    touch(path.join(resources, 'laya', 'install-laya.sh'));
  }
  if (opts.binary !== false) touch(path.join(resources, 'bin', 'system-one'));
  const healthy = { s1: false, laya: false };
  const children: FakeChild[] = [];
  const progress: SystemOneProgress[] = [];
  const mkChild = () => {
    const c = new FakeChild();
    children.push(c);
    return c;
  };
  const spawn = vi.fn<any[], FakeChild>(mkChild);
  const spawnSidecar = vi.fn<any[], FakeChild>(mkChild);
  const fetchMock = vi.fn(async (url: string) => {
    if (url.endsWith('/healthz')) {
      if (!healthy.s1) throw new Error('ECONNREFUSED');
      return new Response(JSON.stringify({ ok: true }), { status: 200 });
    }
    if (url.endsWith('/health')) {
      if (!healthy.laya) throw new Error('ECONNREFUSED');
      return new Response(JSON.stringify({ status: 'ok' }), { status: 200 });
    }
    throw new Error(`unexpected ${url}`);
  });
  const deps: SystemOneDeps = {
    spawn: spawn as unknown as SystemOneDeps['spawn'],
    spawnSidecar: spawnSidecar as unknown as SystemOneDeps['spawnSidecar'],
    fetch: fetchMock as unknown as SystemOneDeps['fetch'],
    env: { PATH: '/nonexistent', ...(opts.uv !== false ? { UV: uv } : {}), ...(opts.env ?? {}) },
    platform: opts.platform ?? 'darwin',
    homedir: home,
    isPackaged: true,
    resourcesPath: resources,
    appPath: path.join(resources, 'app.asar'),
    emitProgress: (p) => progress.push(p),
    sleep: () => new Promise((r) => setImmediate(r)),
  };
  const manager = new SystemOneManager(deps);
  return {
    manager,
    home,
    root: path.join(home, 'Library', 'Application Support', 'Allternit', 'laya'),
    resources,
    uv,
    spawn,
    spawnSidecar,
    children,
    progress,
    healthy,
  };
}

function installLaya(root: string, marker = true): void {
  touch(path.join(root, '.venv', 'bin', 'python'));
  if (marker) touch(path.join(root, 'laya.version'), `${LAYA_VERSION}\n`);
}

const flush = () => new Promise((r) => setImmediate(r));

beforeEach(() => {
  tmp = fs.mkdtempSync(path.join(os.tmpdir(), 's1-manager-'));
});

afterEach(() => {
  fs.rmSync(tmp, { recursive: true, force: true });
});

describe('allternit-api environment', () => {
  it('points the API at local S1 and leaves the backend on auto (no restart when Laya turns healthy)', async () => {
    const h = harness();
    expect(h.manager.getApiEnvironment()).toEqual({
      ALLTERNIT_S1_URL: 'http://127.0.0.1:7717',
      SYSTEM_ONE_LAYA_URL: 'http://127.0.0.1:7718',
      ALLTERNIT_S1_SHADOW_DIR: path.join(h.home, '.allternit', 'system-one', 'shadow'),
    });
    h.healthy.laya = true;
    await h.manager.checkLayaHealth();
    expect(h.manager.getApiEnvironment()).not.toHaveProperty('ALLTERNIT_S1_BACKEND');
    expect((await h.manager.getStatus()).apiBackend).toBe('auto');
  });

  it('lets explicit env exports win and honours the shadow-ledger opt-out (Q28)', () => {
    const h = harness({ env: { ALLTERNIT_S1_BACKEND: 'system_one_local', ALLTERNIT_S1_URL: 'http://127.0.0.1:9999', SYSTEM_ONE_SHADOW_LOG: '0' } });
    const env = h.manager.getApiEnvironment();
    expect(env.ALLTERNIT_S1_URL).toBe('http://127.0.0.1:9999');
    expect(env.ALLTERNIT_S1_BACKEND).toBe('system_one_local');
    expect(env).not.toHaveProperty('ALLTERNIT_S1_SHADOW_DIR');
  });
});

describe('Laya install state', () => {
  it('detects an install by marker or by the pinned dist-info', () => {
    const h = harness();
    expect(h.manager.isLayaInstalled()).toBe(false);
    installLaya(h.root, false);
    expect(h.manager.isLayaInstalled()).toBe(false);
    fs.mkdirSync(path.join(h.root, '.venv', 'lib', 'python3.11', 'site-packages', `laya-${LAYA_VERSION}.dist-info`), { recursive: true });
    expect(h.manager.isLayaInstalled()).toBe(true);
  });

  it('reports needs uv with the install hint when uv is missing', async () => {
    const h = harness({ uv: false });
    await expect(h.manager.install()).rejects.toThrow('needs uv');
    expect(h.spawn).not.toHaveBeenCalled();
    expect(h.progress).toEqual([{ stage: 'needs-uv', message: UV_INSTALL_HINT }]);
    const status = await h.manager.getStatus();
    expect(status.laya.needsUv).toBe(true);
    expect(status.laya.uvHint).toBe(UV_INSTALL_HINT);
  });

  it('runs install-laya.sh with LAYA_HOME and UV and streams progress', async () => {
    const h = harness();
    const done = h.manager.install();
    expect(h.manager.install()).toBe(done); // concurrent callers share one install
    expect(h.spawn).toHaveBeenCalledTimes(1);
    const [cmd, args, options] = h.spawn.mock.calls[0];
    expect(cmd).toBe('bash');
    expect(args).toEqual([path.join(h.resources, 'laya', 'install-laya.sh')]);
    expect(options.env.LAYA_HOME).toBe(h.root);
    expect(options.env.UV).toBe(h.uv);
    expect((await h.manager.getStatus()).laya.installing).toBe(true);
    h.children[0].stdout.emit('data', Buffer.from('installing laya[serve]==0.3.22\n'));
    h.children[0].emit('exit', 0, null);
    await done;
    expect(h.progress.map((p) => p.stage)).toEqual(['starting', 'installing', 'ready']);
  });

  it('maps installer exit 3 to needs uv and other failures to an error', async () => {
    const h = harness();
    const a = h.manager.install();
    h.children[0].emit('exit', 3, null);
    await expect(a).rejects.toThrow('needs uv');
    const b = h.manager.install();
    h.children[1].stderr.emit('data', Buffer.from('pip: no matching distribution\n'));
    h.children[1].emit('exit', 1, null);
    await expect(b).rejects.toThrow('code 1): pip: no matching distribution');
    expect((await h.manager.getStatus()).error).toContain('Laya installer failed');
  });

  it('cancels an install in progress', async () => {
    const h = harness();
    const p = h.manager.install();
    expect(h.manager.cancelInstall()).toBe(true);
    expect(h.children[0].kill).toHaveBeenCalledWith('SIGTERM');
    h.children[0].emit('exit', null, 'SIGTERM');
    await expect(p).rejects.toThrow('cancelled');
  });
});

describe('checkpoint config', () => {
  it('pins the Q29 base revision by default', () => {
    expect(harness().manager.getCheckpoint()).toEqual({ source: 'pinned', revision: LAYA_PINNED_REVISION });
  });

  it('swaps in a revision or a local path through config.json, env winning', async () => {
    const h = harness();
    await h.manager.setCheckpoint({ revision: 'abc1234' });
    expect(h.manager.getCheckpoint()).toEqual({ source: 'revision', revision: 'abc1234' });
    await h.manager.setCheckpoint({ path: '/models/laya-ft' });
    expect(h.manager.getCheckpoint()).toEqual({ source: 'path', path: '/models/laya-ft' });
    await expect(h.manager.setCheckpoint({ path: 'relative/dir' })).rejects.toThrow('absolute');
    await h.manager.setCheckpoint(null);
    expect(h.manager.getCheckpoint().source).toBe('pinned');
    const e = harness({ env: { ALLTERNIT_LAYA_REVISION: 'deadbeef' } });
    expect(e.manager.getCheckpoint()).toEqual({ source: 'revision', revision: 'deadbeef' });
  });
});

describe('raw decision state opt-in (Q28)', () => {
  const s1Calls = (h: Harness) => h.spawnSidecar.mock.calls.filter(([cmd]) => String(cmd).endsWith('system-one'));

  it('is off by default and keeps SYSTEM_ONE_SHADOW_STATE out of the S1 env', async () => {
    const h = harness();
    expect(h.manager.shadowState).toBe(false);
    const p = h.manager.startSystemOne();
    await flush();
    expect(s1Calls(h)[0][2].env).not.toHaveProperty('SYSTEM_ONE_SHADOW_STATE');
    h.healthy.s1 = true;
    await p;
    expect((await h.manager.getStatus()).shadowState).toBe(false);
  });

  it('persists outside the Laya root and restarts the owned S1 server with it', async () => {
    const h = harness();
    const p = h.manager.startSystemOne();
    await flush();
    h.healthy.s1 = true;
    await p;

    h.healthy.s1 = false; // the old server is going away
    const set = h.manager.setShadowState(true);
    await vi.waitFor(() => expect(h.children[0].kill).toHaveBeenCalledWith('SIGTERM'));
    h.children[0].emit('exit', 0, 'SIGTERM');
    await vi.waitFor(() => expect(s1Calls(h)).toHaveLength(2));
    expect(s1Calls(h)[1][2].env.SYSTEM_ONE_SHADOW_STATE).toBe('1');
    h.healthy.s1 = true;
    await expect(set).resolves.toBe(true);

    const settings = path.join(h.home, '.allternit', 'system-one', 'settings.json');
    expect(JSON.parse(fs.readFileSync(settings, 'utf8'))).toEqual({ shadowState: true });
    // A new app session (fresh manager) reads it back; removing Laya keeps it.
    fs.mkdirSync(h.root, { recursive: true });
    await h.manager.remove();
    expect(new SystemOneManager({ ...(h.manager as any).deps }).shadowState).toBe(true);
  });

  it('honours the legacy env export until a setting is saved, then the setting wins', async () => {
    const h = harness({ env: { SYSTEM_ONE_SHADOW_STATE: '1' } });
    expect(h.manager.shadowState).toBe(true);
    await h.manager.setShadowState(false); // no owned S1 process: just saved
    expect(h.manager.shadowState).toBe(false);
    const p = h.manager.startSystemOne();
    await flush();
    expect(s1Calls(h)[0][2].env).not.toHaveProperty('SYSTEM_ONE_SHADOW_STATE');
    h.healthy.s1 = true;
    await p;
  });
});

describe('lifecycle', () => {
  it('starts S1 on 7717 with the shadow ledger and Laya URL, then Laya with the pinned checkpoint', async () => {
    const h = harness();
    installLaya(h.root);
    const started = h.manager.startWithApp();
    await flush();
    const s1Call = h.spawnSidecar.mock.calls.find(([cmd]) => String(cmd).endsWith('system-one'));
    expect(s1Call?.[1]).toEqual(['serve', '--port', '7717']);
    expect(s1Call?.[2].env.SYSTEM_ONE_LAYA_URL).toBe('http://127.0.0.1:7718');
    expect(s1Call?.[2].env.ALLTERNIT_S1_SHADOW_DIR).toBe(path.join(h.home, '.allternit', 'system-one', 'shadow'));
    const layaCall = h.spawnSidecar.mock.calls.find(([, args]) => String(args[0]).endsWith('serve-laya.sh'));
    expect(layaCall?.[0]).toBe('bash');
    expect(layaCall?.[2].env).toMatchObject({ LAYA_HOME: h.root, LAYA_PORT: '7718', LAYA_REVISION: LAYA_PINNED_REVISION, UV: h.uv });
    expect(h.manager.getApiEnvironment()).not.toHaveProperty('ALLTERNIT_S1_BACKEND');
    h.healthy.s1 = true;
    h.healthy.laya = true;
    await started;
    const status = await h.manager.getStatus();
    expect(status.systemOne.running).toBe(true);
    expect(status.laya.running).toBe(true);
    expect(status.backend).toBe('laya_bundled');
    expect(status.error).toBeUndefined();

    h.manager.stop();
    for (const c of h.children) expect(c.kill).toHaveBeenCalledWith('SIGTERM');
  });

  it('installs Laya in the background on first run, then serves it', async () => {
    const h = harness();
    const started = h.manager.startWithApp();
    await flush();
    expect(h.spawn).toHaveBeenCalledTimes(1); // installer
    expect(h.spawnSidecar.mock.calls.some(([, args]) => String(args[0]).endsWith('serve-laya.sh'))).toBe(false);
    installLaya(h.root);
    h.children.find((c) => c === h.spawn.mock.results[0].value)!.emit('exit', 0, null);
    await flush();
    await flush();
    expect(h.spawnSidecar.mock.calls.some(([, args]) => String(args[0]).endsWith('serve-laya.sh'))).toBe(true);
    h.healthy.s1 = true;
    h.healthy.laya = true;
    await started;
    expect(h.manager.backend).toBe('laya_bundled');
  });

  it('serves a local checkpoint path instead of a hub revision', async () => {
    const h = harness({ env: { ALLTERNIT_LAYA_CHECKPOINT_PATH: '/models/laya-ft' } });
    installLaya(h.root);
    h.healthy.s1 = true;
    const p = h.manager.startLaya();
    await flush();
    const env = h.spawnSidecar.mock.calls[0][2].env;
    expect(env.LAYA_CHECKPOINT_PATH).toBe('/models/laya-ft');
    expect(env).not.toHaveProperty('LAYA_REVISION');
    h.healthy.laya = true;
    await p;
  });

  it('skips Laya without uv and never blocks S1', async () => {
    const h = harness({ uv: false });
    h.healthy.s1 = true; // an S1 already serving 7717 is reused, not respawned
    await h.manager.startWithApp();
    expect(h.spawnSidecar).not.toHaveBeenCalled();
    expect(h.spawn).not.toHaveBeenCalled();
    const status = await h.manager.getStatus();
    expect(status.laya.needsUv).toBe(true);
    expect(status.backend).toBe('system_one_local');
    h.manager.stop(); // nothing of ours to kill
  });

  it('records a failed start in status instead of throwing', async () => {
    const h = harness({ binary: false, scripts: false });
    installLaya(h.root);
    await expect(h.manager.startWithApp()).resolves.toBeUndefined();
    const status = await h.manager.getStatus();
    expect(status.systemOne.available).toBe(false);
    expect(status.error).toMatch(/not available in this build/);
  });

  it('reports a Laya that exits before turning healthy', async () => {
    const h = harness();
    installLaya(h.root);
    const p = h.manager.startLaya();
    await flush();
    h.children[0].emit('exit', 1, null);
    await expect(p).rejects.toThrow('Laya exited before becoming healthy');
  });

  it('does not run Laya on Windows', async () => {
    const h = harness({ platform: 'win32', uv: false });
    h.healthy.s1 = true;
    await h.manager.startWithApp();
    const status = await h.manager.getStatus();
    expect(status.layaSupported).toBe(false);
    await expect(h.manager.install()).rejects.toThrow('not supported');
  });
});

describe('remove', () => {
  it('deletes only the managed Laya root', async () => {
    const h = harness();
    installLaya(h.root);
    await h.manager.remove();
    expect(fs.existsSync(h.root)).toBe(false);

    const odd = harness({ env: { ALLTERNIT_LAYA_HOME: path.join(tmp, 'elsewhere') } });
    await expect(odd.manager.remove()).rejects.toThrow('Refusing to delete');
  });
});
