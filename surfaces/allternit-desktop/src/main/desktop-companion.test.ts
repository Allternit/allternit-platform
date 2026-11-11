import { beforeEach, describe, expect, it, vi } from 'vitest';
import { EventEmitter } from 'node:events';

const mocks = vi.hoisted(() => ({ appEvents: new Map<string, Function>(), handlers: new Map<string, Function>(), events: new Map<string, Function>(), windows: [] as any[], menu: [] as any[], stores: [] as any[], watchers: [] as Array<(event: string, file: string) => void>, cursor: { x: 100, y: 100 } }));
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
  screen: { getPrimaryDisplay: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getDisplayNearestPoint: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getDisplayMatching: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getCursorScreenPoint: () => mocks.cursor, on: vi.fn() },
  ipcMain: { handle: (name: string, fn: Function) => mocks.handlers.set(name, fn), on: (name: string, fn: Function) => mocks.events.set(name, fn) },
  Menu: { buildFromTemplate: (items: any[]) => { mocks.menu = items; return { popup: vi.fn() }; } },
  BrowserWindow: class extends EventEmitter {
    bounds: any; visible = false; options: any;
    webContents = Object.assign(new EventEmitter(), { mainFrame: {}, send: vi.fn(), setWindowOpenHandler: vi.fn(), executeJavaScript: vi.fn(async () => true) });
    constructor(options: any) { super(); this.options = options; this.bounds = options; mocks.windows.push(this); }
    isDestroyed() { return false; }
    focused = false;
    isVisible() { return this.visible; } isFocused() { return this.focused; } isMinimized() { return false; }
    ignoreMouse = false;
    setAlwaysOnTop() {} setVisibleOnAllWorkspaces() {} setIgnoreMouseEvents(ignore: boolean) { this.ignoreMouse = ignore; }
    loadURL = vi.fn(async () => {});
    getBounds() { return this.bounds; }
    setBounds(bounds: any) { this.bounds = bounds; }
    setPosition(x: number, y: number) { this.bounds = { ...this.bounds, x, y }; }
    show() { this.visible = true; } showInactive() { this.visible = true; } hide() { this.visible = false; } focus() { this.focused = true; } moveTop() {}
  },
}));
import { installDesktopCompanion } from './desktop-companion.js';

function sender(win: any) { return { sender: win.webContents, senderFrame: win.webContents.mainFrame }; }
beforeEach(() => { mocks.cursor = { x: 100, y: 100 }; mocks.appEvents.clear(); mocks.windows.length = 0; mocks.handlers.clear(); mocks.events.clear(); mocks.stores.length = 0; mocks.watchers.length = 0; });
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

describe('carrying a bot out of the app', () => {
  async function setup() {
    vi.useFakeTimers();
    const main = new (await import('electron')).BrowserWindow({ x: 0, y: 0, width: 600, height: 500 }) as any;
    main.visible = true;
    const openHud = vi.fn(); const closeHud = vi.fn(); const onPetLanded = vi.fn();
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => main, preload: '/preload.js', openHud, closeHud, onPetLanded });
    const carry = (phase: string, agentId?: string) => mocks.events.get('companion:carry')!(sender(main), { phase, agentId });
    return { main, manager, openHud, closeHud, onPetLanded, carry, store: mocks.stores[0] };
  }

  it('follows the cursor out of the window and opens Quick Work where it is dropped', async () => {
    const { openHud, onPetLanded, carry, store } = await setup();
    try {
      store.store.enabled = false;
      mocks.cursor = { x: 900, y: 300 };
      carry('start', 'scout');
      const pet = mocks.windows[1];
      vi.advanceTimersByTime(20);
      expect(store.store).toMatchObject({ agentId: 'scout', enabled: true });
      expect(pet.getBounds()).toMatchObject({ x: 852, y: 252, width: 96 });
      mocks.cursor = { x: 1000, y: 400 };
      vi.advanceTimersByTime(20);
      expect(pet.getBounds()).toMatchObject({ x: 952, y: 352 });
      carry('drop');
      expect(openHud).toHaveBeenCalledWith('scout', expect.objectContaining({ x: 952, y: 352 }));
      expect(onPetLanded).toHaveBeenCalledWith(expect.objectContaining({ x: 952, y: 352 }));
      // The pet stops following after the drop.
      mocks.cursor = { x: 10, y: 10 };
      vi.advanceTimersByTime(50);
      expect(pet.getBounds()).toMatchObject({ x: 952, y: 352 });
    } finally { vi.useRealTimers(); }
  });

  it('goes home when dropped back on the app, restoring the pet it replaced', async () => {
    const { openHud, closeHud, carry, store, main } = await setup();
    try {
      store.store.enabled = false; store.store.agentId = 'old-pet';
      mocks.cursor = { x: 900, y: 300 };
      carry('start', 'scout');
      carry('drop');
      const pet = mocks.windows[1];
      // Later: the pet is dragged back onto the app window.
      mocks.cursor = { x: 300, y: 200 };
      mocks.events.get('companion:drag')!(sender(pet), 'end');
      expect(closeHud).toHaveBeenCalled();
      expect(store.store).toMatchObject({ agentId: 'old-pet', enabled: false });
      expect(pet.visible).toBe(false);
      // Released over the app itself: nothing opens.
      openHud.mockClear();
      carry('start', 'scout');
      carry('drop');
      expect(openHud).not.toHaveBeenCalled();
      expect(main.visible).toBe(true);
    } finally { vi.useRealTimers(); }
  });

  it('ignores carry messages from any window but the app', async () => {
    const { openHud } = await setup();
    try {
      mocks.cursor = { x: 900, y: 300 };
      mocks.events.get('companion:carry')!({ sender: {}, senderFrame: {} }, { phase: 'start', agentId: 'x' });
      mocks.events.get('companion:carry')!({ sender: {}, senderFrame: {} }, { phase: 'drop' });
      expect(openHud).not.toHaveBeenCalled();
    } finally { vi.useRealTimers(); }
  });
});

describe('desktop pet offers', () => {
  const terminal = { app: 'Terminal', appId: '', pid: 77, id: 5, title: '', bounds: { x: 200, y: 100, width: 800, height: 600 } };
  async function setup() {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0.5); // look and roam ticks at 12 s
    const main = new (await import('electron')).BrowserWindow({}) as any;
    const openHud = vi.fn(); const acceptOffer = vi.fn(); const requestAccessibility = vi.fn();
    // The driver is a separate process: this stands in for its answers.
    const driver = vi.fn(async (method: string) => method === 'context'
      ? { pid: 77, app: 'Terminal', bundle_id: 'com.apple.Terminal', title: 'zsh — build' }
      : { elements: [{ role: 'TextArea', value: '> vite build\nnpm ERR! code 1' }] });
    const look = { windows: vi.fn(async () => [terminal]), driver, computerUseActive: vi.fn(() => false) };
    const manager = installDesktopCompanion({ origin: () => 'http://localhost:8013', main: () => main, preload: '/preload.js', openHud, acceptOffer, requestAccessibility, look });
    manager.start();
    const pet = mocks.windows[1];
    pet.webContents.emit('did-finish-load');
    await vi.waitFor(() => expect(pet.visible).toBe(true));
    mocks.events.get('companion:motion-pref')!(sender(pet), false);
    await mocks.handlers.get('companion:update')!(sender(pet), { roam: false });
    const bubble = () => mocks.windows.find(w => w.options.focusable === false);
    const bubbleUrl = () => decodeURIComponent(bubble()?.loadURL.mock.calls.at(-1)?.[0] ?? '');
    const click = (url: string) => bubble()!.webContents.emit('will-navigate', { preventDefault() {} }, url);
    const offerState = () => main.webContents.send.mock.calls.filter((c: any[]) => c[0] === 'companion:offer-state').at(-1)?.[1];
    const invoke = (name: string, from: any, value?: unknown) => mocks.handlers.get(name)!(sender(from), value);
    return { main, pet, manager, openHud, acceptOffer, requestAccessibility, look, driver, bubble, bubbleUrl, click, offerState, invoke, store: mocks.stores[0] };
  }
  const activities = (pet: any) => pet.webContents.send.mock.calls.filter((c: any[]) => c[0] === 'companion:activity').map((c: any[]) => c[1]);

  it.skipIf(process.platform === 'linux')('reads a failing build, walks to its title bar, offers, and Fix it opens Quick Work with the text', async () => {
    const t = await setup();
    try {
      await t.invoke('companion:offer-settings-set', t.main, { readOtherApps: true });
      expect(t.requestAccessibility).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(12_100);
      expect(activities(t.pet).some((a: any) => a.reading === true)).toBe(true);
      await vi.advanceTimersByTimeAsync(5000);
      expect(activities(t.pet).at(-1)).toMatchObject({ reading: false, moving: null });
      // Sat on the title bar: right part of the window's top edge (pet side 96).
      expect(t.pet.getBounds()).toMatchObject({ x: 200 + 800 - 96 - 24, y: 100 - 96 + 14 });
      const url = t.bubbleUrl();
      expect(url).toContain('That build failed. Fix it?');
      expect(url).toContain('>Mute Terminal</a>');
      expect(url).toContain("default-src 'none'");
      expect(t.bubble()!.ignoreMouse).toBe(false);
      // Renderers hear about it, never the screen text.
      expect(t.offerState()).toMatchObject({ kind: 'build_failed', app: 'Terminal', appId: 'com.apple.Terminal', copy: 'That build failed. Fix it?', source: 'look' });
      expect(JSON.stringify(t.offerState())).not.toContain('npm ERR');
      const id = /allternit-offer:\/\/accept\/([\w-]+)/.exec(url)![1];
      t.click(`allternit-offer://accept/${id}`);
      expect(t.openHud).toHaveBeenCalledWith(null, expect.objectContaining({ x: 880, y: 18 }));
      expect(t.acceptOffer).toHaveBeenCalledWith(expect.objectContaining({
        app: 'Terminal', title: 'zsh — build', text: expect.stringContaining('npm ERR! code 1'), intent: 'That build failed. Fix it?', computer: true, kind: 'build_failed',
      }), expect.anything());
      expect(t.bubble()!.visible).toBe(false);
      expect(t.bubble()!.ignoreMouse).toBe(true);
      expect(t.offerState()).toBeNull();
    } finally { vi.useRealTimers(); vi.restoreAllMocks(); }
  });

  it.skipIf(process.platform === 'linux')('does not read while a computer-use run is active, or while reading is off', async () => {
    const t = await setup();
    try {
      await vi.advanceTimersByTimeAsync(40_000);
      expect(t.driver).not.toHaveBeenCalled();
      await t.invoke('companion:offer-settings-set', t.main, { readOtherApps: true });
      t.look.computerUseActive.mockReturnValue(true);
      await vi.advanceTimersByTimeAsync(40_000);
      expect(t.driver).not.toHaveBeenCalled();
    } finally { vi.useRealTimers(); vi.restoreAllMocks(); }
  });

  it('a terminal agent failing offers once, fades on time, and Mute silences that agent', async () => {
    const t = await setup();
    try {
      t.manager.reportActivity('hook:claude', 'failed', 4000);
      expect(t.bubbleUrl()).toContain('That run failed. Fix it?');
      expect(t.bubbleUrl()).toContain('Mute Claude Code');
      await vi.advanceTimersByTimeAsync(12_100);
      expect(t.bubble()!.visible).toBe(false);
      expect(t.offerState()).toBeNull();
      // Inside the 5-minute cooldown: nothing new.
      const shown = t.bubble()!.loadURL.mock.calls.length;
      t.manager.reportActivity('hook:claude', 'failed', 4000);
      expect(t.bubble()!.loadURL.mock.calls.length).toBe(shown);
      await vi.advanceTimersByTimeAsync(5 * 60_000);
      t.manager.reportActivity('hook:claude', 'failed', 4000);
      const id = /allternit-offer:\/\/mute\/([\w-]+)/.exec(t.bubbleUrl())![1];
      t.click(`allternit-offer://mute/${id}`);
      expect(t.bubbleUrl()).toContain('Muted for Claude Code');
      expect(await t.invoke('companion:offer-settings-get', t.pet)).toMatchObject({ muted: ['Claude Code'] });
      await vi.advanceTimersByTimeAsync(10 * 60_000);
      const before = t.bubble()!.loadURL.mock.calls.length;
      t.manager.reportActivity('hook:claude', 'failed', 4000);
      expect(t.bubble()!.loadURL.mock.calls.length).toBe(before);
    } finally { vi.useRealTimers(); vi.restoreAllMocks(); }
  });

  it.skipIf(process.platform === 'linux')('asks once to turn reading on, and only a click turns it on and asks for Accessibility', async () => {
    const t = await setup();
    try {
      await vi.advanceTimersByTimeAsync(125_000);
      expect(t.bubbleUrl()).toContain('Turn on reading?');
      expect(t.requestAccessibility).not.toHaveBeenCalled();
      t.click('allternit-offer://reading/on');
      expect(t.requestAccessibility).toHaveBeenCalledTimes(1);
      expect(await t.invoke('companion:offer-settings-get', t.pet)).toMatchObject({ readOtherApps: true });
      expect(t.store.store.offersAsked).toBe(true);
      await t.invoke('companion:offer-settings-set', t.main, { readOtherApps: false });
      await vi.advanceTimersByTimeAsync(10 * 60_000);
      expect(t.bubbleUrl()).not.toContain('Turn on reading?');
    } finally { vi.useRealTimers(); vi.restoreAllMocks(); }
  });

  it('takes an in-app failure only from the app window, and only while you are away from it', async () => {
    const t = await setup();
    try {
      const failure = { kind: 'run_failed', text: 'step 3\nTypeError: x is undefined', botId: 'bot-7' };
      expect(await t.invoke('companion:offer', t.pet, failure)).toBe(false);
      t.main.visible = true; t.main.focused = true;
      expect(await t.invoke('companion:offer', t.main, failure)).toBe(false);
      t.main.focused = false;
      expect(await t.invoke('companion:offer', t.main, { kind: 'not-a-kind' })).toBe(false);
      expect(await t.invoke('companion:offer', t.main, failure)).toBe(true);
      expect(t.offerState()).toMatchObject({ kind: 'run_failed', app: 'Allternit', botId: 'bot-7', source: 'app' });
      expect(await t.invoke('companion:offer-accept', t.main, t.offerState().id)).toBe(true);
      expect(t.openHud).toHaveBeenCalledWith('bot-7', expect.anything());
      expect(t.acceptOffer).toHaveBeenCalledWith(expect.objectContaining({ text: 'step 3\nTypeError: x is undefined', computer: false }), expect.anything());
    } finally { vi.useRealTimers(); vi.restoreAllMocks(); }
  });

  it('keeps settings in bounds', async () => {
    const t = await setup();
    try {
      const next = await t.invoke('companion:offer-settings-set', t.main, { cooldownMs: 5, bubbleMs: 'x', blockList: ['Slack', ' slack ', 'Slack', 7], tailChars: 99_999 });
      expect(next).toMatchObject({ cooldownMs: 30_000, bubbleMs: 12_000, blockList: ['Slack'], tailChars: 8000, enabled: true });
    } finally { vi.useRealTimers(); vi.restoreAllMocks(); }
  });
});
