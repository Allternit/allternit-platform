import { describe, expect, it } from 'vitest';
import { PhoneLinkManager } from './phone-link.js';
import type { PhoneLinkStatus } from './phone-link-types.js';
import { iosVendorApps, pickLanHost, type GoIosLike, type IctrlLike, type IosDriverDeps, type IosLinkRecord } from './phone-link-ios.js';
import type { LongRunning, RunTestOptions } from './phone-goios.js';
import { RunnerUnavailableError, SigningError, type RunnerArtifacts, type RunnerSigner, type SignRequest } from './phone-ios-runner.js';
import { IctrlError } from './ictrl-client.js';

const instant = () => Promise.resolve();
const LAN = '192.168.1.20:8200';

function proc(): LongRunning & { stopped: boolean } {
  const p = {
    stopped: false,
    onLine: () => () => {},
    exited: new Promise<{ code: number | null; signal: string | null }>(() => {}),
    isRunning: () => !p.stopped,
    stop: () => void (p.stopped = true),
  };
  return p;
}

const ARTIFACTS: RunnerArtifacts = {
  runnerIpa: '/a/runner.ipa',
  hostIpa: '/a/host.ipa',
  manifest: {
    version: '1.0.0',
    protocol: 1,
    bundlePrefix: 'com.allternit.phonelink',
    runner: { file: 'r.ipa', sha256: 'a'.repeat(64), size: 1, bundleId: 'com.allternit.phonelink.ictrl.uitests.xctrunner', xctestConfig: 'iCTRLUITests.xctest', testToRun: 'iCTRLUITests.iCTRLUITests/testStartServer' },
    host: { file: 'h.ipa', sha256: 'b'.repeat(64), size: 1, bundleId: 'com.allternit.phonelink.ictrl' },
  },
};

interface World {
  phonePresent: boolean[];
  trusted: boolean[];
  devmode: boolean[];
  untrustedLaunches: number;
  runnerStandalone: boolean;
  cableSession: boolean;
  lanDiesWithoutCable: boolean;
  lanDeadHosts: string[];
  key: boolean;
  installed: string[];
  launches: string[];
  runtests: RunTestOptions[];
  tunnels: ReturnType<typeof proc>[];
  forwardClosed: number;
  saved: IosLinkRecord[];
  signRequests: SignRequest[];
  calls: { host: string; method: string; params?: Record<string, unknown> }[];
}

function world(over: Partial<World> = {}): World {
  return {
    phonePresent: [false, true],
    trusted: [false, true],
    devmode: [false, false, true],
    untrustedLaunches: 1,
    runnerStandalone: false,
    cableSession: false,
    lanDiesWithoutCable: false,
    lanDeadHosts: [],
    key: false,
    installed: [],
    launches: [],
    runtests: [],
    tunnels: [],
    forwardClosed: 0,
    saved: [],
    signRequests: [],
    calls: [],
    ...over,
  };
}

/** Takes the next scripted value, repeating the last one. */
const next = <T,>(list: T[]): T => (list.length > 1 ? list.shift()! : list[0]);

function fakeGoIos(w: World): GoIosLike {
  let rebootPending = false;
  return {
    list: async () => {
      if (rebootPending) {
        rebootPending = false;
        return [];
      }
      return next(w.phonePresent) ? ['U'] : [];
    },
    info: async () => (next(w.trusted) ? { ok: true, name: 'Eoj iPhone', version: '26.6.2' } : { ok: false, error: 'could not retrieve PairRecord' }),
    pair: async () => ({ ok: false, error: 'pending' }),
    devmodeEnabled: async () => {
      const on = next(w.devmode);
      if (!on && w.devmode.length === 1) rebootPending = true;
      return on;
    },
    devmodeReveal: async () => {},
    tunnelStart: () => {
      const p = proc();
      w.tunnels.push(p);
      return p;
    },
    tunnels: async () => [{ udid: 'U' }],
    imageAuto: async () => {},
    install: async (_u, file) => void w.installed.push(file),
    apps: async () => ['com.openai.chat', 'ai.x.GrokApp', 'com.apple.Maps'],
    launch: async (_u, bundleId) => {
      w.launches.push(bundleId);
      if (bundleId === ARTIFACTS.manifest.host.bundleId && w.untrustedLaunches > 0) {
        w.untrustedLaunches--;
        return { ok: false, error: 'profile has not been explicitly trusted', untrusted: true, notInstalled: false };
      }
      if (bundleId === ARTIFACTS.manifest.runner.bundleId) {
        w.cableSession = false;
        w.runnerStandalone = true;
      }
      return { ok: true };
    },
    kill: async () => {},
    pushToApp: async () => {},
    runtest: (opts) => {
      w.runtests.push(opts);
      w.cableSession = true;
      const p = proc();
      const stop = p.stop;
      p.stop = () => {
        stop();
        w.cableSession = false;
      };
      return p;
    },
  };
}

function fakeIctrl(w: World): (udid: string, host: string) => IctrlLike {
  const make = (host: string): IctrlLike => ({
    host,
    hasKey: () => w.key,
    forget: () => void (w.key = false),
    whoami: async () => (host.startsWith('127.0.0.1') && w.cableSession ? { device_id: 'dev-1', name: 'Eoj iPhone' } : null),
    pair: async () => {
      w.key = true;
    },
    call: async <T,>(method: string, params?: Record<string, unknown>) => {
      w.calls.push({ host, method, params });
      if (!w.key) throw new IctrlError('no_key', 'no key');
      const viaCable = host.startsWith('127.0.0.1');
      const tunnelUp = w.tunnels.some((t) => !t.stopped);
      const lanUp = w.runnerStandalone && (!w.lanDiesWithoutCable || tunnelUp) && !w.lanDeadHosts.includes(host);
      if (viaCable ? !(w.cableSession || w.runnerStandalone) : !lanUp) throw new IctrlError('unreachable', 'unreachable');
      if (method === 'config') return { allow_remote_access: true, address: LAN, listen_port: 8200 } as T;
      return {} as T;
    },
    withHost: (h) => make(h),
  });
  return (_udid, host) => make(host);
}

function deps(w: World, signer: RunnerSigner, extra: Partial<IosDriverDeps> = {}): IosDriverDeps {
  const records = new Map<string, IosLinkRecord>();
  return {
    goios: fakeGoIos(w),
    artifacts: async () => ARTIFACTS,
    signer,
    ictrl: fakeIctrl(w),
    forward: async () => ({ port: 50123, close: async () => void w.forwardClosed++ }),
    ddiDir: async () => '/data/ddi',
    store: {
      load: (u) => records.get(u) ?? null,
      all: () => [...records.values()],
      save: (r) => {
        records.set(r.udid, r);
        w.saved.push(r);
      },
    },
    workDir: '/tmp/work',
    clientName: 'Allternit Desktop (test)',
    ...extra,
  };
}

function appleIdSigner(w: World, failFirst = false): RunnerSigner {
  let attempts = 0;
  return {
    kind: 'apple-id',
    needsAppleId: true,
    async sign(req) {
      w.signRequests.push(req);
      if (failFirst && attempts++ === 0) throw new SigningError('bad_credentials', 'nope');
      const code = await req.twoFactor!({ sms: false });
      expect(code).toBe('123456');
      return {
        runnerIpa: '/s/runner.ipa',
        hostIpa: '/s/host.ipa',
        bundlePrefix: 'com.allternit.phonelink',
        runnerBundleId: ARTIFACTS.manifest.runner.bundleId,
        hostBundleId: ARTIFACTS.manifest.host.bundleId,
        teamId: 'TEAM123456',
        expiresAt: new Date(Date.now() + 7 * 86400_000).toISOString(),
      };
    },
  };
}

function waitFor(manager: PhoneLinkManager, state: PhoneLinkStatus['state']): Promise<PhoneLinkStatus> {
  return new Promise((resolve) => {
    if (manager.status().state === state) return resolve(manager.status());
    const on = (s: PhoneLinkStatus) => {
      if (s.state !== state) return;
      manager.off('status', on);
      resolve(s);
    };
    manager.on('status', on);
  });
}

describe('iPhone Phone Link', () => {
  it('walks cable → trust → Apple ID → Developer Mode → install → trust cert → Wi-Fi', async () => {
    const w = world();
    const manager = new PhoneLinkManager({ platform: 'darwin', sleep: instant, ios: async () => deps(w, appleIdSigner(w, true)) });
    const seen: string[] = [];
    manager.on('status', (s: PhoneLinkStatus) => seen[seen.length - 1] !== s.state && seen.push(s.state));
    manager.start('ios');

    await waitFor(manager, 'apple_id');
    manager.submitAppleId({ appleId: 'eoj@example.com', password: 'wrong' });
    // bad password → asked again, with the reason
    await new Promise<void>((resolve) => {
      const on = (s: PhoneLinkStatus) => {
        if (s.state === 'apple_id' && /didn't work/.test(s.detail ?? '')) {
          manager.off('status', on);
          resolve();
        }
      };
      manager.on('status', on);
    });
    manager.submitAppleId({ appleId: 'eoj@example.com', password: 'right' });
    await waitFor(manager, 'two_factor');
    manager.submitTwoFactor('123456');
    await waitFor(manager, 'trust_certificate');
    manager.continue();
    const ready = await waitFor(manager, 'ready_wifi');

    expect(seen).toEqual([
      'needs_cable',
      'trust_prompt',
      'apple_id',
      'two_factor',
      'apple_id',
      'developer_mode',
      'waiting_for_reboot',
      'installing',
      'trust_certificate',
      'installing',
      'ready_wifi',
    ]);
    expect(ready).toMatchObject({
      platform: 'ios',
      deviceName: 'Eoj iPhone',
      osVersion: 'iOS 26.6.2',
      detail: LAN,
      apps: [
        { vendor: 'chatgpt', signedIn: null },
        { vendor: 'grok', signedIn: null },
      ],
    });
    expect(w.signRequests[1].credentials).toEqual({ appleId: 'eoj@example.com', password: 'right' });
    expect(w.installed).toEqual(['/s/host.ipa', '/s/runner.ipa']);
    // Only Desktop's own cable launch carries the one-shot auto-approval.
    expect(w.runtests).toHaveLength(1);
    expect(w.runtests[0].env).toEqual({ ICTRL_ALLOW_AUTH_ONCE: '1' });
    expect(w.calls.find((c) => c.method === 'config')?.params).toEqual({ allow_remote_access: true });
    // Runner relaunched as a plain app, then proven reachable with no tunnel and no forward.
    expect(w.launches.at(-1)).toBe(ARTIFACTS.manifest.runner.bundleId);
    expect(w.tunnels.every((t) => t.stopped)).toBe(true);
    expect(w.forwardClosed).toBe(1);
    expect(w.saved.at(-1)).toMatchObject({ udid: 'U', lanHost: LAN, runnerBundleId: ARTIFACTS.manifest.runner.bundleId });
  });

  it('reconnects a linked phone over Wi-Fi with no cable, no signing, no install', async () => {
    const w = world({ phonePresent: [false], trusted: [false], key: true, runnerStandalone: true });
    const signer = appleIdSigner(w);
    const d = deps(w, signer);
    d.store.save({
      udid: 'U',
      runnerBundleId: ARTIFACTS.manifest.runner.bundleId,
      hostBundleId: ARTIFACTS.manifest.host.bundleId,
      xctestConfig: 'iCTRLUITests.xctest',
      testToRun: 't',
      expiresAt: new Date(Date.now() + 5 * 86400_000).toISOString(),
      lanHost: LAN,
      linkedAt: new Date().toISOString(),
    });
    const manager = new PhoneLinkManager({ platform: 'darwin', sleep: instant, ios: async () => d });
    manager.start('ios');
    const ready = await waitFor(manager, 'ready_wifi');
    expect(ready.detail).toBe(LAN);
    expect(w.signRequests).toHaveLength(0);
    expect(w.installed).toHaveLength(0);
    expect(w.runtests).toHaveLength(0);
  });

  it('after a phone restart, remounts and relaunches over the cable and finds the new address', async () => {
    const w = world({ phonePresent: [true], trusted: [true], key: true, runnerStandalone: false });
    const d = deps(w, appleIdSigner(w));
    d.store.save({
      udid: 'U',
      runnerBundleId: ARTIFACTS.manifest.runner.bundleId,
      hostBundleId: ARTIFACTS.manifest.host.bundleId,
      xctestConfig: 'iCTRLUITests.xctest',
      testToRun: 't',
      expiresAt: new Date(Date.now() + 5 * 86400_000).toISOString(),
      lanHost: '192.168.1.99:8200',
      linkedAt: new Date().toISOString(),
    });
    const manager = new PhoneLinkManager({ platform: 'darwin', sleep: instant, ios: async () => d });
    manager.start('ios');
    const ready = await waitFor(manager, 'ready_wifi');
    expect(ready.detail).toBe(LAN);
    expect(w.launches).toEqual([ARTIFACTS.manifest.runner.bundleId]);
    expect(w.tunnels.every((t) => t.stopped)).toBe(true);
    expect(w.signRequests).toHaveLength(0);
  });

  it('finds the phone on the LAN by device id when its address changed, before reaching for the cable', async () => {
    const w = world({ phonePresent: [true], trusted: [true], key: true, runnerStandalone: true, lanDeadHosts: ['192.168.1.99:8200'] });
    const d = deps(w, appleIdSigner(w), {
      discover: async (deviceId: string, lastHost: string | undefined) => {
        expect(deviceId).toBe('dev-1');
        expect(lastHost).toBe('192.168.1.99:8200');
        return LAN;
      },
    });
    d.store.save({
      udid: 'U',
      deviceId: 'dev-1',
      runnerBundleId: ARTIFACTS.manifest.runner.bundleId,
      hostBundleId: ARTIFACTS.manifest.host.bundleId,
      xctestConfig: 'iCTRLUITests.xctest',
      testToRun: 't',
      expiresAt: new Date(Date.now() + 5 * 86400_000).toISOString(),
      lanHost: '192.168.1.99:8200',
      linkedAt: new Date().toISOString(),
    });
    const manager = new PhoneLinkManager({ platform: 'darwin', sleep: instant, ios: async () => d });
    manager.start('ios');
    const ready = await waitFor(manager, 'ready_wifi');
    expect(ready.detail).toBe(LAN);
    // No cable session, no relaunch: discovery answered first.
    expect(w.launches).toHaveLength(0);
    expect(w.tunnels).toHaveLength(0);
    expect(w.runtests).toHaveLength(0);
    expect(w.signRequests).toHaveLength(0);
    expect(d.store.load('U')?.lanHost).toBe(LAN);
  });

  it('fails retryably when the runner dies with the cable session', async () => {
    const w = world({ phonePresent: [true], trusted: [true], devmode: [true], untrustedLaunches: 0, lanDiesWithoutCable: true });
    const manager = new PhoneLinkManager({ platform: 'darwin', sleep: instant, ios: async () => deps(w, appleIdSigner(w)) });
    manager.start('ios');
    await waitFor(manager, 'apple_id');
    manager.submitAppleId({ appleId: 'a@b.c', password: 'p' });
    await waitFor(manager, 'two_factor');
    manager.submitTwoFactor('123456');
    const status = await waitFor(manager, 'error');
    expect(status.error).toMatchObject({ code: 'wifi_unreachable', retryable: true });
  });

  it('reports an unpublished runner as not retryable', async () => {
    const w = world({ phonePresent: [true], trusted: [true] });
    const manager = new PhoneLinkManager({
      platform: 'darwin',
      sleep: instant,
      ios: async () => deps(w, appleIdSigner(w), { artifacts: async () => Promise.reject(new RunnerUnavailableError('not yet')) }),
    });
    manager.start('ios');
    const status = await waitFor(manager, 'error');
    expect(status.error).toMatchObject({ code: 'runner_unavailable', retryable: false });
  });

  it('cancel while waiting for the Apple ID goes idle and never signs', async () => {
    const w = world({ phonePresent: [true], trusted: [true] });
    const manager = new PhoneLinkManager({ platform: 'darwin', sleep: instant, ios: async () => deps(w, appleIdSigner(w)) });
    manager.start('ios');
    await waitFor(manager, 'apple_id');
    expect(manager.cancel().state).toBe('idle');
    manager.submitAppleId({ appleId: 'late@x.y', password: 'p' });
    await new Promise((r) => setTimeout(r, 10));
    expect(manager.status().state).toBe('idle');
    expect(w.signRequests).toHaveLength(0);
  });

  it('without iOS deps the build says so', async () => {
    const manager = new PhoneLinkManager({ platform: 'darwin' });
    manager.start('ios');
    expect((await waitFor(manager, 'error')).error?.code).toBe('ios_not_ready');
  });
});

describe('helpers', () => {
  it('maps installed bundle ids to vendors', () => {
    expect(iosVendorApps(['com.anthropic.claude', 'com.microsoft.officemobile', 'x'])).toEqual([
      { vendor: 'claude', signedIn: null },
      { vendor: 'copilot', signedIn: null },
    ]);
  });

  it('picks the LAN address the runner reports', () => {
    expect(pickLanHost({ address: '10.0.0.5:8200' })).toBe('10.0.0.5:8200');
    expect(pickLanHost({ address: '127.0.0.1:8200', ipv4_interfaces: [{ name: 'pdp_ip0', address: '10.1.1.1' }, { name: 'en0', address: '192.168.4.9' }] })).toBe('192.168.4.9:8200');
    expect(pickLanHost({ address: '127.0.0.1:8200' })).toBeNull();
  });
});
