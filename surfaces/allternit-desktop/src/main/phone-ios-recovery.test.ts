import { describe, expect, it, vi } from 'vitest';

// Fakes below copy the REAL response shapes used by phone-link-ios.test.ts:
// go-ios list → string[], launch → {ok:true}, the runner's config call →
// {allow_remote_access, address, listen_port}, /whoami → {device_id, name}.
import type { LongRunning, RunTestOptions } from './phone-goios.js';
import { IctrlError } from './ictrl-client.js';
import { PhoneLinkHealthMonitor, type LinkedPhone } from './phone-link-health.js';
import { probeLinkedPhone } from './phone-link-backend.js';
import { SigningError, type RunnerArtifacts, type RunnerSigner, type SignRequest } from './phone-ios-runner.js';
import type { GoIosLike, IctrlLike, IosDriverDeps, IosLinkRecord } from './phone-link-ios.js';
import { IosRecoveryScheduler, RENEWAL_RETRY_MS, renewalDue, renewalNotice, RESIGN_WINDOW_MS, type IosRecoveryDeps, type RenewalBlocker } from './phone-ios-recovery.js';

const instant = () => Promise.resolve();
const LAN = '192.168.1.20:8200';
const NOW = Date.parse('2026-10-17T12:00:00.000Z');
const PAIRING_REMOTE = 'Library/Application Support/Pairing/pairingFile.plist';

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
  connected: boolean;
  runnerStandalone: boolean;
  lanDeadHosts: string[];
  key: boolean;
  installed: string[];
  launches: string[];
  pushes: { bundleId: string; remote: string }[];
  imageAutos: number;
  tunnels: ReturnType<typeof proc>[];
  saved: IosLinkRecord[];
  signRequests: SignRequest[];
  calls: { host: string; method: string; params?: Record<string, unknown> }[];
  logs: string[];
}

function world(over: Partial<World> = {}): World {
  return {
    connected: true,
    runnerStandalone: true,
    lanDeadHosts: [],
    key: true,
    installed: [],
    launches: [],
    pushes: [],
    imageAutos: 0,
    tunnels: [],
    saved: [],
    signRequests: [],
    calls: [],
    logs: [],
    ...over,
  };
}

function fakeGoIos(w: World): GoIosLike {
  return {
    list: async () => (w.connected ? ['U'] : []),
    info: async () => ({ ok: true, name: 'Eoj iPhone', version: '26.6.2' }),
    pair: async () => ({ ok: true }),
    devmodeEnabled: async () => true,
    devmodeReveal: async () => {},
    tunnelStart: () => {
      const p = proc();
      w.tunnels.push(p);
      return p;
    },
    tunnels: async () => [{ udid: 'U' }],
    imageAuto: async () => {
      w.imageAutos++;
    },
    install: async (_u, file) => void w.installed.push(file),
    uninstall: async () => {},
    apps: async () => [],
    launch: async (_u, bundleId) => {
      w.launches.push(bundleId);
      w.runnerStandalone = true;
      return { ok: true };
    },
    kill: async () => {},
    pushToApp: async (_u, bundleId, _local, remote) => {
      w.pushes.push({ bundleId, remote });
    },
    runtest: (_opts: RunTestOptions) => proc(),
  };
}

function fakeIctrl(w: World): IosDriverDeps['ictrl'] {
  const make = (host: string): IctrlLike => ({
    host,
    hasKey: () => w.key,
    forget: () => void (w.key = false),
    whoami: async () => (w.runnerStandalone ? { device_id: 'dev-1', name: 'Eoj iPhone' } : null),
    pair: async () => {
      w.key = true;
    },
    call: async <T,>(method: string, params?: Record<string, unknown>) => {
      w.calls.push({ host, method, params });
      if (!w.key) throw new IctrlError('no_key', 'no key');
      if (host === LAN && !(w.runnerStandalone && !w.lanDeadHosts.includes(host))) throw new IctrlError('unreachable', 'unreachable');
      if (method === 'config') return { allow_remote_access: true, address: LAN, listen_port: 8200 } as T;
      return {} as T;
    },
    withHost: (h) => make(h),
  });
  return (_udid, host) => make(host);
}

function deps(w: World, signer: RunnerSigner, extra: Partial<IosRecoveryDeps> = {}): IosRecoveryDeps {
  const records = new Map<string, IosLinkRecord>();
  return {
    goios: fakeGoIos(w),
    artifacts: async () => ARTIFACTS,
    signer,
    ictrl: fakeIctrl(w),
    forward: async () => ({ port: 50123, close: async () => {} }),
    ddiDir: async () => '/data/ddi',
    readPairRecord: async () => Buffer.from('plist'),
    store: {
      load: (u) => records.get(u) ?? null,
      all: () => [...records.values()],
      save: (r) => {
        records.set(r.udid, r);
        w.saved.push(r);
      },
    },
    workDir: '/tmp/work',
    now: () => NOW,
    sleep: instant,
    logger: (m) => w.logs.push(m),
    ...extra,
  };
}

/** The product signer path: reuses the Keychain session (empty password) or fails like the sidecar would. */
function renewSigner(w: World, behavior: 'ok' | 'password_required' | 'two_factor_failed' = 'ok'): RunnerSigner {
  return {
    kind: 'apple-id',
    needsAppleId: true,
    async sign(req) {
      w.signRequests.push(req);
      if (behavior !== 'ok') throw new SigningError(behavior, 'session lapsed');
      return {
        runnerIpa: '/s/runner.ipa',
        hostIpa: '/s/host.ipa',
        bundlePrefix: 'com.allternit.phonelink',
        runnerBundleId: ARTIFACTS.manifest.runner.bundleId,
        hostBundleId: ARTIFACTS.manifest.host.bundleId,
        teamId: 'TEAM123456',
        expiresAt: new Date(NOW + 7 * 86400_000).toISOString(),
      };
    },
  };
}

function record(over: Partial<IosLinkRecord> = {}): IosLinkRecord {
  return {
    udid: 'U',
    deviceId: 'dev-1',
    deviceName: 'Eoj iPhone',
    runnerBundleId: ARTIFACTS.manifest.runner.bundleId,
    hostBundleId: ARTIFACTS.manifest.host.bundleId,
    xctestConfig: 'iCTRLUITests.xctest',
    testToRun: 't',
    expiresAt: new Date(NOW + 20 * 3600_000).toISOString(),
    appleId: 'eoj@example.com',
    lanHost: LAN,
    linkedAt: new Date(NOW).toISOString(),
    ...over,
  };
}

const FRESH = new Date(NOW + 5 * 86400_000).toISOString(); // outside the 48 h window

describe('renewalDue', () => {
  it('is due only inside the 48 h window, including already-expired', () => {
    expect(renewalDue(record({ expiresAt: new Date(NOW + 72 * 3600_000).toISOString() }), NOW)).toBe(false);
    expect(renewalDue(record({ expiresAt: new Date(NOW + RESIGN_WINDOW_MS).toISOString() }), NOW)).toBe(true);
    expect(renewalDue(record({ expiresAt: new Date(NOW - 3600_000).toISOString() }), NOW)).toBe(true);
    expect(renewalDue(record({ expiresAt: undefined }), NOW)).toBe(false);
    expect(renewalDue(record({ expiresAt: 'not a date' }), NOW)).toBe(false);
  });
});

describe('IosRecoveryScheduler — renewal', () => {
  it('re-signs with the saved session (empty password), reinstalls, relaunches, updates expiresAt', async () => {
    const w = world({ runnerStandalone: true });
    const d = deps(w, renewSigner(w));
    d.store.save(record());
    const recovered: [string, string][] = [];
    d.onRecovered = (udid, kind) => recovered.push([udid, kind]);

    await new IosRecoveryScheduler({ deps: async () => d }).tick();

    expect(w.signRequests).toHaveLength(1);
    expect(w.signRequests[0].credentials).toEqual({ appleId: 'eoj@example.com', password: '' });
    expect(w.signRequests[0].twoFactor).toBeUndefined(); // a renewal never prompts
    expect(w.installed).toEqual(['/s/host.ipa', '/s/runner.ipa']);
    expect(d.store.load('U')?.expiresAt).toBe(new Date(NOW + 7 * 86400_000).toISOString());
    // Relaunched over the cable and confirmed answering; health re-probe signalled.
    expect(w.launches).toEqual([ARTIFACTS.manifest.runner.bundleId]);
    expect(recovered).toEqual([['U', 'renewal']]);
    // The pairing file the phone-side "Mount without a Mac" reads is refreshed.
    expect(w.pushes).toEqual([{ bundleId: ARTIFACTS.manifest.host.bundleId, remote: PAIRING_REMOTE }]);
    // A later pass sees the new expiry and leaves the phone alone.
    await new IosRecoveryScheduler({ deps: async () => d }).tick();
    expect(w.signRequests).toHaveLength(1);
  });

  it('password_required stays manual: nothing installed, record untouched, no recovery signal', async () => {
    const w = world({ runnerStandalone: true });
    const d = deps(w, renewSigner(w, 'password_required'));
    const before = record();
    d.store.save(before);
    const recovered: [string, string][] = [];
    d.onRecovered = (udid, kind) => recovered.push([udid, kind]);

    await new IosRecoveryScheduler({ deps: async () => d }).tick();

    expect(w.signRequests).toHaveLength(1);
    expect(w.installed).toHaveLength(0);
    expect(w.launches).toHaveLength(0);
    expect(d.store.load('U')).toEqual(before);
    expect(recovered).toHaveLength(0);
    expect(w.logs.some((l) => l.includes('Apple sign-in') && l.includes('expired'))).toBe(true);
  });

  it('a failing renewal is not retried every pass: one attempt per 6 h per phone', async () => {
    const w = world({ runnerStandalone: true });
    const d = deps(w, renewSigner(w, 'password_required'));
    d.store.save(record());
    let t = NOW;
    d.now = () => t;
    const scheduler = new IosRecoveryScheduler({ deps: async () => d });
    await scheduler.tick();
    t += 60_000;
    await scheduler.tick();
    t += 5 * 3600_000;
    await scheduler.tick();
    expect(w.signRequests).toHaveLength(1);
    t += RENEWAL_RETRY_MS;
    await scheduler.tick();
    expect(w.signRequests).toHaveLength(2);
  });

  it('a phone that is not on the cable is left manual and untouched', async () => {
    const w = world({ connected: false, runnerStandalone: true });
    const d = deps(w, renewSigner(w));
    const before = record();
    d.store.save(before);

    await new IosRecoveryScheduler({ deps: async () => d }).tick();

    expect(w.signRequests).toHaveLength(0);
    expect(d.store.load('U')).toEqual(before);
    expect(w.logs.some((l) => l.includes("isn't on the cable"))).toBe(true);
  });

  it('a link without an Apple ID cannot renew automatically and stays manual', async () => {
    const w = world({ runnerStandalone: true });
    const d = deps(w, renewSigner(w));
    d.store.save(record({ appleId: undefined }));

    await new IosRecoveryScheduler({ deps: async () => d }).tick();

    expect(w.signRequests).toHaveLength(0);
    expect(w.logs.some((l) => l.includes('no Apple ID on record'))).toBe(true);
  });

  it('a renewed helper that stays silent still updates expiresAt and signals no recovery', async () => {
    const w = world({ runnerStandalone: false, lanDeadHosts: [LAN] });
    const d = deps(w, renewSigner(w), { discover: async () => null });
    d.store.save(record());
    const recovered: [string, string][] = [];
    d.onRecovered = (udid, kind) => recovered.push([udid, kind]);

    await new IosRecoveryScheduler({ deps: async () => d }).tick();

    expect(w.signRequests).toHaveLength(1);
    expect(w.installed).toEqual(['/s/host.ipa', '/s/runner.ipa']);
    expect(d.store.load('U')?.expiresAt).toBe(new Date(NOW + 7 * 86400_000).toISOString());
    expect(recovered).toHaveLength(0);
    expect(w.logs.some((l) => l.includes("isn't answering yet"))).toBe(true);
  });
});

describe('renewal that needs a person', () => {
  it('tells the person why, once per cause, through onNeedsYou', async () => {
    const cases: Array<[Partial<IosLinkRecord>, Parameters<typeof world>[0], 'ok' | 'password_required', RenewalBlocker]> = [
      [{}, { connected: false, runnerStandalone: true }, 'ok', 'cable'],
      [{}, { runnerStandalone: true }, 'password_required', 'apple_sign_in'],
      [{ appleId: undefined }, { runnerStandalone: true }, 'ok', 'no_apple_id'],
    ];
    for (const [over, opts, behavior, blocker] of cases) {
      const w = world(opts);
      const d = deps(w, renewSigner(w, behavior));
      d.store.save(record(over));
      const seen: RenewalBlocker[] = [];
      d.onNeedsYou = (_r, b) => seen.push(b);
      await new IosRecoveryScheduler({ deps: async () => d }).tick();
      expect(seen).toEqual([blocker]);
    }
  });

  it('a renewed helper raises no notice', async () => {
    const w = world({ runnerStandalone: true });
    const d = deps(w, renewSigner(w));
    d.store.save(record());
    const seen: RenewalBlocker[] = [];
    d.onNeedsYou = (_r, b) => seen.push(b);
    await new IosRecoveryScheduler({ deps: async () => d }).tick();
    expect(seen).toEqual([]);
  });

  it('the notice names the phone, the deadline and the one thing to do', () => {
    const r = record({ deviceName: 'iPhone (4)', expiresAt: '2026-10-26T01:27:28Z' });
    const before = Date.parse('2026-10-25T20:00:00Z');
    const sign = renewalNotice(r, 'apple_sign_in', before, 'America/Chicago');
    expect(sign.title).toBe('Sign in to Apple for iPhone (4)');
    expect(sign.body).toContain('stops working Sun, Oct 25, 8:27 PM');
    expect(sign.body).toContain('open Phone Link setup and sign in');
    expect(renewalNotice(r, 'cable', before, 'America/Chicago').title).toBe('Plug in iPhone (4)');
    expect(renewalNotice(r, 'cable', Date.parse('2026-10-26T03:00:00Z'), 'America/Chicago').body).toContain('stopped working Sun, Oct 25, 8:27 PM');
  });
});

describe('IosRecoveryScheduler — remount on the cable', () => {
  it('a restarted phone on the cable is remounted exactly once, not repeated while it runs', async () => {
    const w = world({ runnerStandalone: false });
    const d = deps(w, renewSigner(w), { discover: async () => null });
    d.store.save(record({ expiresAt: FRESH }));
    // Gate the runner's config read so the first remount is still in flight
    // when the second pass gets its turn.
    let entered!: () => void;
    const gateEntered = new Promise<void>((resolve) => (entered = resolve));
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const base = d.ictrl;
    d.ictrl = (udid, host) => {
      const c = base(udid, host);
      return {
        ...c,
        call: async <T,>(method: string, params?: Record<string, unknown>, timeoutMs?: number) => {
          if (method === 'config') {
            entered();
            await gate;
          }
          return c.call<T>(method, params, timeoutMs);
        },
      };
    };
    const recovered: [string, string][] = [];
    d.onRecovered = (udid, kind) => recovered.push([udid, kind]);

    const scheduler = new IosRecoveryScheduler({ deps: async () => d, remountAfterMisses: 1 });
    const first = scheduler.tick();
    const second = scheduler.tick();
    await gateEntered;
    // The remount is mid-flight: the image is mounted once, one tunnel is up,
    // and the second pass hasn't started a second remount.
    expect(w.imageAutos).toBe(1);
    expect(w.tunnels).toHaveLength(1);
    expect(w.calls.filter((call) => call.method === 'config')).toHaveLength(0);
    release();
    await Promise.all([first, second]);

    expect(w.imageAutos).toBe(1);
    expect(w.tunnels).toHaveLength(1);
    expect(w.launches).toEqual([ARTIFACTS.manifest.runner.bundleId]);
    expect(recovered).toEqual([['U', 'remount']]);
    expect(d.store.load('U')?.lanHost).toBe(LAN);
  });

  it('remount success updates health: the re-probe classifies the phone HEALTHY', async () => {
    const w = world({ runnerStandalone: false });
    const d = deps(w, renewSigner(w), { discover: async () => null });
    d.store.save(record({ expiresAt: FRESH }));
    const saved = d.store.load('U')!;
    const phone: LinkedPhone = { deviceId: 'dev-1', platform: 'ios', storeKey: 'k', lanHost: LAN, expiresAt: saved.expiresAt };
    const monitor = new PhoneLinkHealthMonitor({
      phones: () => [phone],
      probe: (p) => probeLinkedPhone(p, (_phone, host) => d.ictrl('U', host)),
      now: () => NOW,
    });

    // Evidence first: the restarted phone is silent.
    expect((await monitor.check('dev-1')).state).toBe('TRANSPORT_OFFLINE');

    // The same wiring phone-service.ts uses: recovery success re-probes the phone.
    const reProbes: Promise<unknown>[] = [];
    d.onRecovered = (udid) => reProbes.push(monitor.check(d.store.load(udid)?.deviceId ?? udid));
    await new IosRecoveryScheduler({ deps: async () => d, remountAfterMisses: 1 }).tick();
    await Promise.all(reProbes);

    expect(w.launches).toEqual([ARTIFACTS.manifest.runner.bundleId]);
    expect(monitor.health('dev-1')?.state).toBe('HEALTHY');
  });

  it('a linked phone on the cable whose helper answers is left alone', async () => {
    const w = world({ runnerStandalone: true });
    const d = deps(w, renewSigner(w));
    d.store.save(record({ expiresAt: FRESH }));
    const recovered: [string, string][] = [];
    d.onRecovered = (udid, kind) => recovered.push([udid, kind]);

    await new IosRecoveryScheduler({ deps: async () => d, remountAfterMisses: 1 }).tick();

    expect(w.launches).toHaveLength(0);
    expect(w.tunnels).toHaveLength(0);
    expect(w.imageAutos).toBe(0);
    expect(recovered).toHaveLength(0);
  });

  it('one missed check does not relaunch the helper; two in a row do', async () => {
    const w = world({ runnerStandalone: false });
    const d = deps(w, renewSigner(w), { discover: async () => null });
    d.store.save(record({ expiresAt: FRESH }));
    const scheduler = new IosRecoveryScheduler({ deps: async () => d });

    await scheduler.tick();
    expect(w.launches).toHaveLength(0);
    expect(w.imageAutos).toBe(0);

    await scheduler.tick();
    expect(w.launches).toEqual([ARTIFACTS.manifest.runner.bundleId]);
  });

  it('a phone off the cable with a live helper is left alone', async () => {
    const w = world({ connected: false, runnerStandalone: true });
    const d = deps(w, renewSigner(w));
    d.store.save(record({ expiresAt: FRESH }));

    await new IosRecoveryScheduler({ deps: async () => d, remountAfterMisses: 1 }).tick();

    expect(w.launches).toHaveLength(0);
    expect(w.signRequests).toHaveLength(0);
  });
});

describe('IosRecoveryScheduler — idle cost', () => {
  it('a null deps provider (no linked iPhone) makes the pass a no-op', async () => {
    const provider = vi.fn(async () => null);
    const scheduler = new IosRecoveryScheduler({ deps: provider });
    await scheduler.tick();
    await scheduler.tick();
    expect(provider).toHaveBeenCalledTimes(2); // checked, then skipped before touching go-ios
  });

  it('start() schedules passes and stop() ends them', async () => {
    vi.useFakeTimers();
    try {
      const provider = vi.fn(async () => null);
      const scheduler = new IosRecoveryScheduler({ deps: provider, intervalMs: 1_000 });
      scheduler.start();
      await vi.advanceTimersByTimeAsync(3_500);
      scheduler.stop();
      const calls = provider.mock.calls.length;
      expect(calls).toBeGreaterThanOrEqual(3); // immediate + ~3 intervals
      await vi.advanceTimersByTimeAsync(5_000);
      expect(provider.mock.calls.length).toBe(calls); // no more passes after stop
    } finally {
      vi.useRealTimers();
    }
  });
});
