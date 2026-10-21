import { beforeEach, describe, expect, it, vi } from 'vitest';
import { EventEmitter } from 'node:events';

const mocks = vi.hoisted(() => ({ appEvents: new Map<string, Function>(), screenEvents: new Map<string, Function>(), handlers: new Map<string, Function>(), events: new Map<string, Function>(), windows: [] as any[], primary: { bounds: { x: 0, y: 0, width: 1512, height: 982 }, workArea: { x: 0, y: 38, width: 1512, height: 944 }, scaleFactor: 2 } }));
vi.mock('electron', () => ({
  app: { whenReady: () => Promise.resolve(), on: (name: string, fn: Function) => mocks.appEvents.set(name, fn) },
  screen: { getPrimaryDisplay: () => mocks.primary, on: (name: string, fn: Function) => mocks.screenEvents.set(name, fn) },
  ipcMain: { handle: (name: string, fn: Function) => mocks.handlers.set(name, fn), on: (name: string, fn: Function) => mocks.events.set(name, fn) },
  BrowserWindow: class extends EventEmitter {
    bounds: any; visible = false; options: any; ignoreMouse = false; protectedContent = false; level = '';
    webContents = Object.assign(new EventEmitter(), { mainFrame: {}, send: vi.fn(), setWindowOpenHandler: vi.fn(), executeJavaScript: vi.fn(async () => true) });
    constructor(options: any) { super(); this.options = options; this.bounds = { x: options.x, y: options.y, width: options.width, height: options.height }; mocks.windows.push(this); }
    isDestroyed() { return false; }
    isVisible() { return this.visible; }
    setAlwaysOnTop(_on: boolean, level: string) { this.level = level; } setVisibleOnAllWorkspaces() {}
    setIgnoreMouseEvents(ignore: boolean) { this.ignoreMouse = ignore; }
    setContentProtection(on: boolean) { this.protectedContent = on; }
    loadURL = vi.fn(async () => {});
    getBounds() { return this.bounds; }
    setBounds(bounds: any) { this.bounds = bounds; }
    show = vi.fn(() => { this.visible = true; }); showInactive() { this.visible = true; } hide() { this.visible = false; } focus = vi.fn();
  },
}));
import { installAgentCursorWindow } from './agent-cursor-window.js';

function sender(win: any) { return { sender: win.webContents, senderFrame: win.webContents.mainFrame }; }
async function started() {
  const manager = installAgentCursorWindow({ origin: () => 'http://localhost:8013', preload: '/preload.js' });
  manager.start();
  const win = mocks.windows[0];
  win.webContents.emit('did-finish-load');
  await new Promise(resolve => setTimeout(resolve, 0));
  return { manager, win };
}
beforeEach(() => { mocks.appEvents.clear(); mocks.screenEvents.clear(); mocks.handlers.clear(); mocks.events.clear(); mocks.windows.length = 0; mocks.primary.bounds = { x: 0, y: 0, width: 1512, height: 982 }; });

describe('agent cursor window', () => {
  it('covers the whole primary display, hidden, click-through and out of screenshots', async () => {
    const { win } = await started();
    expect(win.loadURL).toHaveBeenCalledWith('http://localhost:8013/companion.html?surface=agent-cursor');
    expect(win.bounds).toEqual({ x: 0, y: 0, width: 1512, height: 982 });
    expect(win.options).toMatchObject({ show: false, focusable: false, transparent: true, frame: false, skipTaskbar: true, hasShadow: false });
    expect(win.visible).toBe(false);
    expect(win.ignoreMouse).toBe(true);
    expect(win.protectedContent).toBe(true);
    expect(win.level).toBe('screen-saver');
  });

  it('shows without focus and hides only for its own page', async () => {
    const { win } = await started();
    const stranger = { sender: { mainFrame: {} }, senderFrame: {} };
    mocks.events.get('agent-cursor:show')!(stranger);
    expect(win.visible).toBe(false);
    mocks.events.get('agent-cursor:show')!(sender(win));
    expect(win.visible).toBe(true);
    expect(win.focus).not.toHaveBeenCalled();
    expect(win.show).not.toHaveBeenCalled();
    mocks.events.get('agent-cursor:hide')!(stranger);
    expect(win.visible).toBe(true);
    mocks.events.get('agent-cursor:hide')!(sender(win));
    expect(win.visible).toBe(false);
    // A subframe of the page isn't trusted either.
    expect(() => mocks.handlers.get('agent-cursor:display')!({ sender: win.webContents, senderFrame: {} })).toThrow();
  });

  it('reports the display and follows display changes', async () => {
    const { win } = await started();
    expect(mocks.handlers.get('agent-cursor:display')!(sender(win))).toEqual({ width: 1512, height: 982, scaleFactor: 2 });
    mocks.primary.bounds = { x: 0, y: 0, width: 2560, height: 1440 };
    mocks.screenEvents.get('display-metrics-changed')!();
    expect(win.bounds).toEqual({ x: 0, y: 0, width: 2560, height: 1440 });
    expect(win.webContents.send).toHaveBeenCalledWith('agent-cursor:display-changed', { width: 2560, height: 1440, scaleFactor: 2 });
  });

  it("never blocks quitting and won't show after it starts", async () => {
    const { win } = await started();
    const closing = { preventDefault: vi.fn() };
    win.emit('close', closing);
    expect(closing.preventDefault).toHaveBeenCalled();
    mocks.appEvents.get('before-quit')!();
    const quittingClose = { preventDefault: vi.fn() };
    win.emit('close', quittingClose);
    expect(quittingClose.preventDefault).not.toHaveBeenCalled();
    mocks.events.get('agent-cursor:show')!(sender(win));
    expect(win.visible).toBe(false);
  });
});
