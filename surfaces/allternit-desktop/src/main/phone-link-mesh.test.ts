/**
 * Phone Link × mesh, with the development switch ON (ALLTERNIT_PHONE_LINK_MESH=1
 * is set via vi.hoisted before the drivers load — PHONE_LINK_MESH_ENABLED is
 * frozen at module load). The switch-off behaviour is proven by the existing
 * phone-link-*.test.ts files plus explicit switch-off tests there.
 *
 * Every fake response shape here is the REAL wire shape (Phone Link Android
 * 0.1.0, live-verified 2026-10-17, and the runner's real /schema document
 * from Schema.kt), not an invented one.
 */
import { describe, expect, it, vi } from 'vitest';

vi.hoisted(() => {
  process.env.ALLTERNIT_PHONE_LINK_MESH = '1';
});

import { PhoneLinkManager } from './phone-link.js';
import type { PhoneLinkStatus } from './phone-link-types.js';
import type { AdbCall, AdbResult } from './phone-tools.js';
import { IctrlError } from './ictrl-client.js';
import type { AndroidDriverDeps, AndroidLinkRecord } from './phone-link-android.js';
import type { AndroidHelper } from './phone-android-helper.js';
import type { GoIosLike, IctrlLike, IosDriverDeps, IosLinkRecord } from './phone-link-ios.js';
import type { LongRunning, RunTestOptions } from './phone-goios.js';
import type { RunnerArtifacts, RunnerSigner } from './phone-ios-runner.js';
import type { LinkedPhone } from './phone-link-health.js';
import { PhoneLinkBackend, cachedHostResolver } from './phone-link-backend.js';
import {
  MESH_PENDING_DETAIL,
  PHONE_LINK_MESH_ENABLED,
  meshHostOf,
  pollMeshHost,
  schemaSupportsMeshConfig,
  shellQuote,
} from './phone-link-mesh.js';

const MESH_CONTROL_URL = 'https://mail.news.allternit.com:8444';
const MESH_AUTH_KEY = 'preauth-key-7f3a9c1e52d84b6f';
const MESH_HOST = '100.64.0.16:8200';

/** The real unauthenticated /whoami body once the join is running (facts, live 2026-10-17). */
function runningWhoami(deviceId = '61f06513-9e3a-4c39-8b02-9d4af1c2e801') {
  return {
    device_id: deviceId,
    name: 'moto g play 2024',
    platform: 'android',
    protocol_version: 1,
    server_version: '0.1.0-android',
    mesh_state: 'running',
    mesh_host: MESH_HOST,
  };
}

/** The real GET /schema document shape (Schema.document() in the helper apps). */
function schemaDocument(configParams: { name: string; type: string; required: boolean; description: string }[]) {
  return {
    protocol: 'JSON-RPC 2.0',
    server_version: '0.1.0-android',
    platform: 'android',
    protocol_version: 1,
    capability_tags: [],
    endpoints: { pair: 'POST /pair', rpc: 'POST /rpc', whoami: 'GET /whoami', schema: 'GET /schema', skill: 'GET /skill' },
    methods: [
      { name: 'device_info', description: 'Device info.', params: [], returns: '{device_name}' },
      { name: 'config', description: 'Read or change server settings.', params: configParams, returns: '{config}' },
    ],
  };
}

/** The runner's config params WITHOUT mesh support (what ships until the parallel task lands). */
const CONFIG_PARAMS_WITHOUT_MESH = [
  { name: 'allow_remote_access', type: 'boolean', required: false, description: 'Listen on all interfaces.' },
  { name: 'listen_port', type: 'number', required: false, description: 'TCP port.' },
];

/** The runner's config params WITH mesh support (added by the parallel runner task). */
const CONFIG_PARAMS_WITH_MESH = [
  ...CONFIG_PARAMS_WITHOUT_MESH,
  { name: 'mesh_control_url', type: 'string', required: false, description: 'Headscale control URL for the mesh join (https only).' },
  { name: 'mesh_auth_key', type: 'string', required: false, description: 'Single-use mesh pre-auth key.' },
];

describe('phone-link-mesh helpers (real wire shapes)', () => {
  it('the development switch is on in this file', () => {
    expect(PHONE_LINK_MESH_ENABLED).toBe(true);
  });

  it('meshHostOf accepts only running with a non-empty mesh_host', () => {
    expect(meshHostOf(runningWhoami())).toBe(MESH_HOST);
    expect(meshHostOf({ ...runningWhoami(), mesh_state: 'stopped', mesh_host: null })).toBeNull();
    expect(meshHostOf({ ...runningWhoami(), mesh_state: 'starting', mesh_host: null })).toBeNull();
    expect(meshHostOf({ ...runningWhoami(), mesh_state: 'needs_login', mesh_host: null })).toBeNull();
    expect(meshHostOf({ ...runningWhoami(), mesh_state: 'error:auth_key_rejected', mesh_host: null })).toBeNull();
    expect(meshHostOf({ ...runningWhoami(), mesh_state: 'running', mesh_host: null })).toBeNull();
    expect(meshHostOf(null)).toBeNull();
    expect(meshHostOf({ device_id: 'x' })).toBeNull();
  });

  it('feature-detects mesh_control_url in the real /schema document, both ways', () => {
    expect(schemaSupportsMeshConfig(schemaDocument(CONFIG_PARAMS_WITHOUT_MESH))).toBe(false);
    expect(schemaSupportsMeshConfig(schemaDocument(CONFIG_PARAMS_WITH_MESH))).toBe(true);
    // A runner without a schema (older iOS runner, fetch error) is "no", never an error.
    expect(schemaSupportsMeshConfig(null)).toBe(false);
    expect(schemaSupportsMeshConfig({ methods: 'nope' })).toBe(false);
    expect(schemaSupportsMeshConfig({ methods: [{ name: 'config', params: 'nope' }] })).toBe(false);
    expect(schemaSupportsMeshConfig('garbage')).toBe(false);
  });

  it('shellQuote wraps for the phone shell and escapes embedded single quotes', () => {
    expect(shellQuote(MESH_CONTROL_URL)).toBe(`'${MESH_CONTROL_URL}'`);
    expect(shellQuote("a'b")).toBe(`'a'\\''b'`);
  });

  it('pollMeshHost resolves the mesh host once whoami reports running, and null on timeout', async () => {
    let polls = 0;
    let t = 0;
    const host = await pollMeshHost(
      async () => (++polls < 3 ? { ...runningWhoami(), mesh_state: 'starting', mesh_host: null } : runningWhoami()),
      '10.0.0.7:8200',
      { sleep: (ms) => { t += ms; return Promise.resolve(); }, now: () => t },
    );
    expect(host).toBe(MESH_HOST);
    expect(polls).toBe(3);

    t = 0;
    polls = 0;
    const timedOut = await pollMeshHost(
      async () => (++polls, { ...runningWhoami(), mesh_state: 'starting', mesh_host: null }),
      '10.0.0.7:8200',
      { sleep: (ms) => { t += ms; return Promise.resolve(); }, now: () => t },
    );
    expect(timedOut).toBeNull();
    expect(polls).toBeGreaterThan(0);
  });
});

// ── Android setup with the mesh switch on ────────────────────────────────────

const ADB_DEVICE_ID = '61f06513-9e3a-4c39-8b02-9d4af1c2e801';
const ADB_LAN = '10.0.0.7:8200';
const PKG = 'com.allternit.phonelink';
const HELPER: AndroidHelper = {
  apk: '/helpers/phone-link.apk',
  manifest: { version: '1.0.0', versionCode: 7, apk: { file: 'p.apk', sha256: 'a'.repeat(64), size: 1 } },
};

interface AdbWorld {
  t: number;
  /** What the app's LAN /whoami reports while setup polls for the join. */
  whoamiMesh: { device_id?: string; mesh_state?: string; mesh_host?: string | null } | null;
  enrollError: Error | null;
  enrollCalls: number;
  logs: string[];
  ran: string[];
  broadcasts: string[];
  saved: AndroidLinkRecord[];
}

function adbWorld(over: Partial<AdbWorld> = {}): AdbWorld {
  return {
    t: 0,
    whoamiMesh: runningWhoami(ADB_DEVICE_ID),
    enrollError: null,
    enrollCalls: 0,
    logs: [],
    ran: [],
    broadcasts: [],
    saved: [],
    ...over,
  };
}

function meshFakeAdb(w: AdbWorld): AdbCall {
  const call = (async (args: string[]): Promise<AdbResult> => {
    const cmd = args.join(' ');
    const inner = cmd.startsWith('shell ') ? cmd.slice('shell '.length) : cmd;
    w.ran.push(cmd);
    const out = (stdout: string, code = 0): AdbResult => ({ stdout, stderr: '', code });
    if (cmd === 'devices -l') return out('List of devices attached\nR5CT1 device usb:1 model:moto_g_play_2024\n');
    if (inner.includes('ro.product.model')) return out('moto g play 2024\n');
    if (inner.includes('ro.build.version.release')) return out('14\n');
    if (inner.startsWith('dumpsys package')) return out('');
    if (cmd.startsWith('install -r -g')) return out('Success\n');
    if (inner === 'settings get secure enabled_accessibility_services') return out('null\n');
    if (inner.startsWith('am broadcast')) {
      w.broadcasts.push(inner);
      return out(`Broadcast completed: result=0, data="provisioned device_id=${ADB_DEVICE_ID} name='moto g play 2024' remote=true port=8200 armed=true"`);
    }
    if (inner.startsWith('dumpsys accessibility')) {
      return out('     Bound services:{Service[label=Allternit Phone Link, capabilities=161]}\n     Enabled services:{{com.allternit.phonelink/com.allternit.phonelink.PhoneLinkAccessibilityService}}\n');
    }
    if (cmd === 'forward tcp:0 tcp:8200') return out('52123\n');
    if (cmd.startsWith('forward --remove')) return out('');
    if (inner.includes('wlan0')) return out('inet 10.0.0.7/24 brd 10.0.0.255 scope global wlan0');
    return out('');
  }) as AdbCall;
  call.binary = async () => Buffer.alloc(0);
  return call;
}

function adbDeps(w: AdbWorld): AndroidDriverDeps {
  const records = new Map<string, AndroidLinkRecord>();
  const makeIctrl = (host: string): IctrlLike => ({
    host,
    hasKey: () => true,
    forget: () => {},
    whoami: async () => (host.startsWith('127.0.0.1') ? { device_id: ADB_DEVICE_ID } : null),
    pair: async () => {},
    call: async <T,>(method: string): Promise<T> => {
      if (method === 'device_info') {
        if (host !== ADB_LAN) throw new IctrlError('unreachable', 'unreachable');
        return { device_id: ADB_DEVICE_ID } as T;
      }
      if (method === 'list_apps') return { apps: [{ package: 'com.openai.chatgpt' }] } as T;
      return {} as T;
    },
    withHost: (h) => makeIctrl(h),
  });
  return {
    adb: async () => meshFakeAdb(w),
    helper: async () => HELPER,
    dataDir: '/data/phone',
    ictrl: (_storeKey, host) => makeIctrl(host),
    discovery: { lanInterfaces: () => [], fetchWhoami: async () => w.whoamiMesh },
    mesh: {
      enroll: async () => {
        w.enrollCalls++;
        if (w.enrollError) throw w.enrollError;
        return { controlUrl: MESH_CONTROL_URL, authKey: MESH_AUTH_KEY };
      },
    },
    store: {
      load: (id) => records.get(id) ?? null,
      all: () => [...records.values()],
      save: (r) => {
        records.set(r.deviceId, r);
        w.saved.push(r);
      },
    },
    now: () => w.t,
    logger: (m) => w.logs.push(m),
  };
}

function startAndroid(w: AdbWorld): { manager: PhoneLinkManager; statuses: PhoneLinkStatus[] } {
  const manager = new PhoneLinkManager({
    sleep: (ms) => {
      w.t += ms;
      return Promise.resolve();
    },
    android: async () => adbDeps(w),
  });
  const statuses: PhoneLinkStatus[] = [];
  manager.on('status', (s) => statuses.push(s));
  manager.start('android');
  return { manager, statuses };
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

/** Everywhere a secret must never appear: driver logs, all emitted statuses, the saved record. */
function leakedSecret(w: { logs: string[]; saved: AndroidLinkRecord[] }, statuses: PhoneLinkStatus[]): string {
  const haystack = [w.logs.join('\n'), JSON.stringify(statuses), JSON.stringify(w.saved)];
  return haystack.find((h) => h.includes(MESH_AUTH_KEY)) ?? '';
}

describe('Android setup enrolls the phone on the mesh (switch on)', () => {
  it('re-provisions with the mesh extras, saves meshHost, and finishes ready_wifi', async () => {
    const w = adbWorld();
    const { manager, statuses } = startAndroid(w);
    const ready = await waitFor(manager, 'ready_wifi');

    expect(w.enrollCalls).toBe(1);
    // The second PROVISION carries the mesh extras, single-quoted for the
    // phone shell, pairing NOT re-armed.
    expect(w.broadcasts).toHaveLength(2);
    const meshProvision = w.broadcasts[1];
    expect(meshProvision).toContain(`-a ${PKG}.PROVISION`);
    expect(meshProvision).toContain('--ez arm_pairing false');
    expect(meshProvision).toContain('--ez allow_remote_access true');
    expect(meshProvision).toContain('--ei listen_port 8200');
    expect(meshProvision).toContain(`--es mesh_control_url '${MESH_CONTROL_URL}'`);
    expect(meshProvision).toContain(`--es mesh_auth_key '${MESH_AUTH_KEY}'`);
    // The join finished: normal detail, meshHost on the record, no pending flag.
    expect(ready.detail).toBe(ADB_LAN);
    expect(w.saved.at(-1)).toMatchObject({ deviceId: ADB_DEVICE_ID, lanHost: ADB_LAN, meshHost: MESH_HOST });
    expect(w.saved.at(-1)).not.toHaveProperty('meshPending');
    // The single-use key never reached a log line, a status, or the record.
    expect(leakedSecret(w, statuses)).toBe('');
  });

  it('enrollment failure (not signed in) still finishes ready_wifi with meshPending', async () => {
    const w = adbWorld({ enrollError: new Error('Desktop is not signed in to an Allternit account.') });
    const { manager, statuses } = startAndroid(w);
    const ready = await waitFor(manager, 'ready_wifi');

    expect(ready.state).toBe('ready_wifi');
    expect(ready.detail).toBe(MESH_PENDING_DETAIL);
    expect(w.broadcasts).toHaveLength(1); // only the original provision — no mesh extras sent
    expect(w.saved.at(-1)).toMatchObject({ meshPending: true });
    expect(w.saved.at(-1)).not.toHaveProperty('meshHost');
    expect(leakedSecret(w, statuses)).toBe('');
  });

  it('a join that never reports running times out to meshPending, never failing setup', async () => {
    const w = adbWorld({ whoamiMesh: { ...runningWhoami(ADB_DEVICE_ID), mesh_state: 'starting', mesh_host: null } });
    const { manager, statuses } = startAndroid(w);
    const ready = await waitFor(manager, 'ready_wifi');

    expect(ready.state).toBe('ready_wifi');
    expect(ready.detail).toBe(MESH_PENDING_DETAIL);
    expect(w.enrollCalls).toBe(1);
    expect(w.broadcasts).toHaveLength(2);
    expect(w.saved.at(-1)).toMatchObject({ meshPending: true });
    expect(w.saved.at(-1)).not.toHaveProperty('meshHost');
    expect(w.t).toBeGreaterThanOrEqual(60_000); // the full join budget was spent polling
    expect(leakedSecret(w, statuses)).toBe('');
  });
});

// ── iPhone setup with the mesh switch on ─────────────────────────────────────

const LAN = '192.168.1.20:8200';
const IOS_DEVICE_ID = 'dev-ios-mesh-1';

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

interface IosWorld {
  schema: unknown;
  enrollError: Error | null;
  enrollCalls: number;
  meshState: string;
  meshHost: string | null;
  /** False = the runner accepts the config but the join never comes up. */
  meshJoins: boolean;
  logs: string[];
  saved: IosLinkRecord[];
  calls: { host: string; method: string; params?: Record<string, unknown> }[];
  schemaHosts: string[];
}

function iosWorld(over: Partial<IosWorld> = {}): IosWorld {
  return {
    schema: schemaDocument(CONFIG_PARAMS_WITH_MESH),
    enrollError: null,
    enrollCalls: 0,
    meshState: 'stopped',
    meshHost: null,
    meshJoins: true,
    logs: [],
    saved: [],
    calls: [],
    schemaHosts: [],
    ...over,
  };
}

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

function meshFakeGoIos(): GoIosLike {
  let rebootPending = false;
  let devmodeChecks = 0;
  return {
    list: async () => {
      if (rebootPending) {
        rebootPending = false;
        return [];
      }
      return ['U'];
    },
    info: async () => ({ ok: true, name: 'Eoj iPhone', version: '26.6.2' }),
    pair: async () => ({ ok: true }),
    devmodeEnabled: async () => {
      devmodeChecks++;
      if (devmodeChecks === 1) {
        rebootPending = true;
        return false;
      }
      return true;
    },
    devmodeReveal: async () => {},
    tunnelStart: () => proc(),
    tunnels: async () => [{ udid: 'U' }],
    imageAuto: async () => {},
    install: async () => {},
    apps: async () => ['com.openai.chat'],
    launch: async (_u, bundleId) => {
      if (bundleId === ARTIFACTS.manifest.host.bundleId && !launches.host) {
        launches.host = true;
        return { ok: false, error: 'profile has not been explicitly trusted', untrusted: true, notInstalled: false };
      }
      return { ok: true };
    },
    kill: async () => {},
    pushToApp: async () => {},
    runtest: (opts: RunTestOptions) => {
      void opts;
      cableSession = true;
      const p = proc();
      const stop = p.stop;
      p.stop = () => {
        stop();
        cableSession = false;
      };
      return p;
    },
  };
}

// Harness-owned mutable state the fakes close over (mirrors phone-link-ios.test.ts).
let cableSession = false;
const launches: { host: boolean } = { host: false };

function resetHarness(): void {
  cableSession = false;
  launches.host = false;
}

function meshFakeIctrl(w: IosWorld): (udid: string, host: string) => IctrlLike {
  const make = (host: string): IctrlLike => ({
    host,
    hasKey: () => true,
    forget: () => {},
    whoami: async () =>
      host.startsWith('127.0.0.1') && cableSession
        ? {
            device_id: IOS_DEVICE_ID,
            name: 'Eoj iPhone',
            platform: 'ios',
            protocol_version: 1,
            server_version: '1.98.12',
            mesh_state: w.meshState,
            mesh_host: w.meshHost,
          }
        : null,
    pair: async () => {},
    call: async <T,>(method: string, params?: Record<string, unknown>): Promise<T> => {
      w.calls.push({ host, method, params });
      const viaCable = host.startsWith('127.0.0.1');
      const lanUp = !viaCable; // the relaunched standalone runner answers on the LAN
      if (viaCable ? !cableSession : !lanUp) throw new IctrlError('unreachable', 'unreachable');
      if (method === 'config') {
        // The runner applies the mesh credentials and joins (live: < 10 s);
        // the fake joins instantly on receipt unless the test says it doesn't.
        if (w.meshJoins && params && typeof params.mesh_control_url === 'string') {
          w.meshState = 'running';
          w.meshHost = MESH_HOST;
        }
        return { allow_remote_access: true, address: LAN, listen_port: 8200 } as T;
      }
      return {} as T;
    },
    withHost: (h) => make(h),
  });
  return (_udid, host) => make(host);
}

function localSigner(): RunnerSigner {
  return {
    kind: 'local-identity',
    needsAppleId: false,
    sign: async () => ({
      runnerIpa: '/s/runner.ipa',
      hostIpa: '/s/host.ipa',
      bundlePrefix: 'com.allternit.phonelink',
      runnerBundleId: ARTIFACTS.manifest.runner.bundleId,
      hostBundleId: ARTIFACTS.manifest.host.bundleId,
      teamId: 'TEAM123456',
      expiresAt: new Date(Date.now() + 7 * 86400_000).toISOString(),
    }),
  };
}

function iosDeps(w: IosWorld): IosDriverDeps {
  const records = new Map<string, IosLinkRecord>();
  return {
    goios: meshFakeGoIos(),
    artifacts: async () => ARTIFACTS,
    signer: localSigner(),
    ictrl: meshFakeIctrl(w),
    forward: async () => ({ port: 50123, close: async () => {} }),
    ddiDir: async () => '/data/ddi',
    store: {
      load: (u) => records.get(u) ?? null,
      all: () => [...records.values()],
      save: (r) => {
        records.set(r.udid, r);
        w.saved.push(r);
      },
    },
    mesh: {
      enroll: async () => {
        w.enrollCalls++;
        if (w.enrollError) throw w.enrollError;
        return { controlUrl: MESH_CONTROL_URL, authKey: MESH_AUTH_KEY };
      },
    },
    fetchSchema: async (host) => {
      w.schemaHosts.push(host);
      return w.schema;
    },
    workDir: '/tmp/work',
    clientName: 'Allternit Desktop (test)',
    logger: (m) => w.logs.push(m),
  };
}

function startIos(w: IosWorld): { manager: PhoneLinkManager; statuses: PhoneLinkStatus[] } {
  const manager = new PhoneLinkManager({
    platform: 'darwin',
    sleep: () => Promise.resolve(),
    ios: async () => iosDeps(w),
  });
  const statuses: PhoneLinkStatus[] = [];
  manager.on('status', (s) => statuses.push(s));
  manager.start('ios');
  return { manager, statuses };
}

describe('iPhone setup enrolls the runner on the mesh (switch on, runner supports it)', () => {
  it('feature-detects via /schema, sends one sealed config, saves meshHost', async () => {
    resetHarness();
    const w = iosWorld();
    const { manager, statuses } = startIos(w);
    await waitFor(manager, 'trust_certificate');
    manager.continue();
    const ready = await waitFor(manager, 'ready_wifi');

    expect(ready.state).toBe('ready_wifi');
    expect(ready.detail).toBe(LAN);
    // /schema was asked on the cable runner only, before the credentials went out.
    expect(w.schemaHosts.length).toBeGreaterThan(0);
    expect(w.schemaHosts.every((h) => h.startsWith('127.0.0.1:'))).toBe(true);
    expect(w.enrollCalls).toBe(1);
    // Exactly one sealed config carried the mesh credentials.
    const meshConfigs = w.calls.filter((c) => c.method === 'config' && c.params && 'mesh_control_url' in c.params);
    expect(meshConfigs).toHaveLength(1);
    expect(meshConfigs[0].host).toMatch(/^127\.0\.0\.1:/);
    expect(meshConfigs[0].params).toEqual({ mesh_control_url: MESH_CONTROL_URL, mesh_auth_key: MESH_AUTH_KEY });
    // The Remote Access config still ran afterwards, untouched by the mesh params.
    const remoteConfigs = w.calls.filter((c) => c.method === 'config' && c.params && 'allow_remote_access' in c.params);
    expect(remoteConfigs).toHaveLength(1);
    expect(w.saved.at(-1)).toMatchObject({ udid: 'U', lanHost: LAN, meshHost: MESH_HOST });
    expect(w.saved.at(-1)).not.toHaveProperty('meshPending');
    // The single-use key never reached a log line, a status, or the record.
    const haystack = [w.logs.join('\n'), JSON.stringify(statuses), JSON.stringify(w.saved)];
    expect(haystack.some((h) => h.includes(MESH_AUTH_KEY))).toBe(false);
  });

  it('a runner whose /schema lacks mesh_control_url is left untouched', async () => {
    resetHarness();
    const w = iosWorld({ schema: schemaDocument(CONFIG_PARAMS_WITHOUT_MESH) });
    const { manager } = startIos(w);
    await waitFor(manager, 'trust_certificate');
    manager.continue();
    await waitFor(manager, 'ready_wifi');

    expect(w.enrollCalls).toBe(0); // no key minted for a runner that can't use it
    expect(w.calls.filter((c) => c.method === 'config' && c.params && 'mesh_control_url' in c.params)).toHaveLength(0);
    expect(w.saved.at(-1)).not.toHaveProperty('meshHost');
    expect(w.saved.at(-1)).not.toHaveProperty('meshPending');
  });

  it('enrollment failure (not signed in) still finishes ready_wifi with meshPending', async () => {
    resetHarness();
    const w = iosWorld({ enrollError: new Error('Desktop is not signed in to an Allternit account.') });
    const { manager, statuses } = startIos(w);
    await waitFor(manager, 'trust_certificate');
    manager.continue();
    const ready = await waitFor(manager, 'ready_wifi');

    expect(ready.state).toBe('ready_wifi');
    expect(ready.detail).toBe(MESH_PENDING_DETAIL);
    expect(w.calls.filter((c) => c.method === 'config' && c.params && 'mesh_control_url' in c.params)).toHaveLength(0);
    expect(w.saved.at(-1)).toMatchObject({ meshPending: true });
    expect(w.saved.at(-1)).not.toHaveProperty('meshHost');
    const haystack = [w.logs.join('\n'), JSON.stringify(statuses)];
    expect(haystack.some((h) => h.includes(MESH_AUTH_KEY))).toBe(false);
  });

  it('a join that never reports running times out to meshPending, never failing setup', async () => {
    resetHarness();
    // The runner accepts the config but the join never comes up.
    const w = iosWorld({ meshState: 'starting', meshJoins: false });
    const deps = iosDeps(w);
    const t = { now: 0 };
    const manager = new PhoneLinkManager({
      platform: 'darwin',
      sleep: (ms) => {
        t.now += ms;
        return Promise.resolve();
      },
      ios: async () => ({ ...deps, now: () => t.now }),
    });
    const statuses: PhoneLinkStatus[] = [];
    manager.on('status', (s) => statuses.push(s));
    manager.start('ios');
    await waitFor(manager, 'trust_certificate');
    manager.continue();
    const ready = await waitFor(manager, 'ready_wifi');

    expect(ready.state).toBe('ready_wifi');
    expect(ready.detail).toBe(MESH_PENDING_DETAIL);
    expect(t.now).toBeGreaterThanOrEqual(60_000); // the full join budget was spent polling
    expect(w.saved.at(-1)).toMatchObject({ meshPending: true });
    expect(w.saved.at(-1)).not.toHaveProperty('meshHost');
    const haystack = [w.logs.join('\n'), JSON.stringify(statuses)];
    expect(haystack.some((h) => h.includes(MESH_AUTH_KEY))).toBe(false);
  });
});

// ── The lane reaches phones over the mesh ────────────────────────────────────

const phone: LinkedPhone = {
  deviceId: ADB_DEVICE_ID,
  platform: 'android',
  name: 'moto g play 2024',
  lanHost: '192.168.1.50:8200',
  meshHost: MESH_HOST,
  storeKey: 'k',
};

describe('cachedHostResolver mesh fallback', () => {
  type Who = { device_id?: string; mesh_state?: string; mesh_host?: string | null } | null;
  const meshDiscovery = (answers: Record<string, Who>) => ({
    lanInterfaces: () => [{ address: '192.168.1.5', netmask: '255.255.255.0' }],
    fetchWhoami: async (host: string): Promise<Who> => answers[host] ?? null,
  });

  it('prefers the LAN and never dials the mesh', async () => {
    const meshProxy = vi.fn(async () => '127.0.0.1:41000');
    const resolve = cachedHostResolver({
      discovery: meshDiscovery({ '192.168.1.50:8200': { device_id: ADB_DEVICE_ID } }),
      remember: () => {},
      meshProxy,
    });
    expect(await resolve(phone)).toBe('192.168.1.50:8200');
    expect(resolve.lastPath?.(ADB_DEVICE_ID)).toBe('lan');
    expect(meshProxy).not.toHaveBeenCalled();
  });

  it('finds a moved phone on the LAN before trying the mesh', async () => {
    const meshProxy = vi.fn(async () => '127.0.0.1:41000');
    const resolve = cachedHostResolver({
      discovery: meshDiscovery({ '192.168.1.60:8200': { device_id: ADB_DEVICE_ID } }),
      remember: () => {},
      meshProxy,
    });
    expect(await resolve({ ...phone, lanHost: '192.168.1.50:8200' })).toBe('192.168.1.60:8200');
    expect(resolve.lastPath?.(ADB_DEVICE_ID)).toBe('lan');
    expect(meshProxy).not.toHaveBeenCalled();
  });

  it('falls back to the mesh proxy when the LAN has the phone nowhere, and does not write the proxy back', async () => {
    const remembered: string[] = [];
    const resolve = cachedHostResolver({
      discovery: meshDiscovery({ '127.0.0.1:41000': { device_id: ADB_DEVICE_ID, mesh_state: 'running', mesh_host: MESH_HOST } }),
      remember: (_p, h) => remembered.push(h),
      meshProxy: async (meshHost) => {
        expect(meshHost).toBe(MESH_HOST);
        return '127.0.0.1:41000';
      },
    });
    // Saved LAN host answers with a different device; the scan finds nothing.
    expect(await resolve({ ...phone, lanHost: '192.168.1.99:8200' })).toBe('127.0.0.1:41000');
    expect(resolve.lastPath?.(ADB_DEVICE_ID)).toBe('mesh');
    expect(remembered).toEqual([]); // the ephemeral loopback port must not become the lanHost
  });

  it('rejects a mesh proxy that answers with another device id', async () => {
    const resolve = cachedHostResolver({
      discovery: meshDiscovery({ '127.0.0.1:41000': { device_id: 'someone-else', mesh_state: 'running', mesh_host: '100.64.0.99:8200' } }),
      remember: () => {},
      meshProxy: async () => '127.0.0.1:41000',
    });
    expect(await resolve({ ...phone, lanHost: '192.168.1.99:8200' })).toBeNull();
    expect(resolve.lastPath?.(ADB_DEVICE_ID)).toBeUndefined();
  });

  it('stays offline without a meshProxy even when meshHost is saved (switch-off wiring)', async () => {
    const resolve = cachedHostResolver({
      discovery: meshDiscovery({}),
      remember: () => {},
    });
    expect(await resolve({ ...phone, lanHost: '192.168.1.99:8200' })).toBeNull();
    expect(resolve.lastPath?.(ADB_DEVICE_ID)).toBeUndefined();
  });

  it('does not use a meshHost the record does not have', async () => {
    const meshProxy = vi.fn(async () => '127.0.0.1:41000');
    const resolve = cachedHostResolver({ discovery: meshDiscovery({}), remember: () => {}, meshProxy });
    expect(await resolve({ ...phone, lanHost: '192.168.1.99:8200', meshHost: undefined })).toBeNull();
    expect(meshProxy).not.toHaveBeenCalled();
  });
});

describe('phone.devices path field', () => {
  it("entries carry path: 'mesh' after the resolver reached the phone over the mesh", async () => {
    const resolve = cachedHostResolver({
      discovery: {
        lanInterfaces: () => [],
        fetchWhoami: async (host) =>
          host === '127.0.0.1:41000' ? { device_id: ADB_DEVICE_ID, mesh_state: 'running', mesh_host: MESH_HOST } : null,
      },
      remember: () => {},
      meshProxy: async () => '127.0.0.1:41000',
    });
    expect(await resolve({ ...phone, lanHost: '192.168.1.99:8200' })).toBe('127.0.0.1:41000');
    const backend = new PhoneLinkBackend({ phones: () => [phone], client: () => ({}) as never, resolveHost: resolve });
    const devices = await backend.linkedDevices();
    expect(devices[0]).toMatchObject({ serial: `link:${ADB_DEVICE_ID}`, path: 'mesh' });
  });

  it('entries carry path: lan after a LAN resolution', async () => {
    const resolve = cachedHostResolver({
      discovery: { lanInterfaces: () => [], fetchWhoami: async () => ({ device_id: ADB_DEVICE_ID }) },
      remember: () => {},
    });
    expect(await resolve(phone)).toBe('192.168.1.50:8200');
    const backend = new PhoneLinkBackend({ phones: () => [phone], client: () => ({}) as never, resolveHost: resolve });
    const devices = await backend.linkedDevices();
    expect(devices[0]).toMatchObject({ path: 'lan' });
  });

  it('omits path when the phone was never resolved (plain resolver)', async () => {
    const backend = new PhoneLinkBackend({ phones: () => [phone], client: () => ({}) as never });
    const devices = await backend.linkedDevices();
    expect(devices[0]).not.toHaveProperty('path');
  });
});
