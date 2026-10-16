import { beforeEach, describe, expect, it, vi } from 'vitest';
import { EventEmitter } from 'node:events';

const mocks = vi.hoisted(() => ({ appEvents: new Map<string, Function>(), handlers: new Map<string, Function>(), events: new Map<string, Function>(), windows: [] as any[], menu: [] as any[], stores: [] as any[], watchers: [] as Array<(event: string, file: string) => void> }));
vi.mock('node:fs', async (importOriginal) => ({ ...(await importOriginal<typeof import('node:fs')>()), watch: (_dir: string, cb: any) => { mocks.watchers.push(cb); return { unref() {} }; } }));
vi.mock('electron-store', () => ({ default: class {
  store: any;
  path = '/tmp/@allternit/desktop/desktop-companion.json';
  constructor(options: any) { this.store = { ...options.defaults }; mocks.stores.push(this); }
  set(key: string, value: unknown) { this.store[key] = value; }
  delete(key: string) { delete this.store[key]; }
} }));
vi.mock('electron', () => ({
  app: { whenReady: () => Promise.resolve(), on: (name: string, fn: Function) => mocks.appEvents.set(name, fn) },
  screen: { getPrimaryDisplay: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getDisplayNearestPoint: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getDisplayMatching: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getCursorScreenPoint: () => ({ x: 100, y: 100 }), on: vi.fn() },
  ipcMain: { handle: (name: string, fn: Function) => mocks.handlers.set(name, fn), on: (name: string, fn: Function) => mocks.events.set(name, fn) },
  Menu: { buildFromTemplate: (items: any[]) => { mocks.menu = items; return { popup: vi.fn() }; } },
  BrowserWindow: class extends EventEmitter {
    bounds: any; visible = false; options: any;
    webContents = Object.assign(new EventEmitter(), { mainFrame: {}, send: vi.fn(), setWindowOpenHandler: vi.fn(), executeJavaScript: vi.fn(async () => true) });
    constructor(options: any) { super(); this.options = options; this.bounds = options; mocks.windows.push(this); }
    isDestroyed() { return false; }
    focused = false;
    isVisible() { return this.visible; } isFocused() { return this.focused; }
    setAlwaysOnTop() {} setVisibleOnAllWorkspaces() {} setIgnoreMouseEvents() {}
    loadURL = vi.fn(async () => {});
    getBounds() { return this.bounds; }
    setBounds(bounds: any) { this.bounds = bounds; }
    setPosition(x: number, y: number) { this.bounds = { ...this.bounds, x, y }; }
    show() { this.visible = true; } showInactive() { this.visible = true; } hide() { this.visible = false; } focus() { this.focused = true; } moveTop() {}
  },
}));
import { installDesktopCompanion } from './desktop-companion.js';

function sender(win: any) { return { sender: win.webContents, senderFrame: win.webContents.mainFrame }; }
beforeEach(() => { mocks.appEvents.clear(); mocks.windows.length = 0; mocks.handlers.clear(); mocks.events.clear(); mocks.stores.length = 0; mocks.watchers.length = 0; });
describe('desktop companion windows', () => {
  it('follows a bot picked by the gizzi terminal pet', async () => {
    vi.useFakeTimers();
    try {
      const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => null, preload: '/preload.js' });
      manager.start();
      const pet = mocks.windows[0];
      const store = mocks.stores[0];
      pet.webContents.send.mockClear();
      // Another file in the folder changing, with agentId unchanged: no broadcast.
      mocks.watchers[0]!('change', 'config.json');
      vi.advanceTimersByTime(200);
      expect(pet.webContents.send).not.toHaveBeenCalled();
      // The terminal replaces our settings file via a temp file; macOS may
      // report only the temp file's name.
      store.store.agentId = 'bot-from-terminal';
      mocks.watchers[0]!('rename', '.desktop-companion.json.123.tmp');
      vi.advanceTimersByTime(200);
      expect(pet.webContents.send).toHaveBeenCalledWith('companion:state', expect.objectContaining({ agentId: 'bot-from-terminal' }));
      // Desktop's own writes don't re-broadcast.
      mocks.handlers.get('companion:update')!(sender(pet), { agentId: 'bot-2' });
      pet.webContents.send.mockClear();
      mocks.watchers[0]!('change', 'desktop-companion.json');
      vi.advanceTimersByTime(200);
      expect(pet.webContents.send).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
  it('opens the bot HUD and keeps the pet anchored and scaled as HUD bounds change', () => {
    const openHud = vi.fn();
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => null, preload: '/preload.js', openHud });
    manager.start();
    const pet = mocks.windows[0];
    mocks.handlers.get('companion:update')!(sender(pet), { agentId: 'bot-1' });
    mocks.handlers.get('companion:open')!(sender(pet), 'chat');
    expect(openHud).toHaveBeenCalledWith('bot-1');
    expect(mocks.windows).toHaveLength(1);
    manager.attachPetToHud({ x: 200, y: 260, width: 600, height: 300 });
    expect(pet.getBounds()).toMatchObject({ x: 688, y: 192, width: 88, height: 88 });
    manager.attachPetToHud({ x: 400, y: 350, width: 800, height: 400 });
    expect(pet.getBounds()).toMatchObject({ x: 1064, y: 258, width: 112, height: 112 });
    mocks.handlers.get('companion:update')!(sender(pet), { agentId: 'bot-2' });
    expect(pet.getBounds()).toMatchObject({ x: 1064, y: 258, width: 112, height: 112 });
    mocks.handlers.get('companion:update')!(sender(pet), { size: 120 });
    expect(pet.getBounds()).toMatchObject({ x: 1064, y: 258, width: 112, height: 112 });
    manager.detachPetFromHud();
    expect(pet.getBounds().y).toBeGreaterThan(500);
  });
  it('uses an independent transparent pet and separate quick chat window', async () => {
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => null, preload: '/preload.js' });
    manager.start(); const pet = mocks.windows[0];
    expect(pet.options).toMatchObject({ frame: false, transparent: true, alwaysOnTop: true, skipTaskbar: true });
    expect(pet.options.parent).toBeUndefined();
    mocks.handlers.get('companion:open')!(sender(pet), 'chat');
    const chat = mocks.windows[1]; chat.webContents.emit('did-finish-load');
    expect(chat.loadURL).toHaveBeenCalledWith('http://localhost:8013/companion.html?surface=chat');
    await vi.waitFor(() => expect(chat.visible).toBe(true));
  });
  it('clamps size, hides/restores, and resets an offscreen saved position', async () => {
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => null, preload: '/preload.js' });
    manager.start(); const pet = mocks.windows[0]; const event = sender(pet);
    pet.webContents.emit('did-finish-load'); await vi.waitFor(() => expect(pet.visible).toBe(true));
    mocks.handlers.get('companion:update')!(event, { size: 999 });
    expect(pet.getBounds().width).toBe(176);
    mocks.handlers.get('companion:update')!(event, { enabled: false }); expect(pet.visible).toBe(false);
    manager.show(); expect(pet.visible).toBe(true);
    pet.setPosition(-9000, 9000); mocks.events.get('companion:drag')!(event, 'end');
    expect(pet.getBounds().x).toBeGreaterThanOrEqual(0);
    manager.reset(); expect(pet.getBounds()).toMatchObject({ x: 1080, y: 600 });
  });
  it('rejects commands from unrelated windows and child frames', () => {
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => null, preload: '/preload.js' });
    manager.start(); const pet = mocks.windows[0];
    expect(() => mocks.handlers.get('companion:update')!({ sender: {}, senderFrame: {} }, { enabled: false })).toThrow('Untrusted');
    expect(() => mocks.handlers.get('companion:update')!({ ...sender(pet), senderFrame: {} }, { enabled: false })).toThrow('Untrusted');
  });
  it('keeps the pet hidden if the server falls back to the workspace page', async () => {
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => null, preload: '/preload.js' });
    manager.start(); const pet = mocks.windows[0];
    pet.webContents.executeJavaScript.mockResolvedValue(false);
    pet.webContents.emit('did-finish-load'); await Promise.resolve();
    expect(pet.visible).toBe(false);
    manager.show();
    expect(pet.visible).toBe(false);
  });
});

describe('desktop pet activity', () => {
  async function setup(opts: { showMain?: () => void } = {}) {
    const main = new (await import('electron')).BrowserWindow({}) as any;
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => main, preload: '/preload.js', ...opts });
    manager.start();
    const pet = mocks.windows[1];
    pet.webContents.emit('did-finish-load');
    await vi.waitFor(() => expect(pet.visible).toBe(true));
    // The pet page announces it can walk by sending its motion preference.
    mocks.events.get('companion:motion-pref')!(sender(pet), false);
    return { main, pet, manager, report: (state: unknown) => mocks.events.get('companion:activity')!(sender(main), state) };
  }
  const lastActivity = (pet: any) => pet.webContents.send.mock.calls.filter((c: any[]) => c[0] === 'companion:activity').at(-1)?.[1];

  it('relays the app window state to the pet, only from the app window', async () => {
    const { pet, report } = await setup();
    report('running');
    expect(lastActivity(pet)).toMatchObject({ state: 'running' });
    mocks.events.get('companion:activity')!(sender(pet), 'failed');
    report('not-a-state');
    expect(lastActivity(pet)).toMatchObject({ state: 'running' });
  });

  it('shows an unread badge for a turn that finished while you were away, cleared on focus', async () => {
    const { main, pet, report } = await setup();
    main.visible = true; main.focused = false;
    report('review'); report('waving');
    expect(lastActivity(pet)).toMatchObject({ state: 'waving', unread: true });
    mocks.appEvents.get('browser-window-focus')!({}, main);
    expect(lastActivity(pet)).toMatchObject({ unread: false });
    main.focused = true;
    report('review'); report('waving');
    expect(lastActivity(pet)).toMatchObject({ unread: false });
  });

  it('terminal sources with a TTL expire on their own', async () => {
    vi.useFakeTimers();
    try {
      const { pet, manager } = await setup();
      manager.reportActivity('hook:claude', 'running', 1000);
      expect(lastActivity(pet)).toMatchObject({ state: 'running' });
      vi.advanceTimersByTime(1100);
      expect(lastActivity(pet)).toMatchObject({ state: 'idle' });
    } finally { vi.useRealTimers(); }
  });

  it('a terminal reply does not badge the pet (it is not waiting in the app)', async () => {
    const { main, pet, manager } = await setup();
    main.visible = false;
    manager.reportActivity('hook:claude', 'waving', 2400);
    expect(lastActivity(pet)).toMatchObject({ state: 'waving', unread: false });
  });

  it('shows speech bubbles above the pet, escaped and click-through, and hides them on time', async () => {
    vi.useFakeTimers();
    try {
      const { main, pet, manager, report } = await setup();
      expect(manager.showBubble('<b>Tests</b> pass', 3000)).toBe(true);
      const bubble = mocks.windows.find(w => w.options.focusable === false)!;
      const url = decodeURIComponent(bubble.loadURL.mock.calls.at(-1)[0]);
      expect(url).toContain('&lt;b&gt;Tests&lt;/b&gt; pass');
      expect(url).toContain("default-src 'none'");
      expect(bubble.options.webPreferences.javascript).toBe(false);
      await vi.waitFor(() => expect(bubble.visible).toBe(true));
      expect(bubble.bounds.y + bubble.bounds.height).toBeLessThanOrEqual(pet.bounds.y + 6);
      vi.advanceTimersByTime(3100);
      expect(bubble.visible).toBe(false);
      // The app's own events speak too: needing an answer, and a reply that finished while you were away.
      main.visible = false;
      report('waiting');
      expect(decodeURIComponent(bubble.loadURL.mock.calls.at(-1)[0])).toContain('Needs your answer');
      report('review'); report('waving');
      expect(decodeURIComponent(bubble.loadURL.mock.calls.at(-1)[0])).toContain('Reply ready');
      // Never while the pet is hidden.
      pet.visible = false;
      expect(manager.showBubble('hi')).toBe(false);
    } finally { vi.useRealTimers(); }
  });

  it('toggles the main window', async () => {
    const showMain = vi.fn();
    const { main, pet } = await setup({ showMain });
    await mocks.handlers.get('companion:toggle-main')!(sender(pet));
    expect(showMain).toHaveBeenCalledTimes(1);
    main.visible = true; main.focused = true;
    await mocks.handlers.get('companion:toggle-main')!(sender(pet));
    expect(main.visible).toBe(false);
  });

  it('walks along its row when idle, and stops for activity, reduced motion or the setting', async () => {
    vi.useFakeTimers();
    try {
      // Mid-range delays (12 s) and targets, so the walk timing is deterministic.
      vi.spyOn(Math, 'random').mockReturnValue(0.5);
      const { pet, report } = await setup();
      const start = pet.getBounds();
      vi.advanceTimersByTime(12_100);
      expect(lastActivity(pet).moving).toMatch(/left|right/);
      vi.advanceTimersByTime(500);
      expect(pet.getBounds().x).not.toBe(start.x);
      expect(pet.getBounds().y).toBe(start.y);
      // Activity stops the walk and saves where it stopped, on the same row.
      report('running');
      expect(lastActivity(pet)).toMatchObject({ state: 'running', moving: null });
      expect(mocks.stores[0].store.position).toEqual({ x: pet.getBounds().x, y: start.y });
      vi.advanceTimersByTime(60_000);
      expect(lastActivity(pet).moving).toBeNull();
      report('idle');
      vi.advanceTimersByTime(12_100);
      expect(lastActivity(pet).moving).toMatch(/left|right/);
      report('idle');
      mocks.events.get('companion:motion-pref')!(sender(pet), true);
      vi.advanceTimersByTime(60_000);
      expect(lastActivity(pet).moving).toBeNull();
      mocks.events.get('companion:motion-pref')!(sender(pet), false);
      mocks.handlers.get('companion:update')!(sender(pet), { roam: false });
      vi.advanceTimersByTime(60_000);
      expect(lastActivity(pet).moving).toBeNull();
    } finally { vi.useRealTimers(); }
  });
  it('never roams for a pet page that has not said it can walk', async () => {
    vi.useFakeTimers();
    try {
      const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => null, preload: '/preload.js' });
      manager.start();
      const pet = mocks.windows[0];
      pet.webContents.emit('did-finish-load');
      await vi.advanceTimersByTimeAsync(0);
      const x = pet.getBounds().x;
      await vi.advanceTimersByTimeAsync(60_000);
      expect(pet.getBounds().x).toBe(x);
    } finally { vi.useRealTimers(); }
  });
});
