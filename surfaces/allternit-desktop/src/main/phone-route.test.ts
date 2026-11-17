import { describe, expect, it } from 'vitest';
import { needsYou, NEEDS_YOU_CODES, NEEDS_YOU_SENTENCES, type RouteHealth } from './phone-link-health.js';
import { IOS_HELPER_RETIRED_INSTRUCTION, PhoneLinkManager } from './phone-link.js';
import type { AppRpcResult } from './phone-app-store.js';
import {
  approvalClass,
  deriveActionKind,
  FALLBACK_FRAME_STALE,
  FALLBACK_FRAME_WAIT_MS,
  legacyHelperLane,
  officialLane,
  officialResult,
  PhoneRouter,
  ROUTE_ORDER,
  usbAgentLane,
  type Capability,
  type LaneTool,
  type PhoneLane,
  type RouteId,
} from './phone-route.js';
import type { AppPhoneLane, PhoneLinkBackend, ToolResult } from './phone-tools.js';

const TAP: Capability[] = ['screenshot', 'read_ui', 'tap', 'type', 'swipe', 'key'];
const SERIAL = 'app:iphone-1';

/** A fake route: the transport edge. Records what it was asked to do. */
function fakeLane(id: RouteId, opts: { health?: RouteHealth | (() => RouteHealth); result?: ToolResult | ((tool: LaneTool) => ToolResult); covers?: Capability[] } = {}) {
  const calls: Array<{ tool: LaneTool; args: Record<string, unknown> }> = [];
  let healthChecks = 0;
  const lane: PhoneLane = {
    id,
    covers: opts.covers ?? (id === 'official' ? ['shortcuts', 'actions', 'home'] : TAP),
    verify: 'self',
    health: async () => {
      healthChecks += 1;
      const h = opts.health ?? { state: 'healthy' };
      return typeof h === 'function' ? h() : h;
    },
    act: async (tool, _serial, args) => {
      calls.push({ tool, args });
      const r = opts.result ?? { ok: true, via: id };
      return typeof r === 'function' ? r(tool) : r;
    },
  };
  return { lane, calls, healthChecks: () => healthChecks };
}

const need = (code: Parameters<typeof needsYou>[0]): RouteHealth => ({ state: 'needs_you', needsYou: needsYou(code) });
const off: RouteHealth = { state: 'unavailable', reason: 'not set up' };

describe('route order', () => {
  it('is official first, then mirror, bt-mac, bt-apple, bt-android, usb-agent, legacy-helper', () => {
    expect(ROUTE_ORDER).toEqual(['official', 'mirror', 'bt-mac', 'bt-apple', 'bt-android', 'usb-agent', 'legacy-helper']);
  });

  it('uses official for a shortcut even when a tap route is healthy', async () => {
    const official = fakeLane('official', { result: { ok: true, detail: { output: 'sent', returned_via: 'app_intent' } } });
    const mirror = fakeLane('mirror');
    const router = new PhoneRouter({ lanes: { official: official.lane, mirror: mirror.lane } });
    const res = await router.run('phone.shortcuts.run', SERIAL, { name: 'Note' });
    expect(res).toMatchObject({ ok: true, route: 'official', verified: false, detail: { output: 'sent' } });
    expect(mirror.calls).toHaveLength(0);
  });

  it('picks the first healthy tap route in order and skips routes that are not set up', async () => {
    const mirror = fakeLane('mirror', { health: off });
    const btMac = fakeLane('bt-mac', { health: off });
    const btApple = fakeLane('bt-apple');
    const btAndroid = fakeLane('bt-android');
    const router = new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane, 'bt-apple': btApple.lane, 'bt-android': btAndroid.lane } });
    const res = await router.run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(res).toMatchObject({ ok: true, route: 'bt-apple', via: 'bt-apple' });
    expect(btAndroid.calls).toHaveLength(0);
  });

  it('returns null when no route serves the phone, so the caller keeps its old path', async () => {
    expect(await new PhoneRouter({ lanes: {} }).run('phone.screenshot', SERIAL, {})).toBeNull();
  });

  it('home tools never fall back to a tap route', async () => {
    const mirror = fakeLane('mirror');
    const router = new PhoneRouter({ lanes: { mirror: mirror.lane } });
    expect(await router.run('phone.home.set', SERIAL, { id: 'lamp', state: { on: true } })).toBeNull();
    expect(mirror.calls).toHaveLength(0);
  });
});

describe('needs you', () => {
  it('returns exactly one needs_you — the first in order — when no route is healthy', async () => {
    const mirror = fakeLane('mirror', { health: need('MAC_LOCKED') });
    const btMac = fakeLane('bt-mac', { health: need('BT_NOT_PAIRED') });
    const router = new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane } });
    const res = await router.run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(res).toMatchObject({
      ok: false,
      error: 'needs_you',
      route: 'mirror',
      verified: false,
      needs_you: { code: 'MAC_LOCKED', message: 'Unlock your Mac so I can use your iPhone through it.' },
    });
    expect(mirror.calls).toHaveLength(0);
    expect(btMac.calls).toHaveLength(0);
  });

  it('skips a route that needs the user when a later one is healthy', async () => {
    const mirror = fakeLane('mirror', { health: need('MIRROR_PHONE_IN_USE') });
    const btMac = fakeLane('bt-mac');
    const res = await new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane } }).run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(res).toMatchObject({ ok: true, route: 'bt-mac' });
  });

  it('a pinned route that needs the user returns its needs_you and never falls back', async () => {
    const mirror = fakeLane('mirror', { health: need('MIRROR_WINDOW_HIDDEN') });
    const btMac = fakeLane('bt-mac');
    const router = new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane } });
    const res = await router.run('phone.tap', SERIAL, { x: 1, y: 2 }, { route: 'mirror' });
    expect(res).toMatchObject({ ok: false, route: 'mirror', needs_you: { code: 'MIRROR_WINDOW_HIDDEN' } });
    expect(btMac.calls).toHaveLength(0);
  });

  it('a pinned route that is not set up says so', async () => {
    const res = await new PhoneRouter({ lanes: {} }).run('phone.tap', SERIAL, { x: 1, y: 2 }, { route: 'bt-mac' });
    expect(res).toMatchObject({ ok: false, error: 'route_unavailable', route: 'bt-mac', detail: { error: 'route_unavailable' } });
  });

  it('has one fixed sentence per code, with <name> and <date> filled', () => {
    expect(Object.keys(NEEDS_YOU_SENTENCES)).toEqual([...NEEDS_YOU_CODES]);
    expect(needsYou('SHORTCUT_NOT_INSTALLED', { name: 'Allternit Actions' }).message).toBe('Add the "Allternit Actions" shortcut in Allternit Settings > Device Access > iPhone actions.');
    expect(needsYou('HELPER_RETIRING', { date: '2026-12-16T00:00:00Z' }).message).toContain('before December 16, 2026.');
  });
});

describe('fallback', () => {
  it('a read moves to the next healthy route when one fails', async () => {
    const mirror = fakeLane('mirror', { result: { ok: false, error: 'phone_failed', message: 'capture failed' } });
    const btMac = fakeLane('bt-mac', { result: { ok: true, mimeType: 'image/jpeg', dataBase64: 'x' } });
    const res = await new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane } }).run('phone.screenshot', SERIAL, {});
    expect(res).toMatchObject({ ok: true, route: 'bt-mac', verified: true, dataBase64: 'x' });
  });

  it('an action moves on only when the failed route is no longer healthy', async () => {
    let mirrorUp = true;
    const mirror = fakeLane('mirror', {
      health: () => (mirrorUp ? { state: 'healthy' } : need('MIRROR_WINDOW_HIDDEN')),
      result: () => ((mirrorUp = false), { ok: false, error: 'phone_failed', message: 'window closed' }),
    });
    const btMac = fakeLane('bt-mac');
    const res = await new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane } }).run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(res).toMatchObject({ ok: true, route: 'bt-mac' });
    expect(mirror.calls).toHaveLength(1);
  });

  it('an action on a route that is still healthy is not sent again elsewhere', async () => {
    const mirror = fakeLane('mirror', { result: { ok: false, error: 'phone_failed', message: 'odd answer' } });
    const btMac = fakeLane('bt-mac');
    const res = await new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane } }).run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(res).toMatchObject({ ok: false, route: 'mirror', error: 'phone_failed' });
    expect(btMac.calls).toHaveLength(0);
  });

  it('no_change never moves to another route (the tap may have landed)', async () => {
    const mirror = fakeLane('mirror', { result: { ok: false, error: 'no_change', message: 'unchanged' } });
    const btMac = fakeLane('bt-mac');
    const res = await new PhoneRouter({ lanes: { mirror: mirror.lane, 'bt-mac': btMac.lane } }).run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(res).toMatchObject({ ok: false, error: 'no_change', route: 'mirror' });
    expect(btMac.calls).toHaveLength(0);
  });

  it('a denied approval stops the call on the chosen route, with detail.error "denied"', async () => {
    const official = fakeLane('official');
    const res = await new PhoneRouter({ lanes: { official: official.lane } }).run('phone.actions.run', SERIAL, { app: 'slack', action: 'SendMessageIntent' }, {
      approve: async () => ({ ok: false, error: 'approval_denied', message: 'The owner denied this action.' }),
    });
    expect(res).toMatchObject({ ok: false, error: 'approval_denied', route: 'official', detail: { error: 'denied' } });
    expect(official.calls).toHaveLength(0);
  });

  it('caches health for a short time and checks again after it', async () => {
    let t = 0;
    const mirror = fakeLane('mirror');
    const router = new PhoneRouter({ lanes: { mirror: mirror.lane }, now: () => t });
    await router.run('phone.tap', SERIAL, { x: 1, y: 2 });
    await router.run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(mirror.healthChecks()).toBe(1);
    t = 5_000;
    await router.run('phone.tap', SERIAL, { x: 1, y: 2 });
    expect(mirror.healthChecks()).toBe(2);
  });
});

describe('official route needs the app in front', () => {
  it('opens Allternit through a healthy tap route, then runs the shortcut', async () => {
    let foreground = false;
    const official = fakeLane('official', { health: () => (foreground ? { state: 'healthy' } : need('APP_NOT_FOREGROUND')), result: { ok: true, detail: { output: null } } });
    const btMac = fakeLane('bt-mac', { result: (tool) => ((foreground = tool === 'phone.open_app'), { ok: true }) });
    const res = await new PhoneRouter({ lanes: { official: official.lane, 'bt-mac': btMac.lane } }).run('phone.shortcuts.run', SERIAL, { name: 'Note' });
    expect(btMac.calls).toEqual([{ tool: 'phone.open_app', args: { app: 'Allternit' } }]);
    expect(res).toMatchObject({ ok: true, route: 'official' });
    expect(official.calls).toEqual([{ tool: 'phone.shortcuts.run', args: { name: 'Note' } }]);
  });

  it('with no tap route, says to open the app', async () => {
    const official = fakeLane('official', { health: need('APP_NOT_FOREGROUND') });
    const res = await new PhoneRouter({ lanes: { official: official.lane } }).run('phone.shortcuts.run', SERIAL, { name: 'Note' });
    expect(res).toMatchObject({ ok: false, needs_you: { code: 'APP_NOT_FOREGROUND', message: 'Open the Allternit app on your iPhone.' } });
  });
});

describe('phone.describe', () => {
  it('lists every route with health, covers and needs_you, and never acts', async () => {
    const mirror = fakeLane('mirror', { health: need('MAC_LOCKED') });
    const usb = fakeLane('usb-agent', { covers: ['screenshot', 'tap'] });
    const router = new PhoneRouter({ lanes: { mirror: mirror.lane, 'usb-agent': usb.lane }, locked: async () => true });
    const res = await router.describe(SERIAL);
    expect(res).toMatchObject({ ok: true, route: 'usb-agent', verified: true, detail: { locked: true, foreground_app: null, shortcuts: [], home: { available: false } } });
    const routes = res.detail.routes as Array<Record<string, unknown>>;
    expect(routes.map((r) => r.route)).toEqual([...ROUTE_ORDER]);
    expect(routes[1]).toEqual({ route: 'mirror', healthy: false, available: true, needs_you: 'MAC_LOCKED', message: 'Unlock your Mac so I can use your iPhone through it.', covers: TAP });
    expect(routes[5]).toEqual({ route: 'usb-agent', healthy: true, available: true, covers: ['screenshot', 'tap'] });
    expect(routes[0]).toMatchObject({ route: 'official', healthy: false, available: false });
    expect(mirror.calls).toHaveLength(0);
    expect(usb.calls).toHaveLength(0);
  });
});

describe('wrapped lanes', () => {
  it('usb-agent passes calls through unchanged and is verified by its own screenChanged', async () => {
    const calls: string[] = [];
    const lane = usbAgentLane(() => ({
      has: (s) => s === 'usb:b1',
      serials: () => ['usb:b1'],
      call: async (name) => (calls.push(name), { ok: true, screenChanged: true }),
    }));
    const router = new PhoneRouter({ lanes: { 'usb-agent': lane } });
    expect(await router.run('phone.tap', 'usb:b1', { x: 1, y: 2 })).toMatchObject({ ok: true, route: 'usb-agent', verified: true, screenChanged: true });
    expect(calls).toEqual(['phone.tap']);
    // The board has no element read: read_ui is left to the old path.
    expect(await router.run('phone.read_ui', 'usb:b1', {})).toBeNull();
  });

  function links(platform: 'ios' | 'android', activity: number[] = []): PhoneLinkBackend {
    return {
      has: (s) => s === 'link:dev-1',
      serials: () => ['link:dev-1'],
      call: async () => ({ ok: true, via: 'phone-link' }),
      linkedDevices: async () => [],
      canText: () => platform === 'android',
      noteActivity: (_s, d) => void activity.push(d),
    };
  }

  it('legacy-helper serves linked iPhones only, marking activity as before', async () => {
    const activity: number[] = [];
    const ios = new PhoneRouter({ lanes: { 'legacy-helper': legacyHelperLane(() => links('ios', activity)) } });
    expect(await ios.run('phone.tap', 'link:dev-1', { x: 1, y: 2 })).toMatchObject({ ok: true, route: 'legacy-helper', via: 'phone-link', verified: false });
    expect(activity).toEqual([1, -1]);
    const android = new PhoneRouter({ lanes: { 'legacy-helper': legacyHelperLane(() => links('android')) } });
    expect(await android.run('phone.tap', 'link:dev-1', { x: 1, y: 2 })).toBeNull();
  });

  it('legacy-helper carries the retirement notice once a removal date is set, and still works', async () => {
    const router = new PhoneRouter({ lanes: { 'legacy-helper': legacyHelperLane(() => links('ios'), () => '2026-12-16') } });
    const res = await router.run('phone.screenshot', 'link:dev-1', {});
    expect(res).toMatchObject({ ok: true, route: 'legacy-helper', detail: { notice: { code: 'HELPER_RETIRING' } } });
  });

  it('new iPhone helper setup is refused; a computer with a linked iPhone can still re-pair', () => {
    const fresh = new PhoneLinkManager({ platform: 'darwin', iosHelperSetupAllowed: () => false });
    expect(fresh.start('ios')).toMatchObject({ platform: 'ios', state: 'not_supported', instruction: IOS_HELPER_RETIRED_INSTRUCTION });
    const linked = new PhoneLinkManager({ platform: 'darwin', iosHelperSetupAllowed: () => true, ios: () => new Promise(() => {}) });
    expect(linked.start('ios').state).not.toBe('not_supported');
    linked.cancel();
  });
});

describe('approval classes (contract §8)', () => {
  it('derives an action kind from the intent title or id', () => {
    expect(deriveActionKind('com.slack.SendMessageIntent')).toBe('send');
    expect(deriveActionKind('Create Note')).toBe('create');
    expect(deriveActionKind('open_channel')).toBe('open');
    expect(deriveActionKind('GetWeather')).toBe('read');
    expect(deriveActionKind('PlayPlaylist')).toBe('control');
    expect(deriveActionKind('Summarize')).toBe('other');
  });

  it('sends, purchases and security changes need the owner; own-data actions do not', () => {
    expect(approvalClass('phone.actions.run', { app: 'slack', action: 'SendMessageIntent' })).toBe('send');
    expect(approvalClass('phone.actions.run', { app: 'whatsapp', action: 'StartCall' })).toBe('send');
    expect(approvalClass('phone.actions.run', { app: 'slack', action: 'SearchMessages' })).toBeNull();
    expect(approvalClass('phone.actions.run', { app: 'doordash', action: 'ReorderLast' })).toBe('purchase');
    expect(approvalClass('phone.actions.run', { app: 'allternit', action: 'notes.create' })).toBeNull();
    expect(approvalClass('phone.actions.run', { app: 'allternit', action: 'ChangePasscode' })).toBe('security');
    // A catalog entry marked guarded is guarded whatever its title says.
    expect(approvalClass('phone.actions.run', { app: 'notion', action: 'Summarize' }, { action: { kind: 'other', guarded: true } })).toBe('send');
  });

  it('an unknown shortcut is always guarded; a known own-data one is not', () => {
    expect(approvalClass('phone.shortcuts.run', { name: 'Morning' })).toBe('send');
    expect(approvalClass('phone.shortcuts.run', { name: 'Order coffee' })).toBe('purchase');
    expect(approvalClass('phone.shortcuts.run', { name: 'Morning' }, { action: { kind: 'create', guarded: false } })).toBeNull();
  });

  it('Home: locks, garage doors and security scenes need the owner; lights do not', () => {
    expect(approvalClass('phone.home.set', { id: 'lamp', state: { on: true } })).toBeNull();
    expect(approvalClass('phone.home.set', { id: 'front', state: { locked: false } })).toBe('security');
    expect(approvalClass('phone.home.set', { id: 'a1', state: { on: true } }, { homeKind: 'garage' })).toBe('security');
    expect(approvalClass('phone.home.scene', { id: 'Good night' })).toBeNull();
    expect(approvalClass('phone.home.scene', { id: 's1' }, { sceneSecurity: true })).toBe('security');
    expect(approvalClass('phone.home.list', {})).toBeNull();
    expect(approvalClass('phone.actions.list', {})).toBeNull();
  });

  it('learns Home device kinds from phone.home.list for the next phone.home.set', async () => {
    const official = fakeLane('official', { result: { ok: true, detail: { devices: [{ id: 'a1', name: 'Door', room: 'Hall', kind: 'lock', state: {} }], scenes: [] } } });
    const router = new PhoneRouter({ lanes: { official: official.lane } });
    await router.run('phone.home.list', SERIAL, {});
    expect(approvalClass('phone.home.set', { id: 'a1', state: { on: true } }, router.hints(SERIAL, 'phone.home.set', { id: 'a1' }))).toBe('security');
  });
});

// ── The official route (the Allternit iPhone app through allternit-api) ─────

const DESCRIBE = {
  ok: true,
  route: 'official',
  verified: true,
  detail: {
    foreground: true,
    foreground_app: 'Allternit',
    locked: false,
    shortcuts: ['Allternit Actions'],
    home: { available: true },
    broadcast: false,
    catalog: {
      built: '2026-11-16',
      actions_shortcut: { name: 'Allternit Actions', known: true, link: null, actions: ['messages.send', 'notes.create'] },
      apps: [{ id: 'slack', name: 'Slack', category: 'Social', actions: 3, web_actions: 1, shortcut: 'Allternit Slack', shortcut_known: false, shortcut_link: null }],
    },
  },
};

const okEnv = (detail: Record<string, unknown>, verified = true): AppRpcResult => ({ ok: true, result: { ok: true, route: 'official', verified, detail } });

/** The node's /phones/app/:id/rpc, faked at the HTTP edge; the app's state as the node lists it. */
function appNode(opts: { appOpen?: boolean | (() => boolean); screen?: boolean; answer?: (method: string, params: Record<string, unknown>) => AppRpcResult } = {}) {
  const calls: Array<{ method: string; params: Record<string, unknown>; timeoutMs: number }> = [];
  let refreshes = 0;
  const open = () => (typeof opts.appOpen === 'function' ? opts.appOpen() : opts.appOpen ?? true);
  const apps: AppPhoneLane = {
    has: (s) => s === SERIAL,
    serials: () => [SERIAL],
    call: async () => ({ ok: false, error: 'not_supported', message: 'no taps here' }),
    refresh: async () => {
      refreshes += 1;
    },
    devices: () => [{ serial: SERIAL, platform: 'ios', via: 'allternit-app', route: 'iphone-app-store', name: 'iPhone', canTap: false, screen: opts.screen ?? false, note: '' }],
    officialState: () => ({ appOpen: open(), screen: opts.screen ?? false, deviceAccess: { shortcuts: true, home: true } }),
    rpc: async (_serial, method, params, timeoutMs) => {
      calls.push({ method, params, timeoutMs });
      if (opts.answer) return opts.answer(method, params);
      return method === 'device.describe' ? { ok: true, result: DESCRIBE } : okEnv({ output: 'done', returned_via: 'app_intent' });
    },
  };
  return { apps, calls, refreshes: () => refreshes };
}

describe('official route: the phone\'s envelope and refusals', () => {
  it('keeps the phone\'s detail and verified, and passes its needs_you through', () => {
    expect(officialResult('phone.home.set', okEnv({ id: 'lamp', state: { on: true } }))).toEqual({ ok: true, verified: true, detail: { id: 'lamp', state: { on: true } } });
    const missing = officialResult('phone.shortcuts.run', {
      ok: true,
      result: { ok: false, route: 'official', verified: false, detail: { error: 'shortcut_not_installed', message: 'x' }, needs_you: { code: 'SHORTCUT_NOT_INSTALLED', message: 'Add the "Standup" shortcut in Allternit Settings > Device Access > iPhone actions.' } },
    });
    expect(missing).toMatchObject({ ok: false, error: 'needs_you', needs_you: { code: 'SHORTCUT_NOT_INSTALLED', message: expect.stringContaining('"Standup"') }, detail: { error: 'shortcut_not_installed' } });
    // The phone's own timeout (no answer from Shortcuts) comes as SHORTCUT_NOT_INSTALLED with error "timeout".
    const late = officialResult('phone.shortcuts.run', { ok: true, result: { ok: false, verified: false, detail: { error: 'timeout' }, needs_you: { code: 'SHORTCUT_NOT_INSTALLED', message: '' } } });
    expect(late).toMatchObject({ error: 'needs_you', needs_you: { code: 'SHORTCUT_NOT_INSTALLED', message: 'Add the "Allternit Actions" shortcut in Allternit Settings > Device Access > iPhone actions.' }, detail: { error: 'timeout' } });
    expect(officialResult('phone.home.set', { ok: true, result: { ok: false, detail: { error: 'unreachable', message: 'Lamp is not responding.' } } })).toMatchObject({ ok: false, error: 'phone_failed', message: 'Lamp is not responding.', detail: { error: 'unreachable' } });
  });

  it('turns runtime refusals into states the person can act on', () => {
    expect(officialResult('phone.shortcuts.run', { ok: false, status: 409, code: 'app_not_open', message: '' })).toMatchObject({ error: 'needs_you', needs_you: { code: 'APP_NOT_FOREGROUND', message: 'Open the Allternit app on your iPhone.' } });
    expect(officialResult('phone.home.set', { ok: false, status: 200, code: 'device_access_off', message: '' })).toMatchObject({ error: 'device_access_off', message: expect.stringContaining('Turn on Home in the Allternit app') });
    expect(officialResult('phone.home.set', { ok: false, status: 200, code: 'device_access_off', message: '' })).toMatchObject({
      needs_you: { code: 'DEVICE_ACCESS_OFF', message: 'Turn on Home in the Allternit app on your iPhone (Settings > Device Access).' },
    });
    expect(officialResult('phone.actions.run', { ok: false, status: 200, code: 'device_access_off', message: '' })).toMatchObject({ message: expect.stringContaining('Turn on Shortcuts') });
    expect(officialResult('phone.home.list', { ok: false, status: 200, code: 'permission_denied', message: '' })).toMatchObject({ error: 'device_access_off', detail: { error: 'permission_denied' } });
    expect(officialResult('phone.shortcuts.run', { ok: false, status: 504, code: 'app_timeout', message: '' })).toMatchObject({ error: 'timeout', message: expect.stringContaining('may still have done it') });
  });
});

describe('official route lane', () => {
  it('needs the app open for shortcuts and Home, and the broadcast for a screen read', async () => {
    const closed = appNode({ appOpen: false });
    const router = new PhoneRouter({ lanes: { official: officialLane(() => closed.apps) } });
    const res = await router.run('phone.home.list', SERIAL, {});
    expect(res).toMatchObject({ ok: false, route: 'official', needs_you: { code: 'APP_NOT_FOREGROUND' } });
    expect(closed.refreshes()).toBe(1);
    expect(closed.calls).toHaveLength(0);
    expect(await router.run('phone.read_ui', SERIAL, {})).toMatchObject({ ok: false, needs_you: { code: 'BROADCAST_STOPPED' } });

    const sharing = appNode({ appOpen: false, screen: true, answer: () => okEnv({ elements: [{ class: 'Text', label: 'Send', id: '' }], text: 'Send', width: 1170, height: 2532 }) });
    const read = await new PhoneRouter({ lanes: { official: officialLane(() => sharing.apps) } }).run('phone.read_ui', SERIAL, { filter: { label: 'Send' } });
    expect(read).toMatchObject({ ok: true, route: 'official', verified: true, text: 'Send', detail: { width: 1170 } });
    expect(sharing.calls).toEqual([{ method: 'device.screen.read', params: { filter: { label: 'Send' } }, timeoutMs: 40_000 }]);
  });

  it('reads the broadcast frame on the computer when the extension has no memory to read it', async () => {
    const outOfMemory = (fallback: string): AppRpcResult => ({
      ok: true,
      result: { ok: false, route: 'official', verified: false, detail: { error: 'unavailable', message: 'Not enough memory.', fallback, available_mb: 9 } },
    });
    // A clock the waits move: the request goes out at t = 10 000.
    let t = 10_000;
    const waits: number[] = [];
    const read: string[] = [];
    const run = async (fallback: string, ages: number[]) => {
      const node = appNode({ appOpen: false, screen: true, answer: () => outOfMemory(fallback) });
      const shots: number[] = [];
      node.apps.call = async () => {
        shots.push(t);
        const ageMs = ages[Math.min(shots.length - 1, ages.length - 1)];
        return { ok: true, dataBase64: 'anBlZw==', mimeType: 'image/jpeg', width: 1170, height: 2532, ageMs } as ToolResult;
      };
      const lane = officialLane(() => node.apps, () => t, {
        sleep: async (ms) => {
          waits.push(ms);
          t += ms;
        },
        readImage: () => async (b64) => {
          read.push(b64);
          return {
            elements: [{ class: 'Text', label: 'Messages', id: '', bounds: [10, 20, 200, 60] }, { class: 'Text', label: 'Send', id: '', bounds: [900, 2400, 1100, 2460] }],
            text: 'Messages\nSend',
            width: 1170,
            height: 2532,
          };
        },
      });
      const res = await new PhoneRouter({ lanes: { official: lane } }).run('phone.read_ui', SERIAL, { filter: { label: 'send' } });
      return { res, shots };
    };

    // A fresh frame was sent: the still from before the request is passed over until the new one arrives.
    const sent = await run('frame_sent', [500, 100]);
    expect(sent.res).toMatchObject({ ok: true, route: 'official', text: 'Send', elements: [{ label: 'Send', bounds: [900, 2400, 1100, 2460] }], detail: { read_by: 'desktop', width: 1170 } });
    expect(sent.shots).toEqual([10_000, 10_150]);
    expect(waits).toEqual([150]);
    expect(read).toEqual(['anBlZw==']);

    // The new frame never comes: refuse the old still, after a few seconds at most.
    t = 20_000;
    waits.length = 0;
    read.length = 0;
    const stale = await run('frame_sent', [60_000]);
    expect(stale.res).toMatchObject({ ok: false, error: 'no_screen', message: FALLBACK_FRAME_STALE, detail: { error: 'stale_frame' } });
    expect(t - 20_000).toBeGreaterThanOrEqual(FALLBACK_FRAME_WAIT_MS);
    expect(t - 20_000).toBeLessThan(FALLBACK_FRAME_WAIT_MS + 500);
    expect(read).toEqual([]);

    // No fresh frame promised: the newest still is the screen now, read at once.
    t = 30_000;
    waits.length = 0;
    const latest = await run('runtime_reads_frames', [60_000]);
    expect(latest.res).toMatchObject({ ok: true, text: 'Send', detail: { read_by: 'desktop' } });
    expect(latest.shots).toEqual([30_000]);
    expect(waits).toEqual([]);

    // No OCR in this build (Windows, Linux): the phone's own answer stands.
    const node = appNode({ appOpen: false, screen: true, answer: () => outOfMemory('frame_sent') });
    const plain = await new PhoneRouter({ lanes: { official: officialLane(() => node.apps) } }).run('phone.read_ui', SERIAL, {});
    expect(plain).toMatchObject({ ok: false, detail: { error: 'unavailable' } });
  });

  it('sends Home ids as device_id / scene_id and keeps the phone\'s read-back as verified', async () => {
    const node = appNode({ answer: (method, params) => okEnv(method === 'device.home.set' ? { id: params.device_id, state: { on: true } } : { id: params.scene_id, ran: true }) });
    const router = new PhoneRouter({ lanes: { official: officialLane(() => node.apps) } });
    expect(await router.run('phone.home.set', SERIAL, { id: 'lamp', state: { on: true } })).toMatchObject({ ok: true, verified: true, detail: { id: 'lamp' } });
    await router.run('phone.home.scene', SERIAL, { id: 'evening' });
    await router.run('phone.home.list', SERIAL, { kind: 'lock' });
    expect(node.calls.map((c) => [c.method, c.params])).toEqual([
      ['device.home.set', { device_id: 'lamp', state: { on: true } }],
      ['device.home.scene', { scene_id: 'evening' }],
      ['device.home.list', { kind: 'lock' }],
    ]);
  });

  it('waits the shortcut\'s timeout_s plus a margin for the runtime', async () => {
    const node = appNode();
    const router = new PhoneRouter({ lanes: { official: officialLane(() => node.apps) } });
    await router.run('phone.shortcuts.run', SERIAL, { name: 'Note', input: 'hi', timeout_s: 90 });
    expect(node.calls).toEqual([{ method: 'device.shortcuts.run', params: { name: 'Note', input: 'hi', timeout_s: 90 }, timeoutMs: 100_000 }]);
  });

  it('lists actions from device.describe and runs one through its shortcut', async () => {
    const node = appNode();
    const router = new PhoneRouter({ lanes: { official: officialLane(() => node.apps) } });
    const list = await router.run('phone.actions.list', SERIAL, {});
    expect(list).toMatchObject({ ok: true, detail: { apps: [{ id: 'allternit', installed: true, actions: [{ id: 'messages.send', kind: 'send', guarded: true }, { id: 'notes.create', kind: 'create', guarded: false }] }, { id: 'slack', shortcut: 'Allternit Slack', installed: false, action_count: 4 }] } });
    expect((await router.run('phone.actions.list', SERIAL, { installed_only: true }))?.detail.apps).toHaveLength(1);

    const run = await router.run('phone.actions.run', SERIAL, { app: 'allternit', action: 'notes.create', params: { title: 'Milk' } });
    expect(run).toMatchObject({ ok: true, route: 'official', detail: { output: 'done', dispatched_as: 'actions_shortcut' } });
    expect(node.calls.filter((c) => c.method === 'device.describe')).toHaveLength(1);
    expect(node.calls.at(-1)).toMatchObject({ method: 'device.shortcuts.run', params: { name: 'Allternit Actions', input: { action: 'notes.create', params: { title: 'Milk' } } } });
    await router.run('phone.actions.run', SERIAL, { app: 'slack', action: 'SendMessageIntent', params: { text: 'hi' } });
    expect(node.calls.at(-1)?.params).toMatchObject({ name: 'Allternit Slack', input: { action: 'SendMessageIntent' } });
    expect(await router.run('phone.actions.run', SERIAL, { app: 'nope', action: 'x' })).toMatchObject({ ok: false, error: 'unknown_action' });
    expect(await router.run('phone.actions.run', SERIAL, { app: 'allternit', action: 'wallet.pay' })).toMatchObject({ ok: false, error: 'unknown_action' });
  });

  it('lists every third-party action from device.describe\'s action_list and refuses an unknown id', async () => {
    const detail = {
      ...DESCRIBE.detail,
      catalog: {
        ...DESCRIBE.detail.catalog,
        actions_shortcut: { ...DESCRIBE.detail.catalog.actions_shortcut, action_list: [{ id: 'notes.create', title: 'Create a note', kind: 'create', params: ['title'] }] },
        apps: [{
          ...DESCRIBE.detail.catalog.apps[0],
          shortcut_known: true,
          action_list: [
            { id: 'SendMessageIntent', title: 'Send Message', kind: 'send', params: ['channel', 'text'], opens_app: false, source: 'app_intent' },
            { id: 'SearchMessages', title: 'Search Messages', kind: 'search', params: ['query'], source: 'app_intent' },
            { id: 'set status', title: 'set status', kind: 'control', params: ['text'], confidence: 'medium', source: 'web_research' },
          ],
        }],
      },
    };
    const node = appNode({ answer: (method) => (method === 'device.describe' ? okEnv(detail) : okEnv({ output: 'done', returned_via: 'x_callback' })) });
    const router = new PhoneRouter({ lanes: { official: officialLane(() => node.apps) } });
    const list = await router.run('phone.actions.list', SERIAL, { app: 'slack' });
    expect(list?.detail.apps).toEqual([{
      id: 'slack', name: 'Slack', category: 'Social', shortcut: 'Allternit Slack', installed: true, action_count: 3,
      actions: [
        { id: 'SendMessageIntent', title: 'Send Message', kind: 'send', params: ['channel', 'text'], opens_app: false, dispatch: 'app_shortcut', installed: true, guarded: true },
        { id: 'SearchMessages', title: 'Search Messages', kind: 'search', params: ['query'], dispatch: 'app_shortcut', installed: true, guarded: false },
        { id: 'set status', title: 'set status', kind: 'control', params: ['text'], confidence: 'medium', dispatch: 'app_shortcut', installed: true, guarded: true },
      ],
    }]);
    // A kind filter now reaches third-party apps too.
    const sends = await router.run('phone.actions.list', SERIAL, { kind: 'send' });
    expect((sends?.detail.apps as Array<{ id: string; actions: Array<{ id: string }> }>).map((a) => [a.id, a.actions.map((x) => x.id)])).toEqual([['slack', ['SendMessageIntent']]]);

    expect(await router.run('phone.actions.run', SERIAL, { app: 'slack', action: 'DeleteWorkspace' })).toMatchObject({ ok: false, error: 'unknown_action' });
    expect(node.calls.some((c) => c.method === 'device.shortcuts.run')).toBe(false);
    expect(await router.run('phone.actions.run', SERIAL, { app: 'slack', action: 'SearchMessages', params: { query: 'lunch' } })).toMatchObject({ ok: true, detail: { dispatched_as: 'app_shortcut' } });
  });

  it('phone.describe reports the app\'s facts when the app is open', async () => {
    const node = appNode();
    const res = await new PhoneRouter({ lanes: { official: officialLane(() => node.apps) } }).describe(SERIAL);
    expect(res).toMatchObject({ ok: true, route: 'official', detail: { locked: false, foreground_app: 'Allternit', shortcuts: ['Allternit Actions'], home: { available: true } } });
    expect((res.detail.routes as Array<Record<string, unknown>>)[0]).toMatchObject({ route: 'official', healthy: true, covers: ['shortcuts', 'actions', 'home', 'read_ui'] });
  });

  it('find_and_tap never lands on the official route (it reads but cannot tap)', async () => {
    const node = appNode({ screen: true });
    const mirror = fakeLane('mirror');
    const router = new PhoneRouter({ lanes: { official: officialLane(() => node.apps), mirror: mirror.lane } });
    expect(await router.run('phone.find_and_tap', SERIAL, { element_id: 'send' })).toMatchObject({ ok: true, route: 'mirror' });
    expect(node.calls).toHaveLength(0);
  });
});
