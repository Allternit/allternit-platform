import { describe, expect, it } from 'vitest';
import { IctrlError } from './ictrl-client.js';
import {
  mergeParsed,
  parseAccountScreen,
  parsePlan,
  parseResetTime,
  parseUsageWindows,
  pickTap,
  onHomeScreen,
  pickTextTap,
  PhoneAccountScheduler,
  readLinkedAccount,
  remainingPct,
  screenElements,
} from './phone-account-reader.js';
import { ACCOUNT_SPECS, deniedElement, type AccountTapStep } from './phone-account-specs.js';
import { PhoneLinkBackend } from './phone-link-backend.js';
import type { LinkedPhone } from './phone-link-health.js';
import type { IctrlLike } from './phone-link-ios.js';
import type { PhoneLinkAccountResult, PhoneLinkApp, PhoneLinkHealth } from './phone-link-types.js';

// Sunday 2026-10-18 14:00 local time.
const NOW = new Date(2026, 9, 18, 14, 0, 0);
const local = (y: number, m: number, d: number, h = 0, min = 0) => new Date(y, m, d, h, min).toISOString();
const lines = (...labels: string[]) => labels.map((l, i) => `${l} @ (10, ${10 + i * 40})`).join('\n');

// All screen texts below are ILLUSTRATIVE: written from what these apps
// show on the web, not captured from a phone yet (specs are verified: false).

describe('parseAccountScreen (illustrative screens)', () => {
  it('illustrative: Claude settings › usage with a session and weekly window', () => {
    const text = lines(
      'Settings',
      'eoj@example.com',
      'Max plan',
      'Plan usage limits',
      'Current session',
      'Resets in 2 hr 10 min',
      '34% used',
      'Weekly limits',
      'All models',
      'Resets Thu 9:00 AM',
      '12% used',
      'Weekly · Opus',
      'Resets Thu 9:00 AM',
      '40% used',
    );
    const parsed = parseAccountScreen(text, ACCOUNT_SPECS.claude, NOW);
    expect(parsed.account).toBe('eoj@example.com');
    expect(parsed.plan).toBe('Max');
    expect(parsed.windows).toEqual([
      { id: 'five_hour', label: '5-hour', remaining_pct: 66, resets_at: new Date(NOW.getTime() + (2 * 60 + 10) * 60_000).toISOString() },
      { id: 'weekly', label: 'Weekly', remaining_pct: 88, resets_at: local(2026, 9, 22, 9, 0) },
      { id: 'weekly_opus', label: 'Weekly (Opus)', remaining_pct: 60, resets_at: local(2026, 9, 22, 9, 0) },
    ]);
  });

  it('illustrative: ChatGPT settings with the plan under "Subscription"', () => {
    const text = lines('Settings', 'Account', 'Email', 'eoj@example.com', 'Subscription', 'ChatGPT Plus', 'Upgrade to Pro', 'Personalization');
    expect(parseAccountScreen(text, ACCOUNT_SPECS.chatgpt, NOW)).toEqual({ account: 'eoj@example.com', plan: 'Plus', windows: [] });
  });

  it('illustrative: bare plan name only counts next to a plan label', () => {
    expect(parsePlan(['Subscription', 'Pro'], ACCOUNT_SPECS.perplexity)).toBe('Pro');
    expect(parsePlan(['Pro', 'Ask anything'], ACCOUNT_SPECS.perplexity)).toBeNull();
  });

  it('illustrative: a plan comparison page gives no plan (never guesses)', () => {
    expect(parsePlan(['Plan', 'Free', 'Plus', 'Pro', 'Get Plus'], ACCOUNT_SPECS.chatgpt)).toBeNull();
    expect(parsePlan(['Try SuperGrok', 'Upgrade to SuperGrok'], ACCOUNT_SPECS.grok)).toBeNull();
  });

  it('illustrative: Grok, Gemini and Kimi plan labels', () => {
    expect(parsePlan(['@eoj', 'SuperGrok'], ACCOUNT_SPECS.grok)).toBeNull();
    expect(parsePlan(['Your plan', 'SuperGrok'], ACCOUNT_SPECS.grok)).toBe('SuperGrok');
    expect(parsePlan(['Google AI Pro'], ACCOUNT_SPECS.gemini)).toBe('Google AI Pro');
    expect(parsePlan(['Membership', 'Moderato'], ACCOUNT_SPECS.kimi)).toBe('Moderato');
    expect(parsePlan(['Plan: Allegretto'], ACCOUNT_SPECS.kimi)).toBe('Allegretto');
  });

  it('illustrative: one-line window and a header-less message count', () => {
    expect(parseUsageWindows(['Current session 34% used Resets in 45 min'], NOW)).toEqual([
      { id: 'five_hour', label: '5-hour', remaining_pct: 66, resets_at: new Date(NOW.getTime() + 45 * 60_000).toISOString() },
    ]);
    expect(parseUsageWindows(['12 of 40 messages left', 'Resets at 6:00 PM'], NOW)).toEqual([
      { id: 'messages', label: 'Messages', remaining_pct: 30, resets_at: local(2026, 9, 18, 18, 0) },
    ]);
  });

  it('illustrative: unparseable usage stays null; a bare percentage is not a window', () => {
    expect(parseUsageWindows(['Weekly limits', '34%'], NOW)).toEqual([]);
    expect(parseUsageWindows(['Daily limit', '20% left', 'Resets soon'], NOW)).toEqual([
      { id: 'daily', label: 'Daily', remaining_pct: 20, resets_at: null },
    ]);
    expect(parseAccountScreen(lines('New chat', 'Ask anything'), ACCOUNT_SPECS.chatgpt, NOW)).toEqual({ account: null, plan: null, windows: [] });
  });
});

describe('remainingPct / parseResetTime', () => {
  it('reads used, left and X of Y forms', () => {
    expect(remainingPct('34% used')).toBe(66);
    expect(remainingPct('66% left')).toBe(66);
    expect(remainingPct('28/40 used')).toBe(30);
    expect(remainingPct('used 10 of 50')).toBe(80);
    expect(remainingPct('Resets Thu 9:00 AM')).toBeNull();
  });

  it('turns relative and absolute text into ISO in local time', () => {
    expect(parseResetTime('in 1 day 3 hours', NOW)).toBe(new Date(NOW.getTime() + 27 * 3600_000).toISOString());
    expect(parseResetTime('Sun 9:00 AM', NOW)).toBe(local(2026, 9, 25, 9, 0)); // today's 9 AM already passed
    expect(parseResetTime('Oct 21, 9:00 AM', NOW)).toBe(local(2026, 9, 21, 9, 0));
    expect(parseResetTime('Jan 2', NOW)).toBe(local(2027, 0, 2));
    expect(parseResetTime('tomorrow at 8 pm', NOW)).toBe(local(2026, 9, 19, 20, 0));
    expect(parseResetTime('at 1:30 PM', NOW)).toBe(local(2026, 9, 19, 13, 30));
    expect(parseResetTime('soon', NOW)).toBeNull();
    expect(parseResetTime('Thursday', NOW)).toBeNull();
    expect(parseResetTime('tomorrow', NOW)).toBeNull();
  });
});

describe('tap safety', () => {
  it('never picks a denied label or a toggle, even when a spec candidate matches', () => {
    expect(deniedElement('Sign out')).toBe(true);
    expect(deniedElement('Log out')).toBe(true);
    expect(deniedElement('Upgrade plan')).toBe(true);
    expect(deniedElement('Delete account')).toBe(true);
    expect(deniedElement('Manage subscription')).toBe(true);
    expect(deniedElement('Cancel subscription')).toBe(true);
    expect(deniedElement('Settings')).toBe(false);
    expect(deniedElement('Subscription')).toBe(false);
    expect(deniedElement('Usage', 'android.widget.Switch')).toBe(true);
    const els = screenElements(['android.widget.Switch "Usage" [com.x:id/usage]', 'android.widget.TextView "Sign out" [com.x:id/signout]'].join('\n'));
    expect(pickTap(els, { tap: ['Usage', 'Sign out', 'id:signout'] })).toBeNull();
  });

  it('picks the first candidate on screen, by label or resource id', () => {
    const els = screenElements(['android.widget.Button "Open sidebar" [com.openai:id/menu]', 'android.view.View [com.openai:id/settings] {{0, 0}, {10, 10}}'].join('\n'));
    expect(pickTap(els, { tap: ['Menu', 'open sidebar'] })).toEqual({ element_id: 'Open sidebar' });
    expect(pickTap(els, { tap: ['Settings', 'id:settings'] })).toEqual({ element_id: 'com.openai:id/settings' });
    expect(pickTap(screenElements(lines('Settings')), { tap: ['Settings'] })).toEqual({ element_id: 'Settings' });
  });
});

// ── Reader against a fake phone ─────────────────────────────────────────────

type Handler = (params: Record<string, unknown>) => unknown;

function fakeClient(handlers: Record<string, Handler>) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const client: IctrlLike = {
    host: '192.168.1.30:8200',
    hasKey: () => true,
    forget: () => {},
    whoami: async () => ({ device_id: 'and-1' }),
    pair: async () => ({}),
    withHost: () => client,
    call: async <T = Record<string, unknown>>(method: string, params: Record<string, unknown> = {}): Promise<T> => {
      calls.push({ method, params });
      const handler = handlers[method];
      if (!handler) throw new IctrlError('rpc_error', `Method not found: ${method}`);
      return handler(params) as T;
    },
  };
  return { client, calls };
}

const healthy: PhoneLinkHealth = { state: 'HEALTHY', checkedAt: '2026-10-18T00:00:00.000Z', recovery: 'automatic' };

function setup(handlers: Record<string, Handler>, platform: 'ios' | 'android' = 'android') {
  const { client, calls } = fakeClient(handlers);
  const phone: LinkedPhone = {
    deviceId: 'and-1',
    platform,
    name: 'Pixel 9',
    lanHost: '192.168.1.30:8200',
    storeKey: 'k',
    apps: [{ vendor: 'claude', signedIn: true, name: 'Claude', category: 'AI chat' }],
  };
  const saved: PhoneLinkApp[][] = [];
  const lane = new PhoneLinkBackend({
    phones: () => [phone],
    client: () => client,
    health: { health: () => healthy } as never,
    saveApps: (_p, apps) => {
      saved.push(apps);
      phone.apps = apps;
    },
  });
  return { lane, calls, saved, phone };
}

const items = (...labels: string[]) => ({ items: labels.map((label, i) => ({ label, center_x: 10, center_y: 10 + i })) });
const source = (...rows: string[]) => ({ source: rows.join('\n') });

describe('readLinkedAccount', () => {
  it('illustrative: walks menu → settings → usage, taps only spec labels, saves the result and goes Home', async () => {
    let screen = 'home';
    const { lane, calls, saved } = setup({
      launch_app: () => ({ message: 'Launched' }),
      page_source: () =>
        screen === 'home'
          ? source('android.widget.ImageButton "Open menu"', 'android.widget.TextView "Sign out"')
          : screen === 'menu'
            ? source('android.widget.TextView "Settings"', 'android.widget.TextView "Log out"')
            : source('android.widget.TextView "Usage"', 'android.widget.TextView "Delete account"'),
      screen_text: () =>
        screen === 'usage'
          ? items('Usage', 'Current session', 'Resets in 1 hr', '50% used', 'Weekly limits', 'Resets Thu 9:00 AM', '10% used')
          : screen === 'settings'
            ? items('Settings', 'eoj@example.com', 'Claude Max', 'Usage')
            : items('New chat'),
      find_and_tap: (p) => {
        screen = p.element_id === 'Open menu' ? 'menu' : p.element_id === 'Settings' ? 'settings' : p.element_id === 'Usage' ? 'usage' : screen;
        return { message: 'tapped' };
      },
      swipe: () => ({}),
      back: () => ({}),
      home: () => ({}),
    });
    const res = await readLinkedAccount({ lane, sleep: async () => {}, now: () => NOW }, { deviceId: 'and-1', app: 'claude' });
    expect(res.ok).toBe(true);
    expect(res.account).toBe('eoj@example.com');
    expect(res.plan).toBe('Max');
    expect(res.usage?.windows.map((w) => [w.id, w.remaining_pct])).toEqual([
      ['five_hour', 50],
      ['weekly', 90],
    ]);
    const taps = calls.filter((c) => c.method === 'find_and_tap').map((c) => c.params.element_id);
    expect(taps).toEqual(['Open menu', 'Settings', 'Usage']);
    expect(calls.some((c) => c.method === 'type')).toBe(false);
    expect(calls.at(-1)?.method).toBe('home');
    expect(saved.at(-1)?.[0]).toMatchObject({ vendor: 'claude', account: 'eoj@example.com', plan: 'Max', accountCheckedAt: NOW.toISOString() });
  });

  it('stops on accessibility off and still goes Home', async () => {
    const off = () => {
      throw new IctrlError('rpc_error', "Phone Link's accessibility service is not running. Re-run setup from Allternit Desktop.");
    };
    const { lane, calls } = setup({ launch_app: off, home: () => ({}) });
    const res = await readLinkedAccount({ lane, sleep: async () => {} }, { deviceId: 'and-1', app: 'claude' });
    expect(res).toMatchObject({ ok: false, account: null, plan: null, usage: null, error: { code: 'accessibility_off' } });
    expect(calls.at(-1)?.method).toBe('home');
  });

  it('waits while a bot is using the phone', async () => {
    const { lane, calls } = setup({});
    const res = await readLinkedAccount({ lane, busy: () => true }, { deviceId: 'and-1', app: 'claude' });
    expect(res.error?.code).toBe('phone_busy');
    expect(calls).toEqual([]);
  });

  it('checks an app without a spec for sign-in only: a sign-in screen is saved as signed out', async () => {
    const { lane, calls, saved } = setup({
      launch_app: () => ({ message: 'Launched' }),
      page_source: () => source('android.widget.Button "Sign in"', 'android.widget.Button "Create account"'),
      screen_text: () => items('Get paid anywhere', 'Create account', 'Sign in'),
      home: () => ({}),
    });
    const res = await readLinkedAccount({ lane, sleep: async () => {}, now: () => NOW }, { deviceId: 'and-1', app: 'quickbooks' });
    expect(res).toMatchObject({ ok: true, signedIn: false, account: null, plan: null, usage: null });
    expect(calls.some((c) => c.method === 'find_and_tap' || c.method === 'tap')).toBe(false);
    expect(saved.at(-1)?.find((a) => a.vendor === 'quickbooks')).toMatchObject({ signedIn: false, account: null, accountCheckedAt: NOW.toISOString() });
    expect(calls.at(-1)?.method).toBe('home');
  });

  it('a signed-out app with a spec stops at its sign-in screen and forgets the old account', async () => {
    const { lane, calls, saved, phone } = setup({
      launch_app: () => ({ message: 'Launched' }),
      page_source: () => source('android.widget.TextView "Session expired. Please log in again."', 'android.widget.Button "Log in"'),
      screen_text: () => items('Session expired. Please log in again.', 'Log in'),
      home: () => ({}),
    });
    phone.apps = [{ vendor: 'claude', signedIn: true, name: 'Claude', category: 'AI chat', account: 'old@example.com', plan: 'Max', accountCheckedAt: '2026-10-17T00:00:00.000Z' }];
    const res = await readLinkedAccount({ lane, sleep: async () => {}, now: () => NOW }, { deviceId: 'and-1', app: 'claude' });
    expect(res).toMatchObject({ ok: true, signedIn: false, account: null });
    expect(calls.filter((c) => c.method === 'find_and_tap')).toEqual([]);
    expect(saved.at(-1)?.[0]).toMatchObject({ signedIn: false, account: null, plan: null });
  });

  it('a read that sees nothing keeps the last account', async () => {
    const { lane, saved, phone } = setup({
      launch_app: () => ({ message: 'Launched' }),
      page_source: () => source('android.widget.TextView "New chat"'),
      screen_text: () => items('New chat'),
      find_and_tap: () => ({ message: 'tapped' }),
      swipe: () => ({}),
      back: () => ({}),
      home: () => ({}),
    });
    phone.apps = [{ vendor: 'claude', signedIn: true, name: 'Claude', category: 'AI chat', account: 'eoj@example.com', plan: 'Max', accountCheckedAt: '2026-10-17T00:00:00.000Z' }];
    const res = await readLinkedAccount({ lane, sleep: async () => {}, now: () => NOW }, { deviceId: 'and-1', app: 'claude' });
    expect(res).toMatchObject({ ok: true, signedIn: true, account: null });
    expect(saved.at(-1)?.[0]).toMatchObject({ signedIn: true, account: 'eoj@example.com', plan: 'Max' });
  });

  it('swipes a Settings list to find a row further down, and taps visible text that has no element', async () => {
    let screen = 'home';
    let swipes = 0;
    const { lane, calls } = setup(
      {
        launch_app: () => ({ message: 'Launched' }),
        // The iPhone runner: the element tree comes back as XCUI text.
        page_source: () => ({
          source:
            screen === 'settings' && swipes > 0
              ? "Button, label: 'Usage and limits', {{16, 504}, {361, 51}}"
              : screen === 'settings'
                ? "Button, label: 'Subscription, Plus', {{16, 348}, {361, 52}}"
                : "TextView, label: 'New Message', {{16, 400}, {361, 40}}",
        }),
        screen_text: () => (screen === 'home' ? items('Ask ChatGPT', 'Settings') : items('Settings')),
        tap: () => {
          screen = 'settings';
          return {};
        },
        find_and_tap: (p) => {
          if (p.element_id === 'Usage and limits') screen = 'usage';
          return { message: 'tapped' };
        },
        swipe: () => {
          swipes++;
          return {};
        },
        home: () => ({}),
      },
      'ios',
    );
    const spec = { ...ACCOUNT_SPECS.chatgpt, paths: [{ name: 't', verifiedOn: 'ios' as const, steps: [{ tap: ['Settings'] }, { tap: ['Usage and limits'], scroll: 2 }] }] };
    const prior = ACCOUNT_SPECS.chatgpt;
    (ACCOUNT_SPECS as Record<string, typeof prior>).chatgpt = spec;
    try {
      lane.linkedPhones()[0].apps = [{ vendor: 'chatgpt', signedIn: true, name: 'ChatGPT', category: 'AI chat' }];
      await readLinkedAccount({ lane, sleep: async () => {}, now: () => NOW }, { deviceId: 'and-1', app: 'chatgpt' });
    } finally {
      (ACCOUNT_SPECS as Record<string, typeof prior>).chatgpt = prior;
    }
    expect(calls.find((c) => c.method === 'tap')?.params).toMatchObject({ x: 10 });
    expect(swipes).toBe(1);
    expect(calls.filter((c) => c.method === 'find_and_tap').map((c) => c.params.element_id)).toEqual(['Usage and limits']);
  });
});

describe('PhoneAccountScheduler', () => {
  const T0 = Date.parse('2026-10-18T12:00:00Z');
  const base = (over: Partial<PhoneLinkApp> = {}): LinkedPhone => ({
    deviceId: 'and-1',
    platform: 'android',
    storeKey: 'k',
    apps: [
      { vendor: 'claude', signedIn: true, category: 'AI chat', accountCheckedAt: '2026-10-18T11:00:00Z', ...over },
      { vendor: 'slack', signedIn: true, category: 'Communication', accountCheckedAt: '2026-10-17T11:00:00Z' },
      { vendor: 'chatgpt', signedIn: false, category: 'AI chat', accountCheckedAt: '2026-10-17T11:00:00Z' },
      { vendor: 'notion', signedIn: null, category: 'Productivity' },
    ],
  });
  const ok = { ok: true, account: null, plan: null, usage: null } as PhoneLinkAccountResult;

  it('checks a new app once, re-reads only signed-in AI chat apps every 6 h, never other apps on a timer', async () => {
    let t = T0;
    const read: string[] = [];
    const s = new PhoneAccountScheduler({ phones: () => [base()], health: () => healthy, busy: () => false, read: async (o) => (read.push(o.app), ok), now: () => t });
    expect((await s.tick())?.app).toBe('notion'); // never checked: once
    expect(await s.tick()).toBeNull();
    t += 5 * 3600_000; // claude last read 11:00 → due at 17:00
    expect((await s.tick())?.app).toBe('claude');
    t += 3 * 24 * 3600_000;
    expect((await s.tick())?.app).toBe('claude'); // slack and the signed-out chatgpt: never on a timer
    expect(await s.tick()).toBeNull();
    expect(read).toEqual(['notion', 'claude', 'claude']);
  });

  it('waits until the phone has sat on its Home screen a few minutes; a Check in the wizard reads at once', async () => {
    let t = T0 + 6 * 3600_000;
    let home = false;
    const read: string[] = [];
    const s = new PhoneAccountScheduler({
      phones: () => [base({ accountCheckedAt: '2026-10-18T11:00:00Z' })],
      health: () => healthy,
      busy: () => false,
      read: async (o) => (read.push(o.app), ok),
      now: () => t,
      atHome: async () => home,
      idleMs: 4 * 60_000,
    });
    expect(await s.tick()).toBeNull(); // in an app: wait
    home = true;
    expect(await s.tick()).toBeNull(); // on Home: the clock starts
    t += 5 * 60_000;
    expect((await s.tick())?.app).toBe('claude'); // usage app, 6 h since its last read
    s.kick('and-1', 'slack');
    home = false;
    expect((await s.tick())?.app).toBe('slack'); // just signed in through the wizard: read now
    expect(await s.tick()).toBeNull(); // notion still waits for an idle phone
    expect(read).toEqual(['claude', 'slack']);
  });

  it('skips locked, unhealthy and busy phones', async () => {
    const mk = (health: PhoneLinkHealth['state'], busy: boolean, phone: LinkedPhone = base()) =>
      new PhoneAccountScheduler({ phones: () => [phone], health: () => ({ ...healthy, state: health }), busy: () => busy, read: async () => ok, now: () => T0 });
    expect(mk('DEVICE_LOCKED', false).due()).toBeNull();
    expect(mk('ACCESSIBILITY_OFF', false).due()).toBeNull();
    expect(mk('HEALTHY', true).due()).toBeNull();
    expect(mk('HEALTHY', false).due()).toEqual({ deviceId: 'and-1', app: 'notion', forced: false });
    // A renewal warning while the signature is still valid: the phone works.
    expect(mk('SIGNATURE_RENEWAL_REQUIRED', false, { ...base(), expiresAt: '2026-10-19T08:00:00Z' }).due()).toEqual({ deviceId: 'and-1', app: 'notion', forced: false });
    expect(mk('SIGNATURE_RENEWAL_REQUIRED', false, { ...base(), expiresAt: '2026-10-18T08:00:00Z' }).due()).toBeNull();
  });

  it('knows a Home screen from an app on both platforms', () => {
    expect(onHomeScreen("Window, {{0, 0}, {393, 852}}\n  Other, {{0, 0}, {393, 852}}\n    Icon, label: 'Weather', {{20, 80}, {64, 64}}", 'ios')).toBe(true);
    expect(onHomeScreen("Application, label: 'ChatGPT', {{0, 0}, {393, 852}}\n  Window, {{0, 0}, {393, 852}}", 'ios')).toBe(false);
    expect(onHomeScreen("Window(application), {{0, 0}, {720, 1600}}, package: 'com.motorola.launcher3'", 'android')).toBe(true);
    expect(onHomeScreen("Window(application), {{0, 0}, {720, 1600}}, package: 'com.discord'", 'android')).toBe(false);
  });
});

/** A path's tap step by its first candidate (paths start with optional back-out taps). */
const stepOf = (app: string, first: string, path = 0) =>
  (ACCOUNT_SPECS[app].paths[path].steps as AccountTapStep[]).find((st) => 'tap' in st && st.tap[0] === first && !st.optional) ??
  (ACCOUNT_SPECS[app].paths[path].steps as AccountTapStep[]).find((st) => 'tap' in st && st.tap[0] === first)!;

// Lines copied from the real Claude iOS app (iPhone, 2026-10-19), email replaced.
describe('Claude on iPhone (real screens)', () => {
  const sidebar = [
    "Button, label: 'Open sidebar', id: 'drawer-toggle', {{20, 63}, {36, 36}}",
    "Button, label: 'joe, Settings', id: 'sidebar-account-button', {{30, 778}, {44, 44}}",
  ].join('\n');
  const settings = [
    "NavigationBar, id: 'Settings', {{0, 75}, {393, 54}}",
    "Other, label: 'Personal, someone​@​example​.com', value: 'Max', {{32, 160}, {329, 44}}",
    "Button, label: 'Add account', {{32, 234}, {329, 21}}",
    "Button, label: 'Billing, Max plan', {{32, 392}, {329, 21}}",
    "Button, label: 'Usage', {{32, 443}, {329, 21}}",
  ].join('\n');
  const usage = [
    "StaticText, label: 'Current session, 0% used, Starts when a message is sent', {{32, 164}, {329, 61}}",
    "StaticText, label: 'Current session', {{32, 164}, {127, 21}}",
    "StaticText, label: 'Weekly limits', {{32, 270}, {120, 21}}",
    "StaticText, label: 'All models, 60% used, Resets Thu 8:59 AM', {{32, 318}, {329, 61}}",
    "StaticText, label: 'All models', {{32, 318}, {83, 21}}",
    "StaticText, label: 'Fable only, 0% used, Resets Thu 9:00 AM', {{32, 418}, {329, 61}}",
    "Button, label: 'Buy usage credits', {{32, 780}, {200, 21}}",
  ].join('\n');
  const now = new Date(2026, 9, 19, 9, 0, 0);

  it('taps the sidebar, the account button and Usage by their real ids and labels', () => {
    expect(pickTap(screenElements(sidebar), stepOf('claude', 'id:drawer-toggle'))).toEqual({ element_id: 'drawer-toggle' });
    expect(pickTap(screenElements(sidebar), stepOf('claude', 'id:sidebar-account-button'))).toEqual({ element_id: 'sidebar-account-button' });
    expect(pickTap(screenElements(settings), stepOf('claude', 'Usage'))).toEqual({ element_id: 'Usage' });
    // The back-out taps a cut-short read relies on find nothing on the sidebar.
    expect(pickTap(screenElements(sidebar), stepOf('claude', 'Close'))).toBeNull();
    expect(ACCOUNT_SPECS.claude.verified).toBe(true);
  });

  it('reads account, plan and every window', () => {
    const merged = mergeParsed(parseAccountScreen(settings, ACCOUNT_SPECS.claude, now), parseAccountScreen(usage, ACCOUNT_SPECS.claude, now));
    expect(merged.account).toBe('someone@example.com');
    expect(merged.plan).toBe('Max');
    expect(merged.windows.map((w) => [w.id, w.label, w.remaining_pct])).toEqual([
      ['five_hour', '5-hour', 100],
      ['weekly', 'Weekly', 40],
      ['weekly_fable', 'Weekly (Fable)', 100],
    ]);
    expect(new Date(merged.windows[1].resets_at!).getDay()).toBe(4);
  });

  it('never picks the credits purchase button', () => {
    expect(pickTap(screenElements(usage), { tap: ['Buy usage credits'] })).toBeNull();
  });
});

// Lines copied from the real apps on an iPhone (2026-10-19), every Settings
// page scrolled; emails, names and handles replaced.
describe('ChatGPT on iPhone (real screens)', () => {
  const settings = [
    "Button, label: 'Email, someone@example.com', {{16, 221}, {361, 74}}",
    "Button, label: 'Subscription, Plus', {{16, 348}, {361, 52}}",
    "Button, label: 'Restore purchases', {{16, 400}, {361, 52}}",
  ].join('\n');
  const page2 = "Button, label: 'Usage and limits', {{16, 504}, {361, 51}}";
  const usage = [
    "Button, label: 'Back', id: 'BackButton', {{16, 59}, {44, 44}}",
    "StaticText, label: 'Agentic usage limit', {{32, 124}, {151, 20}}",
    "StaticText, label: 'Weekly usage limit', {{32, 178}, {141, 20}}",
    "StaticText, label: '95% remaining', {{254, 180}, {106, 18}}",
    "StaticText, label: 'Resets Sat 11:15 PM', {{32, 226}, {111, 14}}",
    "StaticText, label: 'Credits remaining, 0 Credits', {{32, 330}, {300, 20}}",
    "Button, label: 'Buy credits', {{32, 430}, {120, 40}}",
  ].join('\n');
  const now = new Date(2026, 9, 19, 16, 0, 0);
  it('finds "Usage and limits" on Settings page 2 and backs out', () => {
    const row = stepOf('chatgpt', 'Usage and limits');
    expect(row).toMatchObject({ tap: ['Usage and limits'], scroll: 2 });
    expect(pickTap(screenElements(settings), row)).toBeNull();
    expect(pickTap(screenElements(page2), row)).toEqual({ element_id: 'Usage and limits' });
    expect(pickTap(screenElements(usage), stepOf('chatgpt', 'id:BackButton'))).toEqual({ element_id: 'BackButton' });
    // Left on Usage by a cut-short read: the path's first taps back out of it.
    expect(ACCOUNT_SPECS.chatgpt.paths[0].steps[0]).toEqual({ tap: ['id:BackButton'], optional: true });
    expect(pickTap(screenElements(settings), { tap: ['Restore purchases'] })).toBeNull();
    expect(pickTap(screenElements(usage), { tap: ['Buy credits'] })).toBeNull();
  });

  it('reads account, plan and the weekly window', () => {
    const merged = mergeParsed(parseAccountScreen(settings, ACCOUNT_SPECS.chatgpt, now), parseAccountScreen(usage, ACCOUNT_SPECS.chatgpt, now));
    expect(merged.account).toBe('someone@example.com');
    expect(merged.plan).toBe('Plus');
    expect(merged.windows).toEqual([{ id: 'weekly', label: 'Weekly', remaining_pct: 95, resets_at: local(2026, 9, 24, 23, 15) }]);
  });
});

describe('Gemini on iPhone (real screens)', () => {
  const home =
    "Button, label: 'Account and settings. Signed in as gizzi, someone@example.com', id: 'kOGLAccessibilityIdentifierAccountParticleDiscButton', {{326, 52}, {58, 58}}";
  const sheet = "Button, label: 'Settings ', {{16, 340}, {361, 56}}";
  const settings = [
    "Button, label: 'Upgrade to Google AI Plus', {{16, 197}, {361, 54}}",
    "Button, label: 'Usage limits', {{16, 251}, {361, 54}}",
  ].join('\n');
  // The element tree lists the weekly window without its percentage; the visible text has it.
  const tree = [
    "StaticText, label: 'Your plan's limits determine how much you can use Gemini over time. Advanced models and features can take up more usage. Learn more', {{32, 206}, {318, 56}}",
    "StaticText, label: 'Current usage', {{32, 330}, {110, 21}}",
    "StaticText, label: '0% used', {{295, 330}, {66, 21}}",
    "StaticText, label: 'Resets at 8:54 PM', {{32, 379}, {116, 19}}",
    "StaticText, label: 'Weekly limit', {{32, 432}, {85, 19}}",
    "StaticText, label: 'Resets at 6:54 PM', {{32, 455}, {103, 16}}",
    "Button, label: 'Upgrade', {{278, 515}, {90, 48}}",
    "Switch, label: 'Auto model selection', value: '1', {{313, 678}, {52, 32}}",
  ].join('\n');
  const text = ['Current usage ① @ (96, 343)', 'Resets at 8:54PM @ (90, 389)', '0% used @ (329, 341)', 'Weekly limit @ (75, 443)', 'Resets at 6:54PM @ (84, 464)', '0% used @ (331, 452)'].join('\n');
  const now = new Date(2026, 9, 19, 16, 1, 0);
  it('opens the account sheet, Settings (trailing space) and Usage limits', () => {
    expect(pickTap(screenElements(home), stepOf('gemini', 'id:kOGLAccessibilityIdentifierAccountParticleDiscButton'))).toEqual({ element_id: 'kOGLAccessibilityIdentifierAccountParticleDiscButton' });
    expect(pickTap(screenElements(sheet), stepOf('gemini', 'Settings'))).toEqual({ element_id: 'Settings ' });
    expect(pickTap(screenElements(settings), stepOf('gemini', 'Usage limits'))).toEqual({ element_id: 'Usage limits' });
    expect(pickTap(screenElements(tree), { tap: ['Upgrade', 'Auto model selection'] })).toBeNull();
  });

  it('reads the account and both windows, filling the weekly percentage from the visible text', () => {
    const merged = mergeParsed(parseAccountScreen(home, ACCOUNT_SPECS.gemini, now), parseAccountScreen(`${tree}\n${text}`, ACCOUNT_SPECS.gemini, now));
    expect(merged.account).toBe('someone@example.com');
    expect(merged.plan).toBeNull();
    expect(merged.windows).toEqual([
      { id: 'current', label: 'Current', remaining_pct: 100, resets_at: local(2026, 9, 19, 20, 54) },
      { id: 'weekly', label: 'Weekly', remaining_pct: 100, resets_at: local(2026, 9, 19, 18, 54) },
    ]);
  });
});

describe('apps that are not AI chat, on iPhone (real screens)', () => {
  const spec = (app: string) => ACCOUNT_SPECS[app];
  const first = (app: string) => (spec(app).paths[0].steps as AccountTapStep[]).find((st) => 'tap' in st && !st.optional)!;

  it('Discord: the status button by pattern, then the account email (or username)', () => {
    const home = "Button, label: 'Joedirt, Online', {{36, 774}, {257, 32}}";
    expect(pickTap(screenElements(home), first('discord'))).toEqual({ element_id: 'Joedirt, Online' });
    expect(pickTap(screenElements("Button, label: 'Back 19,457 mentions', {{16, 60}, {44, 44}}"), stepOf('discord', 're:^back( [\\d,]+ mentions?)?$'))).toEqual({ element_id: 'Back 19,457 mentions' });
    const account = [
      "Button, label: 'Username, someone01440', {{16, 178}, {361, 56}}",
      "Button, label: 'Display Name, Someone', {{16, 234}, {361, 56}}",
      "Button, label: 'Email, someone@example.com', {{16, 290}, {361, 56}}",
      "Button, label: 'Delete Account', {{16, 1080}, {361, 56}}",
    ].join('\n');
    expect(parseAccountScreen(account, spec('discord'), NOW).account).toBe('someone@example.com');
    expect(parseAccountScreen(account.split('\n')[0], spec('discord'), NOW).account).toBe('someone01440');
    expect(pickTap(screenElements(account), { tap: ['Delete Account'] })).toBeNull();
  });

  it('Telegram: taps the Settings tab where the text is, then reads the handle after the phone number', () => {
    const chats = ['Contacts @ (68, 815)', 'Calls @ (152, 815)', 'Chats @ (239, 815)', 'Settings @ (324, 816)'].join('\n');
    expect(pickTap(screenElements("Other, label: 'Allternit', {{0, 100}, {393, 76}}"), first('telegram'))).toBeNull();
    expect(pickTextTap(chats, first('telegram'))).toEqual({ x: 324, y: 816, label: 'Settings' });
    const settings = ['Someone @ (196, 202)', '+1555 555 0100•@someone @ (197, 230)', 'Add Account @ (127, 440)'].join('\n');
    expect(parseAccountScreen(settings, spec('telegram'), NOW).account).toBe('@someone');
    expect(pickTextTap(settings, { tap: ['Add Account'] })).toBeNull();
  });

  it('Instagram: the username just before the creation-menu button, not a story tile', () => {
    const profile = [
      "Button, label: 'someone_mc', value: 'Other profiles have notifications', id: 'user-switch-title-button', {{128, 29}, {135, 109}}",
      "StaticText, label: 'someone_mc', {{128, 69}, {107, 28}}",
      "Button, label: 'Tap to open creation menu', id: 'profile-add-button', {{16, 62}, {44, 44}}",
    ].join('\n');
    expect(pickTap(screenElements("Button, label: 'Profile', id: 'profile-tab', {{295, 770}, {65, 60}}"), first('instagram'))).toEqual({ element_id: 'profile-tab' });
    expect(parseAccountScreen(profile, spec('instagram'), NOW).account).toBe('someone_mc');
  });

  it("Facebook: the profile name, never the bio's email", () => {
    const profile = [
      "Button, label: 'Edit profile picture', {{234, 250}, {36, 36}}",
      'StaticText, {{131, 300}, {1, 1}}',
      "StaticText, label: 'Some One ', {{131, 307}, {134, 26}}",
      "Button, label: 'Some One', {{131, 306}, {129, 28}}",
      "Button, label: 'Instagram-Snapchat-Twitter : @someone                        booking@example.com', {{0, 367}, {393, 44}}",
    ].join('\n');
    expect(parseAccountScreen(profile, spec('facebook'), NOW).account).toBe('Some One');
  });

  it("TikTok and X: the person's own @handle (X wraps it in bidi marks); a feed's handles are never read", () => {
    expect(first('tiktok')).toMatchObject({ tap: ['id:a11y_vo_profile', 'Profile'] });
    expect(parseAccountScreen(['What’s up? @ (197, 96)', '@someone @ (197, 240)', 'Edit @ (311, 217)'].join('\n'), spec('tiktok'), NOW).account).toBe('@someone');
    const menu = "Button, label: '⁦‪@Someone‬⁩', {{32, 144}, {103, 18}}";
    expect(parseAccountScreen(menu, spec('twitter'), NOW).account).toBe('@Someone');
    expect(pickTap(screenElements("Button, label: 'Account Menu', id: 'NavigationBarDashButton', {{16, 65}, {32, 32}}"), first('twitter'))).toEqual({ element_id: 'NavigationBarDashButton' });
  });

  it('YouTube: the Google account email on Accounts; GitHub: the login in the navigation bar', () => {
    const accounts = ["StaticText, label: 'someone@example.com', {{12, 133}, {132, 16}}", "StaticText, label: '@somechannel', {{74, 199}, {69, 17}}"].join('\n');
    expect(parseAccountScreen(accounts, spec('youtube'), NOW).account).toBe('someone@example.com');
    const github = ["Button, label: 'Someone', {{20, 60}, {54, 42}}", "Button, label: 'Add', {{281, 63}, {36, 36}}", "Button, label: 'Search', {{337, 63}, {36, 36}}"].join('\n');
    expect(parseAccountScreen(github, spec('github'), NOW).account).toBe('Someone');
  });
});

// Lines copied from the real apps on Android (moto g play, 2026-10-19); names and emails replaced.
describe('apps on Android (real screens)', () => {
  const spec = (app: string) => ACCOUNT_SPECS[app];
  // The first tap that isn't a back-out ("Back" / "Back button").
  const androidStep = (app: string) =>
    (spec(app).paths.find((p) => p.verifiedOn === 'android')!.steps as AccountTapStep[]).find((st) => 'tap' in st && !/^back/i.test(st.tap[0]))!;

  it('parses the Android helper\'s tree, which adds flags after the bounds', () => {
    const tabs = "FrameLayout, label: 'Profile', id: 'com.instagram.android:id/profile_tab', {{576, 1439}, {144, 77}}, clickable long-clickable";
    expect(screenElements(tabs)).toEqual([{ className: 'FrameLayout', label: 'Profile', id: 'com.instagram.android:id/profile_tab' }]);
    expect(pickTap(screenElements(tabs), androidStep('instagram'))).toEqual({ element_id: 'com.instagram.android:id/profile_tab' });
    const profile = "TextView, label: 'someone', id: 'com.instagram.android:id/action_bar_title', {{300, 100}, {100, 40}}";
    expect(parseAccountScreen(profile, spec('instagram'), NOW).account).toBe('someone');
  });

  it('Google apps: the account button names the account, never an inbox sender', () => {
    const gmail = [
      "FrameLayout, label: 'Signed in as Some One someone@example.com\nAccount and settings.', id: 'com.google.android.gm:id/selected_account_disc_gmail', {{620, 90}, {80, 80}}, clickable",
    ].join('\n');
    // A label can carry a newline; the tree line is still one element.
    expect(parseAccountScreen(gmail.replace('\n', '\\n'), spec('gmail'), NOW).account).toBe('someone@example.com');
    expect(parseAccountScreen(['billing@vendor.example @ (10, 300)', 'Signed in as Some One someone@example.com @ (650, 120)'].join('\n'), spec('gmail'), NOW).account).toBe('someone@example.com');
    for (const app of ['google_drive', 'googlecalendar', 'googlemeet', 'googlephotos']) expect(spec(app).paths[0].verifiedOn).toBe('android');
  });

  it('LinkedIn, X and Discord on Android', () => {
    const linkedin = ["View, label: 'Edit profile', {{600, 400}, {60, 60}}, clickable", "TextView, label: 'Some Company LLC', {{40, 480}, {400, 40}}"].join('\n');
    expect(parseAccountScreen(linkedin, spec('linkedin'), NOW).account).toBe('Some Company LLC');
    expect(androidStep('twitter')).toMatchObject({ tap: ['Show navigation drawer'] });
    expect(parseAccountScreen("TextView, label: '@someone', {{40, 200}, {200, 30}}", spec('twitter'), NOW).account).toBe('@someone');
    const discord = "Button, label: 'someone, Online', {{0, 1400}, {300, 80}}, clickable";
    expect(pickTap(screenElements(discord), androidStep('discord'))).toEqual({ element_id: 'someone, Online' });
  });
});
