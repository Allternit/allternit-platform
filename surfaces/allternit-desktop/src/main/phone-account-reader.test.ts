import { describe, expect, it } from 'vitest';
import { IctrlError } from './ictrl-client.js';
import {
  mergeParsed,
  parseAccountScreen,
  parsePlan,
  parseResetTime,
  parseUsageWindows,
  pickTap,
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

  it('refuses apps without a spec', async () => {
    const { lane } = setup({});
    expect((await readLinkedAccount({ lane }, { deviceId: 'and-1', app: 'slack' })).error?.code).toBe('unsupported_app');
  });
});

describe('PhoneAccountScheduler', () => {
  const base = (over: Partial<PhoneLinkApp> = {}): LinkedPhone => ({
    deviceId: 'and-1',
    platform: 'android',
    storeKey: 'k',
    apps: [
      { vendor: 'claude', signedIn: true, category: 'AI chat', ...over },
      { vendor: 'slack', signedIn: true, category: 'Communication' },
      { vendor: 'chatgpt', signedIn: false, category: 'AI chat' },
    ],
  });

  it('reads one due signed-in AI chat app per tick, then waits 6 h', async () => {
    let t = Date.parse('2026-10-18T12:00:00Z');
    const read: string[] = [];
    const s = new PhoneAccountScheduler({
      phones: () => [base()],
      health: () => healthy,
      busy: () => false,
      read: async (o) => {
        read.push(o.app);
        return { ok: true, account: null, plan: null, usage: null } as PhoneLinkAccountResult;
      },
      now: () => t,
    });
    expect((await s.tick())?.app).toBe('claude');
    expect(await s.tick()).toBeNull();
    t += 6 * 3600_000;
    expect((await s.tick())?.app).toBe('claude');
    expect(read).toEqual(['claude', 'claude']);
  });

  it('skips locked, unhealthy and busy phones, and fresh reads', async () => {
    const mk = (health: PhoneLinkHealth['state'], busy: boolean, phone = base()) =>
      new PhoneAccountScheduler({
        phones: () => [phone],
        health: () => ({ ...healthy, state: health }),
        busy: () => busy,
        read: async () => ({ ok: true, account: null, plan: null, usage: null }),
        now: () => Date.parse('2026-10-18T12:00:00Z'),
      });
    expect(mk('DEVICE_LOCKED', false).due()).toBeNull();
    expect(mk('ACCESSIBILITY_OFF', false).due()).toBeNull();
    expect(mk('HEALTHY', true).due()).toBeNull();
    expect(mk('HEALTHY', false, base({ accountCheckedAt: '2026-10-18T10:00:00Z' })).due()).toBeNull();
    expect(mk('HEALTHY', false).due()).toEqual({ deviceId: 'and-1', app: 'claude' });
  });
});

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
    const steps = ACCOUNT_SPECS.claude.paths[0].steps as AccountTapStep[];
    expect(pickTap(screenElements(sidebar), steps[0])).toEqual({ element_id: 'drawer-toggle' });
    expect(pickTap(screenElements(sidebar), steps[1])).toEqual({ element_id: 'sidebar-account-button' });
    expect(pickTap(screenElements(settings), steps[3])).toEqual({ element_id: 'Usage' });
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
