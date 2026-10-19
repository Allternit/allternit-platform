import { describe, expect, it } from 'vitest';
import {
  ScreenServiceKeeper,
  SCREEN_SERVICE_KILL_LINE,
  SCREEN_SERVICE_MIN_GAP_MS,
  SCREEN_SERVICE_RECHECK_MS,
  matchLinkForAdb,
  serialFromMdns,
  validateLaunch,
  type AdbScreenPhone,
  type ScreenServiceLink,
} from './phone-screen-service.js';

const TOKEN = 'ab'.repeat(32);
const APK = '/data/app/~~Ab12-_==/com.allternit.phonelink-Xy_9==/base.apk';
const START = `setsid app_process -Djava.class.path=${APK} / com.allternit.phonelink.shell.ShellServerMain ${TOKEN} 18210 </dev/null >/data/local/tmp/allternit-shell.log 2>&1 &`;
const LAUNCH = { kill: SCREEN_SERVICE_KILL_LINE, start: START };

describe('validateLaunch', () => {
  it('accepts exactly the two lines the app builds', () => {
    expect(validateLaunch(LAUNCH)).toEqual(LAUNCH);
  });

  it('refuses anything else', () => {
    const bad = [
      null,
      'x',
      { kill: 'reboot', start: START },
      { kill: SCREEN_SERVICE_KILL_LINE, start: `${START}; reboot` },
      { kill: SCREEN_SERVICE_KILL_LINE, start: START.replace(TOKEN, 'zz'.repeat(32)) },
      { kill: SCREEN_SERVICE_KILL_LINE, start: START.replace(APK, '/data/app/x;reboot/base.apk') },
      { kill: SCREEN_SERVICE_KILL_LINE, start: START.replace(APK, '/data/app/$(id)/base.apk') },
      { kill: SCREEN_SERVICE_KILL_LINE, start: START.replace(APK, '/data/app/../../sdcard/base.apk') },
      { kill: SCREEN_SERVICE_KILL_LINE, start: START.replace(APK, '/sdcard/base.apk') },
      { kill: SCREEN_SERVICE_KILL_LINE, start: START.replace('18210', '80') },
      { kill: SCREEN_SERVICE_KILL_LINE, start: START.replace('allternit-shell.log', 'x.log') },
      { kill: SCREEN_SERVICE_KILL_LINE, start: `${START}\nreboot` },
    ];
    for (const launch of bad) expect(validateLaunch(launch)).toBeNull();
  });
});

describe('matchLinkForAdb', () => {
  const links: ScreenServiceLink[] = [
    { deviceId: 'dev-pixel', serial: '2A111FDH200ABC', deviceName: 'Google Pixel 8' },
    { deviceId: 'dev-sam', serial: 'R58N123', deviceName: 'samsung SM-S918B' },
  ];

  it('matches by USB serial, ro.serialno or the serial in an mDNS name', () => {
    expect(matchLinkForAdb({ serial: '2A111FDH200ABC' }, links)?.deviceId).toBe('dev-pixel');
    expect(matchLinkForAdb({ serial: '192.168.1.20:41234', hardwareSerial: 'R58N123' }, links)?.deviceId).toBe('dev-sam');
    expect(serialFromMdns('adb-R58N123-AbCdEf._adb-tls-connect._tcp')).toBe('R58N123');
    expect(matchLinkForAdb({ serial: 'adb-R58N123-AbCdEf._adb-tls-connect._tcp' }, links)?.deviceId).toBe('dev-sam');
  });

  it('falls back to the model only when exactly one record matches', () => {
    expect(matchLinkForAdb({ serial: '10.0.0.5:5555', model: 'Pixel_8' }, links)?.deviceId).toBe('dev-pixel');
    const twins = [...links, { deviceId: 'dev-pixel-2', serial: 'OTHER', deviceName: 'Google Pixel 8' }];
    expect(matchLinkForAdb({ serial: '10.0.0.5:5555', model: 'Pixel 8' }, twins)).toBeNull();
    expect(matchLinkForAdb({ serial: '10.0.0.5:5555' }, links)).toBeNull();
  });
});

function harness(opts: { statuses: unknown[]; phones?: () => AdbScreenPhone[]; links?: ScreenServiceLink[] }) {
  let t = 1_000_000;
  const shells: string[] = [];
  const calls: string[] = [];
  const logs: string[] = [];
  const statuses = [...opts.statuses];
  const keeper = new ScreenServiceKeeper({
    adbPhones: async () => opts.phones?.() ?? [{ serial: 'R58N123' }],
    links: () => opts.links ?? [{ deviceId: 'dev-sam', serial: 'R58N123', deviceName: 'Galaxy' }],
    helperCall: async (deviceId, method) => {
      calls.push(`${deviceId} ${method}`);
      const next = statuses.length > 1 ? statuses.shift() : statuses[0];
      if (next instanceof Error) throw next;
      return next;
    },
    adbShell: async (serial, line) => {
      shells.push(`${serial} ${line}`);
      return { code: line.startsWith('pkill') ? 1 : 0, stdout: '', stderr: '' };
    },
    now: () => t,
    sleep: async () => {},
    log: (m) => logs.push(m),
  });
  return { keeper, shells, calls, logs, advance: (ms: number) => (t += ms) };
}

const down = { running: false, state: 'waiting_for_computer', lastError: 'Waiting', launch: LAUNCH };
const up = { running: true, state: 'running', lastError: null, launch: null };

describe('ScreenServiceKeeper', () => {
  it('starts a stopped service with kill then start, and confirms it', async () => {
    const h = harness({ statuses: [down, up] });
    await h.keeper.pass();
    expect(h.shells).toEqual([`R58N123 ${SCREEN_SERVICE_KILL_LINE}`, `R58N123 ${START}`]);
    expect(h.calls).toEqual(['dev-sam screen_service', 'dev-sam screen_service']);
    expect(h.logs).toHaveLength(1);
    expect(h.logs[0]).toContain('started over adb');
  });

  it('leaves a running service alone', async () => {
    const h = harness({ statuses: [up] });
    await h.keeper.pass();
    expect(h.shells).toEqual([]);
    expect(h.logs[0]).toContain('running');
  });

  it('never runs lines that fail validation', async () => {
    const h = harness({ statuses: [{ ...down, launch: { kill: SCREEN_SERVICE_KILL_LINE, start: 'reboot' } }] });
    await h.keeper.pass();
    expect(h.shells).toEqual([]);
    expect(h.logs[0]).toContain('no valid launch lines');
  });

  it('logs an unreachable phone and does nothing over adb', async () => {
    const h = harness({ statuses: [new Error('offline')] });
    await h.keeper.pass();
    expect(h.shells).toEqual([]);
    expect(h.logs[0]).toContain("didn't answer");
  });

  it('ignores adb phones that are not linked Androids', async () => {
    const h = harness({ statuses: [down], phones: () => [{ serial: 'SOMEONE-ELSE' }] });
    await h.keeper.pass();
    expect(h.calls).toEqual([]);
  });

  it('checks on connect, then every 5 min, never more than once per 30 s', async () => {
    const h = harness({ statuses: [up] });
    await h.keeper.pass();
    expect(h.calls).toHaveLength(1);
    h.advance(SCREEN_SERVICE_MIN_GAP_MS + 1);
    await h.keeper.pass(); // still online: not due yet
    expect(h.calls).toHaveLength(1);
    h.advance(SCREEN_SERVICE_RECHECK_MS);
    await h.keeper.pass();
    expect(h.calls).toHaveLength(2);
  });

  it('a reconnect is checked right away, or right after the 30 s gap', async () => {
    let phones: AdbScreenPhone[] = [{ serial: 'R58N123' }];
    const h = harness({ statuses: [up], phones: () => phones });
    await h.keeper.pass();
    expect(h.calls).toHaveLength(1);
    phones = [];
    await h.keeper.pass();
    phones = [{ serial: 'R58N123' }];
    await h.keeper.pass(); // reconnect within 30 s: not yet
    expect(h.calls).toHaveLength(1);
    h.advance(SCREEN_SERVICE_MIN_GAP_MS + 1);
    await h.keeper.pass(); // first pass after the gap: checked, not 5 min later
    expect(h.calls).toHaveLength(2);
  });
});
