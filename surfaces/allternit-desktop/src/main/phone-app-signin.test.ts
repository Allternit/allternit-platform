import { describe, expect, it } from 'vitest';
import { IctrlError } from './ictrl-client.js';
import { catalogApp } from './phone-app-catalog.js';
import { checkLinkedApp, classifySignIn, openLinkedApp, screenLabels, visibleAccount } from './phone-app-signin.js';
import { PhoneLinkBackend } from './phone-link-backend.js';
import type { LinkedPhone } from './phone-link-health.js';
import type { IctrlLike } from './phone-link-ios.js';
import type { PhoneLinkApp, PhoneLinkHealth } from './phone-link-types.js';

type Handler = (params: Record<string, unknown>) => unknown;

function fakeClient(handlers: Record<string, Handler>) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const client: IctrlLike = {
    host: '192.168.1.20:8200',
    hasKey: () => true,
    forget: () => {},
    whoami: async () => ({ device_id: 'dev-1' }),
    pair: async () => ({}),
    withHost: () => client,
    call: async <T = Record<string, unknown>>(method: string, params: Record<string, unknown> = {}): Promise<T> => {
      calls.push({ method, params });
      const handler = handlers[method];
      if (!handler) throw new IctrlError('rpc_error', `no handler for ${method}`);
      return handler(params) as T;
    },
  };
  return { client, calls };
}

const iphone: LinkedPhone = { deviceId: 'dev-1', platform: 'ios', name: 'iPhone 15', lanHost: '192.168.1.20:8200', storeKey: 'k1', apps: [{ vendor: 'chatgpt', signedIn: null, name: 'ChatGPT', category: 'AI chat' }] };
const healthy: PhoneLinkHealth = { state: 'HEALTHY', checkedAt: '2026-10-18T00:00:00.000Z', recovery: 'automatic' };

function setup(handlers: Record<string, Handler>, health: PhoneLinkHealth = healthy) {
  const { client, calls } = fakeClient(handlers);
  const saved: PhoneLinkApp[][] = [];
  const phone = { ...iphone };
  const lane = new PhoneLinkBackend({
    phones: () => [phone],
    client: () => client,
    health: { health: () => health } as never,
    saveApps: (_p, apps) => {
      saved.push(apps);
      phone.apps = apps;
    },
  });
  return { lane, calls, saved, deps: { lane, sleep: async () => {} } };
}

const screen = (...labels: string[]) => ({ items: labels.map((label, i) => ({ label, center_x: 10, center_y: 10 + i })) });

describe('classifySignIn', () => {
  const chatgpt = catalogApp('chatgpt')!;
  const slack = catalogApp('slack')!;

  it('reads a sign-in gate as signed out, even next to a composer', () => {
    expect(classifySignIn(chatgpt, 'Ask anything @ (10, 20)\nLog in @ (10, 40)\nSign up @ (10, 60)')).toEqual({ signedIn: false, account: null });
    expect(classifySignIn(slack, 'Continue with Google\nContinue with Apple')).toEqual({ signedIn: false, account: null });
  });

  it('reads the AI chat composer as signed in, with the visible email', () => {
    expect(classifySignIn(chatgpt, 'Message ChatGPT\nNew chat\neoj@example.com')).toEqual({ signedIn: true, account: 'eoj@example.com' });
  });

  it('uses category markers and Sign out for other apps', () => {
    // Other apps' screens are full of other people's handles: no account from the sign-in check.
    expect(classifySignIn(slack, 'Direct messages\nActivity\n@eoj')).toEqual({ signedIn: true, account: null });
    expect(classifySignIn(catalogApp('discord')!, 'Messages\nAdd Friends')).toEqual({ signedIn: true, account: null });
    expect(classifySignIn(catalogApp('kimi')!, 'Session expired. Please log in again.')).toEqual({ signedIn: false, account: null });
    expect(classifySignIn(catalogApp('notion')!, 'Sign out')).toEqual({ signedIn: true, account: null });
  });

  it('is null when the screen shows neither', () => {
    expect(classifySignIn(slack, 'Loading…')).toEqual({ signedIn: null, account: null });
    // A sentence that merely mentions signing in is not a gate.
    expect(classifySignIn(slack, 'Sign in to another workspace to see more')).toEqual({ signedIn: null, account: null });
  });

  it('strips the lane suffix and read_ui quoting', () => {
    expect(screenLabels('XCUIElementTypeButton "Log in" {{1, 2}, {3, 4}}\nNew chat @ (1, 2)')).toEqual(['Log in', 'New chat']);
    expect(visibleAccount(['Hello', 'Settings'])).toBeNull();
  });
});

describe('phone-link:open-app / check-app', () => {
  it('opens the catalog app by its bundle id on the iPhone', async () => {
    const { deps, calls } = setup({ launch_app: () => ({}) });
    expect(await openLinkedApp(deps, { deviceId: 'dev-1', app: 'chatgpt' })).toEqual({ ok: true, signedIn: null, account: null });
    expect(calls).toEqual([{ method: 'launch_app', params: { bundle_id: 'com.openai.chat' } }]);
  });

  it('checks the screen, never taps or types, and caches signedIn for phone-link:devices', async () => {
    const { deps, calls, saved } = setup({ launch_app: () => ({}), screen_text: () => screen('Message ChatGPT', 'eoj@example.com') });
    expect(await checkLinkedApp(deps, { deviceId: 'dev-1', app: 'chatgpt' })).toEqual({ ok: true, signedIn: true, account: 'eoj@example.com' });
    expect(calls.map((c) => c.method)).toEqual(['launch_app', 'screen_text']);
    expect(saved.at(-1)).toEqual([{ vendor: 'chatgpt', signedIn: true, name: 'ChatGPT', category: 'AI chat' }]);
  });

  it('looks twice before answering null, and adds an app the cached list lacked', async () => {
    const { deps, calls, saved } = setup({ launch_app: () => ({}), screen_text: () => screen('Loading') });
    expect(await checkLinkedApp(deps, { deviceId: 'dev-1', app: 'slack' })).toEqual({ ok: true, signedIn: null, account: null });
    expect(calls.filter((c) => c.method === 'screen_text')).toHaveLength(2);
    expect(saved.at(-1)?.map((a) => a.vendor)).toEqual(['chatgpt', 'slack']);
  });

  it('refuses a locked or offline phone with the health wording', async () => {
    const locked = setup({}, { ...healthy, state: 'DEVICE_LOCKED', recovery: 'manual', action: 'Unlock your phone.' });
    expect(await checkLinkedApp(locked.deps, { deviceId: 'dev-1', app: 'chatgpt' })).toMatchObject({
      ok: false,
      error: { code: 'device_locked', message: 'Unlock your phone.' },
    });
    expect(locked.calls).toEqual([]);
    const offline = setup({}, { ...healthy, state: 'TRANSPORT_OFFLINE' });
    expect((await openLinkedApp(offline.deps, { deviceId: 'dev-1', app: 'chatgpt' })).error?.code).toBe('device_offline');
  });

  it('reports unknown phones, unknown apps and apps that are not installed', async () => {
    const { deps } = setup({ launch_app: () => { throw new IctrlError('rpc_error', 'not installed'); } });
    expect((await openLinkedApp(deps, { deviceId: 'nope', app: 'chatgpt' })).error?.code).toBe('device_not_found');
    expect((await openLinkedApp(deps, { deviceId: 'dev-1', app: 'not-an-app' })).error?.code).toBe('unknown_app');
    expect((await openLinkedApp(deps, { deviceId: 'dev-1', app: 'slack' })).error?.code).toBe('not_installed');
  });
});
