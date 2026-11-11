import { describe, expect, it } from 'vitest';

import { LookMemo, bankingHost, lookAt, lookText, pickLookTarget, type LookWindow } from './pet-look.js';
import { petOfferSettingsDefaults } from './pet-offers.gen.js';

const settings = (patch = {}) => ({ ...petOfferSettingsDefaults(), readOtherApps: true, ...patch });
const win = (patch: Partial<LookWindow> = {}): LookWindow => ({
  app: 'Terminal', appId: 'com.apple.Terminal', pid: 501, id: 9, title: 'zsh', bounds: { x: 0, y: 0, width: 800, height: 600 }, ...patch,
});

// The driver is a separate process (the edge): this stands in for its answers
// and records which methods were asked, which is what the order rule is about.
function fakeDriver(answers: { context?: Record<string, unknown>; elements?: unknown[]; down?: boolean }) {
  const calls: string[] = [];
  const driver = async (method: string) => {
    calls.push(method);
    if (answers.down) throw new Error('not running');
    if (method === 'context') return { pid: 501, app: 'Terminal', bundle_id: 'com.apple.Terminal', title: 'zsh', ...answers.context };
    return { elements: answers.elements ?? [{ role: 'TextArea', value: 'npm ERR! code 1' }] };
  };
  return { calls, driver };
}

describe('lookAt: order of checks', () => {
  it.each([
    ['a password manager by name', win({ app: '1Password', appId: 'com.1password.1password' })],
    ['a password manager by id under another name', win({ app: '1P', appId: 'com.1password.1password' })],
    ['a password manager whose name carries a version', win({ app: '1Password 7', appId: '' })],
    ['Messages', win({ app: 'Messages', appId: 'com.apple.MobileSMS' })],
    ['a banking app by name', win({ app: 'Chase Mobile', appId: 'com.chase.mobile' })],
    ['an app on the user block list', win({ app: 'Slack', appId: 'com.tinyspeck.slackmacgap' })],
  ])('never calls the driver for %s', async (_label, target) => {
    const { calls, driver } = fakeDriver({});
    const out = await lookAt(target, settings({ blockList: ['slack'] }), new LookMemo(), { driver, platform: 'darwin' });
    expect(out).toEqual({ skip: 'blocked_app' });
    expect(calls).toEqual([]);
  });

  it('a muted app is not read either', async () => {
    const { calls, driver } = fakeDriver({});
    expect(await lookAt(win(), settings({ muted: ['Terminal'] }), new LookMemo(), { driver, platform: 'darwin' })).toEqual({ skip: 'muted' });
    expect(calls).toEqual([]);
  });

  it.each([
    ['a private Safari window', { app: 'Safari', bundle_id: 'com.apple.Safari', title: 'Apple — Private Browsing' }, 'private_window'],
    ['an Incognito Chrome window', { app: 'Google Chrome', bundle_id: 'com.google.Chrome', title: 'New Incognito Tab' }, 'private_window'],
    ['a bank site', { app: 'Safari', bundle_id: 'com.apple.Safari', title: 'Accounts', url: 'https://secure.chase.com/x', host: 'secure.chase.com' }, 'blocked_host'],
    ['a browser whose title cannot be read', { app: 'Google Chrome', bundle_id: 'com.google.Chrome', title: '' }, 'browser_no_title'],
    ['an app the context names as blocked', { app: 'Bitwarden', bundle_id: 'com.bitwarden.desktop' }, 'blocked_app'],
  ])('reads the title, then stops for %s without reading the tree', async (_label, context, skip) => {
    const { calls, driver } = fakeDriver({ context });
    expect(await lookAt(win({ app: 'Browser', appId: '', title: '' }), settings(), new LookMemo(), { driver, platform: 'darwin' })).toEqual({ skip });
    expect(calls).toEqual(['context']);
  });

  it('reads an allowed window, then skips the same window and title for 30 s', async () => {
    const { calls, driver } = fakeDriver({});
    const memo = new LookMemo();
    let eye = 0;
    const out = await lookAt(win(), settings(), memo, { driver, platform: 'darwin', onRead: () => eye++ }, 1000);
    expect(out).toMatchObject({ app: 'Terminal', appId: 'com.apple.Terminal', title: 'zsh', windowKey: 'com.apple.Terminal|zsh', text: 'npm ERR! code 1' });
    expect(calls).toEqual(['context', 'read_ui']);
    expect(eye).toBe(1);
    expect(await lookAt(win(), settings(), memo, { driver, platform: 'darwin' }, 20_000)).toEqual({ skip: 'recent' });
    expect(await lookAt(win(), settings(), memo, { driver, platform: 'darwin' }, 31_001)).toMatchObject({ text: 'npm ERR! code 1' });
  });

  it('on Windows the exe name from the driver is the id, and a bank tab title blocks a browser with no URL', async () => {
    const terminal = fakeDriver({ context: { app: 'WindowsTerminal.exe', bundle_id: undefined, title: 'PowerShell' } });
    expect(await lookAt(win({ app: 'Windows Terminal', appId: '' }), settings({ muted: ['WindowsTerminal.exe'] }), new LookMemo(), { driver: terminal.driver, platform: 'win32' }))
      .toEqual({ skip: 'muted' });
    const edge = fakeDriver({ context: { app: 'msedge.exe', bundle_id: undefined, title: 'Wells Fargo - Accounts and 2 more pages' } });
    expect(await lookAt(win({ app: 'Microsoft Edge', appId: '' }), settings(), new LookMemo(), { driver: edge.driver, platform: 'win32' }))
      .toEqual({ skip: 'blocked_host' });
    expect(edge.calls).toEqual(['context']);
  });

  it('without the driver (not bundled, Windows build without it) Look returns nothing and does not throw', async () => {
    const { driver } = fakeDriver({ down: true });
    expect(await lookAt(win(), settings(), new LookMemo(), { driver, platform: 'win32' })).toEqual({ skip: 'no_driver' });
  });

  it('reads nothing on Linux', async () => {
    const { calls, driver } = fakeDriver({});
    expect(await lookAt(win(), settings(), new LookMemo(), { driver, platform: 'linux' })).toEqual({ skip: 'platform' });
    expect(calls).toEqual([]);
  });
});

describe('lookText', () => {
  const limits = { tailChars: 4000, maxTextChars: 8000 };

  it('drops secure and masked fields', () => {
    const { text } = lookText([
      { role: 'SecureTextField', value: 'hunter2' },
      { role: 'AXTextField', name: 'Password', value: '••••' },
      { role: 'StaticText', name: 'Sign in failed' },
    ], limits);
    expect(text).not.toContain('hunter2');
    expect(text).not.toContain('•');
    expect(text).toContain('Sign in failed');
  });

  it('keeps the tail of the largest element, last, so a terminal error at the bottom survives', () => {
    const buffer = `${'old line\n'.repeat(1000)}npm ERR! code ELIFECYCLE`;
    const { text } = lookText([{ role: 'Button', name: 'Clear' }, { role: 'TextArea', value: buffer }], limits);
    expect(text.length).toBeLessThanOrEqual('Clear\n'.length + 4000);
    expect(text.startsWith('Clear\n')).toBe(true);
    expect(text.endsWith('npm ERR! code ELIFECYCLE')).toBe(true);
  });

  it('caps the total at maxTextChars, keeping the end', () => {
    const many = Array.from({ length: 100 }, (_, i) => ({ role: 'StaticText', name: `row ${i} ${'x'.repeat(200)}` }));
    const { text } = lookText(many, limits);
    expect(text.length).toBe(8000);
    expect(text).toContain('row 99');
  });

  it('flags a dialog from a sheet or dialog role', () => {
    expect(lookText([{ role: 'AXSheet' }, { role: 'StaticText', name: 'The app quit unexpectedly.' }], limits).dialog).toBe(true);
    expect(lookText([{ role: 'Dialog', name: 'Error' }], limits).dialog).toBe(true);
    expect(lookText([{ role: 'StaticText', name: 'x' }], limits).dialog).toBe(false);
  });
});

describe('helpers', () => {
  it('banking hosts: listed suffixes and banking words, not lookalikes', () => {
    expect(bankingHost('www.wellsfargo.com')).toBe(true);
    expect(bankingHost('mybank.example')).toBe(true);
    expect(bankingHost('notchase.com')).toBe(false);
    expect(bankingHost('github.com')).toBe(false);
  });

  it('picks the window under the pet, else the frontmost', () => {
    const front = win({ id: 1, bounds: { x: 0, y: 0, width: 500, height: 500 } });
    const back = win({ id: 2, bounds: { x: 600, y: 0, width: 500, height: 500 } });
    expect(pickLookTarget([front, back], { x: 700, y: 100 })?.id).toBe(2);
    expect(pickLookTarget([front, back], { x: 2000, y: 2000 })?.id).toBe(1);
    expect(pickLookTarget([], null)).toBeNull();
  });
});
