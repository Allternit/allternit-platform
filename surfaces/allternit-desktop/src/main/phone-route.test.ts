import { describe, expect, it } from 'vitest';
import { needsYou, NEEDS_YOU_CODES, NEEDS_YOU_SENTENCES, type RouteHealth } from './phone-link-health.js';
import { IOS_HELPER_RETIRED_INSTRUCTION, PhoneLinkManager } from './phone-link.js';
import {
  approvalClass,
  deriveActionKind,
  legacyHelperLane,
  PhoneRouter,
  ROUTE_ORDER,
  usbAgentLane,
  type Capability,
  type LaneTool,
  type PhoneLane,
  type RouteId,
} from './phone-route.js';
import type { PhoneLinkBackend, ToolResult } from './phone-tools.js';

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
    expect(needsYou('SHORTCUT_NOT_INSTALLED', { name: 'Allternit Actions' }).message).toBe('Add the "Allternit Actions" shortcut in Allternit Settings > Shortcuts.');
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
    expect(routes[1]).toEqual({ route: 'mirror', healthy: false, available: true, needs_you: 'MAC_LOCKED', covers: TAP });
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
