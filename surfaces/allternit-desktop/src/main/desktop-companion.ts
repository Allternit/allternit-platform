import { app, BrowserWindow, ipcMain, Menu, screen, type IpcMainEvent, type IpcMainInvokeEvent } from 'electron';
import Store from 'electron-store';

export interface CompanionState {
  enabled: boolean;
  size: number;
  agentId: string | null;
  panel: 'chat' | 'settings';
  position?: { x: number; y: number };
}

export function installDesktopCompanion(options: { origin: () => string; main: () => BrowserWindow | null; preload: string }) {
  const settings = new Store<CompanionState>({ name: 'desktop-companion', defaults: { enabled: true, size: 80, agentId: null, panel: 'chat' } });
  let pet: BrowserWindow | null = null;
  let chat: BrowserWindow | null = null;
  const ready = new WeakSet<BrowserWindow>();
  let drag: { x: number; y: number; left: number; top: number } | null = null;
  let quitting = false;
  const state = () => settings.store;
  const owned = (event: IpcMainEvent | IpcMainInvokeEvent) => [pet, chat, options.main()].some(win =>
    win && !win.isDestroyed() && win.webContents === event.sender && event.senderFrame === event.sender.mainFrame,
  );
  const clamp = (n: number, low: number, high: number) => Math.max(low, Math.min(n, high));
  const broadcast = () => {
    for (const win of [pet, chat, options.main()]) if (win && !win.isDestroyed()) win.webContents.send('companion:state', state());
  };
  function petBounds(reset = false) {
    const size = state().size + 16;
    const saved = reset ? undefined : state().position;
    const area = saved ? screen.getDisplayNearestPoint(saved).workArea : screen.getPrimaryDisplay().workArea;
    return { width: size, height: size,
      x: Math.round(clamp(saved?.x ?? area.x + area.width - size - 24, area.x, area.x + area.width - size)),
      y: Math.round(clamp(saved?.y ?? area.y + area.height - size - 24, area.y, area.y + area.height - size)),
    };
  }
  function placeChat(height = chat?.getBounds().height ?? 128) {
    if (!chat || !pet) return;
    const p = pet.getBounds();
    const area = screen.getDisplayMatching(p).workArea;
    const width = Math.min(400, area.width - 24);
    height = clamp(Math.round(height), 112, Math.min(660, area.height - 24));
    chat.setBounds({ width, height,
      x: Math.round(clamp(p.x + p.width - width, area.x + 12, area.x + area.width - width - 12)),
      y: Math.round(clamp(p.y - height - 8, area.y + 12, area.y + area.height - height - 12)),
    });
  }
  function makeWindow(kind: 'pet' | 'chat') {
    const win = new BrowserWindow({
      ...(kind === 'pet' ? petBounds() : { width: 400, height: 128 }),
      title: kind === 'pet' ? 'Allternit Desktop Pet' : 'Allternit Quick Chat',
      frame: false, transparent: true, backgroundColor: '#00000000', hasShadow: false,
      resizable: false, minimizable: false, maximizable: false, fullscreenable: false,
      skipTaskbar: true, alwaysOnTop: true, show: false,
      type: process.platform === 'darwin' ? 'panel' : undefined,
      webPreferences: { preload: options.preload, sandbox: true, contextIsolation: true, nodeIntegration: false },
    });
    win.setAlwaysOnTop(true, 'floating');
    win.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
    const target = new URL(`/companion.html?surface=${kind}`, options.origin()).toString();
    win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
    win.webContents.on('will-navigate', (event, url) => { if (url !== target) event.preventDefault(); });
    win.webContents.on('did-start-navigation', () => { ready.delete(win); win.hide(); });
    win.webContents.on('did-finish-load', () => {
      void win.webContents.executeJavaScript('document.documentElement.dataset.desktopCompanion === "true"')
        .then((valid: boolean) => {
          if (win.isDestroyed()) return;
          if (!valid) {
            console.error(`[desktop-companion] ${target} did not render the companion entry`);
            win.hide();
            return;
          }
          ready.add(win);
          if (kind === 'pet' && state().enabled) win.showInactive();
          if (kind === 'chat') { placeChat(); win.show(); win.focus(); broadcast(); }
        })
        .catch(error => { console.error('[desktop-companion] renderer validation failed', error); if (!win.isDestroyed()) win.hide(); });
    });
    win.on('close', event => { if (!quitting) { event.preventDefault(); win.hide(); } });
    void win.loadURL(target);
    return win;
  }
  function ensurePet() {
    if (!pet || pet.isDestroyed()) pet = makeWindow('pet');
    return pet;
  }
  function showPanel(panel: 'chat' | 'settings' = 'chat') {
    settings.set('panel', panel);
    ensurePet();
    if (!chat || chat.isDestroyed()) {
      chat = makeWindow('chat');
    } else if (ready.has(chat)) { placeChat(); chat.show(); chat.focus(); }
    broadcast();
  }
  function update(patch: Partial<CompanionState>) {
    if (typeof patch.enabled === 'boolean') settings.set('enabled', patch.enabled);
    if (typeof patch.size === 'number' && Number.isFinite(patch.size)) settings.set('size', clamp(Math.round(patch.size), 48, 160));
    if (patch.agentId === null || typeof patch.agentId === 'string' && patch.agentId.length <= 200) settings.set('agentId', patch.agentId);
    if (state().enabled) { ensurePet().setBounds(petBounds()); if (pet && ready.has(pet)) pet.showInactive(); }
    else { pet?.hide(); if (state().panel !== 'settings') chat?.hide(); }
    placeChat(); broadcast();
    return state();
  }
  function reset() { settings.delete('position'); pet?.setBounds(petBounds(true)); placeChat(); broadcast(); return state(); }
  function handle(name: string, fn: (event: IpcMainInvokeEvent, value: any) => unknown) {
    ipcMain.handle(name, (event, value) => { if (!owned(event)) throw new Error('Untrusted companion sender'); return fn(event, value); });
  }
  handle('companion:get', () => state());
  handle('companion:update', (_event, patch) => update(patch && typeof patch === 'object' ? patch : {}));
  handle('companion:reset', () => reset());
  handle('companion:open', (_event, panel) => showPanel(panel === 'settings' ? 'settings' : 'chat'));
  handle('companion:close', () => chat?.hide());
  handle('companion:menu', () => {
    Menu.buildFromTemplate([
      { label: 'Quick chat', click: () => showPanel() },
      { label: 'Pet, size and visibility…', click: () => showPanel('settings') },
      { label: 'Reset position', click: () => reset() },
      { type: 'separator' },
      { label: 'Hide desktop pet', click: () => update({ enabled: false }) },
    ]).popup({ window: pet ?? undefined });
  });
  ipcMain.on('companion:drag', (event, phase: string) => {
    if (!pet || event.sender !== pet.webContents || !owned(event)) return;
    const cursor = screen.getCursorScreenPoint();
    if (phase === 'start') { const b = pet.getBounds(); drag = { x: cursor.x, y: cursor.y, left: b.x, top: b.y }; }
    if (phase === 'move' && drag) { pet.setPosition(drag.left + cursor.x - drag.x, drag.top + cursor.y - drag.y); placeChat(); }
    if (phase === 'end') { drag = null; const b = pet.getBounds(); settings.set('position', { x: b.x, y: b.y }); pet.setBounds(petBounds()); placeChat(); }
  });
  ipcMain.on('companion:height', (event, height: number) => {
    if (chat && event.sender === chat.webContents && owned(event) && Number.isFinite(height)) placeChat(height);
  });
  ipcMain.on('companion:ignore-mouse', (event, ignore: boolean) => {
    if (pet && event.sender === pet.webContents && owned(event)) pet.setIgnoreMouseEvents(ignore === true, { forward: true });
  });
  const repair = () => { pet?.setBounds(petBounds()); placeChat(); };
  app.whenReady().then(() => {
    screen.on('display-removed', repair); screen.on('display-metrics-changed', repair);
  });
  app.on('before-quit', () => { quitting = true; });
  return { start: () => { if (state().enabled) ensurePet(); }, show: () => update({ enabled: true }), settings: () => showPanel('settings'), reset };
}
