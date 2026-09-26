import { beforeEach, describe, expect, it, vi } from 'vitest';
import { EventEmitter } from 'node:events';

const mocks = vi.hoisted(() => ({ handlers: new Map<string, Function>(), events: new Map<string, Function>(), windows: [] as any[], menu: [] as any[] }));
vi.mock('electron-store', () => ({ default: class {
  store: any;
  constructor(options: any) { this.store = { ...options.defaults }; }
  set(key: string, value: unknown) { this.store[key] = value; }
  delete(key: string) { delete this.store[key]; }
} }));
vi.mock('electron', () => ({
  app: { whenReady: () => Promise.resolve(), on: vi.fn() },
  screen: { getPrimaryDisplay: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getDisplayNearestPoint: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getDisplayMatching: () => ({ workArea: { x: 0, y: 0, width: 1280, height: 800 } }), getCursorScreenPoint: () => ({ x: 100, y: 100 }), on: vi.fn() },
  ipcMain: { handle: (name: string, fn: Function) => mocks.handlers.set(name, fn), on: (name: string, fn: Function) => mocks.events.set(name, fn) },
  Menu: { buildFromTemplate: (items: any[]) => { mocks.menu = items; return { popup: vi.fn() }; } },
  BrowserWindow: class extends EventEmitter {
    bounds: any; visible = false; options: any;
    webContents = Object.assign(new EventEmitter(), { mainFrame: {}, send: vi.fn(), setWindowOpenHandler: vi.fn(), executeJavaScript: vi.fn(async () => true) });
    constructor(options: any) { super(); this.options = options; this.bounds = options; mocks.windows.push(this); }
    isDestroyed() { return false; }
    setAlwaysOnTop() {} setVisibleOnAllWorkspaces() {} setIgnoreMouseEvents() {}
    loadURL = vi.fn(async () => {});
    getBounds() { return this.bounds; }
    setBounds(bounds: any) { this.bounds = bounds; }
    setPosition(x: number, y: number) { this.bounds = { ...this.bounds, x, y }; }
    show() { this.visible = true; } showInactive() { this.visible = true; } hide() { this.visible = false; } focus() {}
  },
}));
import { installDesktopCompanion } from './desktop-companion.js';

function sender(win: any) { return { sender: win.webContents, senderFrame: win.webContents.mainFrame }; }
beforeEach(() => { mocks.windows.length = 0; mocks.handlers.clear(); mocks.events.clear(); });
describe('desktop companion windows', () => {
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
