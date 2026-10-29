import { app, BrowserWindow, ipcMain, Menu, screen, type IpcMainEvent, type IpcMainInvokeEvent } from 'electron';
import { NO_SYSTEM, bubbleText, dotTab, dotTone, sanitizeSummary, type SystemSummary, type SystemTab } from './system-summary.js';
import Store from 'electron-store';
import { watch } from 'node:fs';
import path from 'node:path';
import { PetActivityHub, isPetState, pickRoamTarget, roamStep, type PetState } from './pet-activity-hub.js';

export interface CompanionState {
  enabled: boolean;
  size: number;
  agentId: string | null;
  panel: 'chat' | 'settings';
  position?: { x: number; y: number };
  /** Walk along its row while nothing is happening (Petdex-style roaming). */
  roam: boolean;
}

/** Sent to the pet window on every change: what to play, the unread badge, and which way it is walking. */
export interface CompanionActivity {
  state: PetState;
  unread: boolean;
  moving: 'left' | 'right' | null;
}

export function installDesktopCompanion(options: { origin: () => string; main: () => BrowserWindow | null; preload: string; showMain?: () => void; openHud?: (agentId: string | null, anchor?: Electron.Rectangle) => void; closeHud?: () => void; moveHudWithPet?: (dx: number, dy: number) => boolean; onPetLanded?: (petBounds: Electron.Rectangle) => void; onSystem?: (summary: SystemSummary) => void; openSystem?: (tab: SystemTab) => void }) {
  const settings = new Store<CompanionState>({ name: 'desktop-companion', defaults: { enabled: true, size: 80, agentId: null, panel: 'chat', roam: true } });
  let pet: BrowserWindow | null = null;
  let chat: BrowserWindow | null = null;
  const ready = new WeakSet<BrowserWindow>();
  let drag: { x: number; y: number; lastX: number; lastY: number; left: number; top: number } | null = null;
  let quitting = false;
  // A bot carried out of the app: the pet follows the cursor until the drop.
  let carry: { timer: ReturnType<typeof setInterval>; expires: ReturnType<typeof setTimeout> } | null = null;
  // What the pet was before the first carry out; flicking the bot home puts it back.
  let away: { agentId: string | null; enabled: boolean; position?: { x: number; y: number } } | null = null;
  let attachedHudBounds: Electron.Rectangle | null = null;
  const state = () => settings.store;
  const owned = (event: IpcMainEvent | IpcMainInvokeEvent) => [pet, chat, options.main()].some(win =>
    win && !win.isDestroyed() && win.webContents === event.sender && event.senderFrame === event.sender.mainFrame,
  );
  const clamp = (n: number, low: number, high: number) => Math.max(low, Math.min(n, high));
  const broadcast = () => {
    for (const win of [pet, chat, options.main()]) if (win && !win.isDestroyed()) win.webContents.send('companion:state', state());
  };

  // ─── Activity: what the pet plays, the unread badge, roaming ──────────────
  const hub = new PetActivityHub();
  let activity: CompanionActivity = { state: 'idle', unread: false, moving: null };
  // Unknown until the pet page says it can play walk rows (sends its motion preference); older pages never roam.
  let reducedMotion: boolean | null = null;
  let expiryTimer: ReturnType<typeof setTimeout> | undefined;
  const mainAway = () => {
    const main = options.main();
    return !main || main.isDestroyed() || !main.isVisible() || !main.isFocused();
  };
  function sendActivity(patch: Partial<CompanionActivity>) {
    const next = { ...activity, ...patch };
    if (next.state === activity.state && next.unread === activity.unread && next.moving === activity.moving) return;
    activity = next;
    if (pet && !pet.isDestroyed()) pet.webContents.send('companion:activity', activity);
  }
  let appState: PetState = 'idle';
  function refreshActivity() {
    clearTimeout(expiryTimer);
    const next = hub.current();
    // A reply that finished in the app while you were elsewhere leaves a badge on the pet.
    // Only the app's own turns count: a terminal agent's reply isn't waiting in the app.
    const app = hub.get('app');
    const finished = app === 'waving' && appState !== 'waving' && mainAway();
    if (app !== appState) {
      if (app === 'waiting' && mainAway()) showBubble('Needs your answer', 6000);
      else if (finished) showBubble('Reply ready', 5000);
    }
    appState = app;
    const unread = activity.unread || finished;
    sendActivity({ state: next, unread });
    const expiry = hub.nextExpiry();
    if (expiry !== undefined) expiryTimer = setTimeout(refreshActivity, Math.max(0, expiry - Date.now()) + 5);
    if (next !== 'idle') stopRoam(true); else scheduleRoam();
  }
  /** Report a source's pet state (the app window now; terminal agents through the hook port later). */
  function reportActivity(source: string, value: unknown, ttlMs?: number) {
    if (!isPetState(value)) return false;
    hub.set(source, value, ttlMs);
    refreshActivity();
    return true;
  }
  const clearUnread = () => sendActivity({ unread: false });

  // ─── Speech bubbles: a small click-through window above the pet ───────────
  let bubble: BrowserWindow | null = null;
  let bubbleTimer: ReturnType<typeof setTimeout> | undefined;
  const BUBBLE_WIDTH = 240;
  const BUBBLE_HEIGHT = 64;
  function placeBubble() {
    if (!bubble || bubble.isDestroyed() || !pet || pet.isDestroyed()) return;
    const p = pet.getBounds();
    const area = screen.getDisplayMatching(p).workArea;
    bubble.setBounds({ width: BUBBLE_WIDTH, height: BUBBLE_HEIGHT,
      x: Math.round(clamp(p.x + p.width / 2 - BUBBLE_WIDTH / 2, area.x, area.x + area.width - BUBBLE_WIDTH)),
      y: Math.round(clamp(p.y - BUBBLE_HEIGHT + 6, area.y, area.y + area.height - BUBBLE_HEIGHT)),
    });
  }
  function hideBubble() {
    clearTimeout(bubbleTimer); bubbleTimer = undefined;
    if (bubble && !bubble.isDestroyed()) bubble.hide();
  }
  /** Show a short line above the pet (hook clients, "Reply ready"); never while the pet is hidden or quick chat is open. */
  function showBubble(text: string, ttlMs = 5000) {
    if (!state().enabled || !pet || pet.isDestroyed() || !pet.isVisible() || (chat && !chat.isDestroyed() && chat.isVisible())) return false;
    if (!bubble || bubble.isDestroyed()) {
      bubble = new BrowserWindow({
        width: BUBBLE_WIDTH, height: BUBBLE_HEIGHT, show: false, focusable: false,
        frame: false, transparent: true, backgroundColor: '#00000000', hasShadow: false,
        resizable: false, minimizable: false, maximizable: false, fullscreenable: false,
        skipTaskbar: true, alwaysOnTop: true, type: process.platform === 'darwin' ? 'panel' : undefined,
        webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false, javascript: false },
      });
      bubble.setAlwaysOnTop(true, 'floating');
      bubble.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true, skipTransformProcessType: true });
      bubble.setIgnoreMouseEvents(true);
      bubble.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
      bubble.webContents.on('will-navigate', event => event.preventDefault());
      bubble.on('close', event => { if (!quitting) { event.preventDefault(); bubble?.hide(); } });
    }
    const win = bubble;
    clearTimeout(bubbleTimer);
    void win.loadURL(bubbleHtml(text)).then(() => {
      if (win.isDestroyed() || !pet || pet.isDestroyed() || !pet.isVisible()) return;
      placeBubble(); win.showInactive();
    }, () => {});
    bubbleTimer = setTimeout(hideBubble, Math.max(1500, Math.min(ttlMs, 30_000)));
    return true;
  }

  let roamTimer: ReturnType<typeof setTimeout> | undefined;
  let walkTimer: ReturnType<typeof setInterval> | undefined;
  const canRoam = () => state().enabled && state().roam !== false && reducedMotion === false && activity.state === 'idle'
    && !!pet && !pet.isDestroyed() && pet.isVisible() && ready.has(pet) && !drag && !carry && !attachedHudBounds
    && !(chat && !chat.isDestroyed() && chat.isVisible());
  function stopRoam(save = false) {
    clearTimeout(roamTimer); roamTimer = undefined;
    if (walkTimer) {
      clearInterval(walkTimer); walkTimer = undefined;
      if (save && pet && !pet.isDestroyed()) { const b = pet.getBounds(); settings.set('position', { x: b.x, y: b.y }); }
    }
    sendActivity({ moving: null });
  }
  function scheduleRoam() {
    if (roamTimer || walkTimer || !canRoam()) return;
    roamTimer = setTimeout(() => { roamTimer = undefined; walk(); }, 6000 + Math.random() * 12000);
  }
  function walk() {
    if (!canRoam() || !pet) return;
    const start = pet.getBounds();
    const area = screen.getDisplayMatching(start).workArea;
    const target = pickRoamTarget(area, start.width, start.x);
    if (target === start.x) { scheduleRoam(); return; }
    let x = start.x;
    let last = Date.now();
    sendActivity({ moving: target > x ? 'right' : 'left' });
    walkTimer = setInterval(() => {
      if (!canRoam() || !pet) { stopRoam(true); return; }
      const now = Date.now();
      x = roamStep(x, target, now - last);
      last = now;
      pet.setPosition(Math.round(x), pet.getBounds().y);
      placeBubble();
      if (x === target) { stopRoam(true); scheduleRoam(); }
    }, 33);
  }
  app.on('browser-window-focus', (_event, win) => { if (win === options.main()) clearUnread(); });
  function toggleMain() {
    clearUnread();
    const main = options.main();
    if (main && !main.isDestroyed() && main.isVisible() && main.isFocused()) { main.hide(); return; }
    if (options.showMain) options.showMain();
    else if (main && !main.isDestroyed()) { main.show(); main.focus(); }
  }
  // The gizzi terminal pet wears the same bot: it rewrites this settings file
  // when the user picks a bot in its HUD. Follow those external writes so
  // both pets match (electron-store re-reads the file on each access). Any
  // event in the folder triggers a re-read: when a temp file is renamed over
  // the settings file, macOS may report only the temp file's name.
  let lastAgentId = state().agentId;
  let externalTimer: ReturnType<typeof setTimeout> | undefined;
  const settingsPath = (settings as { path?: unknown }).path;
  if (typeof settingsPath === 'string') {
    try {
      watch(path.dirname(settingsPath), () => {
        clearTimeout(externalTimer);
        externalTimer = setTimeout(() => {
          const next = state().agentId;
          if (next === lastAgentId) return;
          lastAgentId = next;
          broadcast();
        }, 150);
      }).unref();
    } catch (error) {
      console.warn('[desktop-companion] not following terminal pet changes', error);
    }
  }
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
  function attachPetToHud(bounds: Electron.Rectangle) {
    attachedHudBounds = { ...bounds };
    stopRoam();
    if (!pet || pet.isDestroyed() || !state().enabled) return;
    const nextSize = clamp(Math.round(bounds.width * 0.12), 48, 160);
    if (state().size !== nextSize) settings.set('size', nextSize);
    const side = nextSize + 16;
    const area = screen.getDisplayMatching(bounds).workArea;
    pet.setBounds({
      width: side, height: side,
      x: Math.round(clamp(bounds.x + bounds.width - side - 24, area.x, area.x + area.width - side)),
      y: Math.round(clamp(bounds.y - side + 20, area.y, area.y + area.height - side)),
    });
    pet.moveTop();
    placeBubble();
    broadcast();
  }
  function detachPetFromHud() {
    attachedHudBounds = null;
    if (pet && !pet.isDestroyed()) pet.setBounds(petBounds());
    placeBubble();
    scheduleRoam();
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
          if (kind === 'pet' && state().enabled) { win.showInactive(); win.webContents.send('companion:activity', activity); scheduleRoam(); }
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
    hideBubble();
    settings.set('panel', panel);
    if (panel === 'chat') clearUnread();
    stopRoam(true);
    ensurePet();
    if (panel === 'chat' && options.openHud) {
      chat?.hide();
      options.openHud(state().agentId);
      if (away && pet && !pet.isDestroyed()) options.onPetLanded?.(pet.getBounds());
      broadcast();
      return;
    }
    if (!chat || chat.isDestroyed()) {
      chat = makeWindow('chat');
    } else if (ready.has(chat)) { placeChat(); chat.show(); chat.focus(); }
    broadcast();
  }
  // ─── Carry: drag a bot out of the app and it becomes the pet under the cursor ─
  const insideMain = (point: Electron.Point) => {
    const main = options.main();
    if (!main || main.isDestroyed() || !main.isVisible() || main.isMinimized()) return false;
    const b = main.getBounds();
    return point.x >= b.x && point.x < b.x + b.width && point.y >= b.y && point.y < b.y + b.height;
  };
  function followCursor() {
    if (!pet || pet.isDestroyed()) return;
    const cursor = screen.getCursorScreenPoint();
    const side = state().size + 16;
    pet.setBounds({ width: side, height: side, x: Math.round(cursor.x - side / 2), y: Math.round(cursor.y - side / 2) });
  }
  function carryStart(agentId: string) {
    if (carry) return;
    away ??= { agentId: state().agentId, enabled: state().enabled, position: state().position };
    options.closeHud?.(); chat?.hide();
    hideBubble(); stopRoam();
    if (state().agentId !== agentId) { settings.set('agentId', agentId); lastAgentId = agentId; }
    settings.set('enabled', true);
    const win = ensurePet();
    // The pet sits under the cursor; the release must reach the app window, not the pet.
    win.setIgnoreMouseEvents(true);
    followCursor();
    if (ready.has(win)) win.showInactive();
    broadcast();
    // The app window ends every carry; this only guards against a renderer that never does.
    carry = { timer: setInterval(followCursor, 16), expires: setTimeout(() => carryEnd('cancel'), 120_000) };
  }
  function carryEnd(phase: 'drop' | 'back' | 'cancel') {
    if (!carry) return;
    clearInterval(carry.timer); clearTimeout(carry.expires); carry = null;
    if (pet && !pet.isDestroyed()) pet.setIgnoreMouseEvents(false);
    if (phase !== 'drop' || !pet || pet.isDestroyed() || insideMain(screen.getCursorScreenPoint())) { goHome(); return; }
    const bounds = pet.getBounds();
    settings.set('position', { x: bounds.x, y: bounds.y });
    settings.set('panel', 'chat');
    clearUnread();
    if (options.openHud) options.openHud(state().agentId, bounds);
    else showPanel('chat');
    options.onPetLanded?.(bounds);
    broadcast();
  }
  /** Flick it home: Quick Work closes and the pet is put back the way it was before the carry. */
  function goHome() {
    const back = away;
    away = null;
    options.closeHud?.(); chat?.hide();
    if (back) {
      settings.set('agentId', back.agentId); lastAgentId = back.agentId;
      if (back.position) settings.set('position', back.position); else settings.delete('position');
      settings.set('enabled', back.enabled);
    }
    if (state().enabled) pet?.setBounds(petBounds());
    else { pet?.hide(); hideBubble(); }
    placeBubble(); broadcast(); scheduleRoam();
  }
  function update(patch: Partial<CompanionState>) {
    const geometryChanged = typeof patch.enabled === 'boolean' || typeof patch.size === 'number' && Number.isFinite(patch.size);
    if (typeof patch.enabled === 'boolean') settings.set('enabled', patch.enabled);
    if (typeof patch.size === 'number' && Number.isFinite(patch.size)) settings.set('size', clamp(Math.round(patch.size), 48, 160));
    if (patch.agentId === null || typeof patch.agentId === 'string' && patch.agentId.length <= 200) { settings.set('agentId', patch.agentId); lastAgentId = patch.agentId; }
    if (typeof patch.roam === 'boolean') settings.set('roam', patch.roam);
    if (state().enabled) { ensurePet(); if (geometryChanged) { if (attachedHudBounds) attachPetToHud(attachedHudBounds); else pet?.setBounds(petBounds()); } if (pet && ready.has(pet)) pet.showInactive(); }
    else { pet?.hide(); hideBubble(); if (state().panel !== 'settings') chat?.hide(); }
    placeChat(); placeBubble(); broadcast();
    if (canRoam()) scheduleRoam(); else stopRoam(true);
    return state();
  }
  function reset() { settings.delete('position'); if (attachedHudBounds) attachPetToHud(attachedHudBounds); else pet?.setBounds(petBounds(true)); placeChat(); placeBubble(); broadcast(); return state(); }
  function handle(name: string, fn: (event: IpcMainInvokeEvent, value: any) => unknown) {
    ipcMain.handle(name, (event, value) => { if (!owned(event)) throw new Error('Untrusted companion sender'); return fn(event, value); });
  }
  handle('companion:get', () => state());
  handle('companion:update', (_event, patch) => update(patch && typeof patch === 'object' ? patch : {}));
  handle('companion:reset', () => reset());
  handle('companion:open', (_event, panel) => showPanel(panel === 'settings' ? 'settings' : 'chat'));
  handle('companion:close', () => { chat?.hide(); options.closeHud?.(); scheduleRoam(); });
  handle('companion:activity-get', () => activity);
  handle('companion:toggle-main', () => toggleMain());

  // ─── System drawer (Settings → Developer): the dot on the pet ─────────────
  // The app window reports the summary; the pet draws a dot from it. Hovering
  // the dot shows the summary in the bubble, clicking the dot opens the drawer.
  // Clicking the pet itself still opens quick chat.
  let system: SystemSummary = NO_SYSTEM;
  ipcMain.on('companion:system-report', (event, value: unknown) => {
    const main = options.main();
    if (!main || main.isDestroyed() || event.sender !== main.webContents || !owned(event)) return;
    system = sanitizeSummary(value);
    if (pet && !pet.isDestroyed()) pet.webContents.send('companion:system', system);
    if (!dotTone(system)) hideBubble();
    options.onSystem?.(system);
  });
  handle('companion:system-get', () => system);
  ipcMain.on('companion:system-hover', (event, on: unknown) => {
    if (!pet || event.sender !== pet.webContents || !owned(event)) return;
    if (on === true && dotTone(system)) showBubble(bubbleText(system), 30_000);
    else hideBubble();
  });
  handle('companion:system-open', (event) => {
    if (!pet || event.sender !== pet.webContents || !dotTone(system)) return false;
    hideBubble();
    options.openSystem?.(dotTab(system));
    return true;
  });
  // Only the app window reports activity: it owns the pet-activity signal.
  ipcMain.on('companion:activity', (event, value: unknown) => {
    const main = options.main();
    if (!main || main.isDestroyed() || event.sender !== main.webContents || !owned(event)) return;
    reportActivity('app', value);
  });
  ipcMain.on('companion:motion-pref', (event, reduced: unknown) => {
    if (!pet || event.sender !== pet.webContents || !owned(event)) return;
    reducedMotion = reduced === true;
    if (reducedMotion) stopRoam(true); else scheduleRoam();
  });
  handle('companion:menu', () => {
    Menu.buildFromTemplate([
      { label: 'Quick chat', click: () => showPanel() },
      { label: 'Pet, size and visibility…', click: () => showPanel('settings') },
      { label: 'Reset position', click: () => reset() },
      { label: 'Walk around when idle', type: 'checkbox', checked: state().roam !== false, click: () => update({ roam: state().roam === false }) },
      { type: 'separator' },
      { label: 'Hide desktop pet', click: () => update({ enabled: false }) },
    ]).popup({ window: pet ?? undefined });
  });
  ipcMain.on('companion:drag', (event, phase: string) => {
    if (!pet || event.sender !== pet.webContents || !owned(event)) return;
    const cursor = screen.getCursorScreenPoint();
    if (phase === 'start') { stopRoam(); const b = pet.getBounds(); drag = { x: cursor.x, y: cursor.y, lastX: cursor.x, lastY: cursor.y, left: b.x, top: b.y }; }
    if (phase === 'move' && drag) {
      const movedHud = options.moveHudWithPet?.(cursor.x - drag.lastX, cursor.y - drag.lastY);
      drag.lastX = cursor.x; drag.lastY = cursor.y;
      if (!movedHud) { pet.setPosition(drag.left + cursor.x - drag.x, drag.top + cursor.y - drag.y); placeChat(); }
      placeBubble();
    }
    if (phase === 'end') {
      drag = null;
      // A bot that was carried out goes home when it is dropped back on the app.
      if (away && insideMain(cursor)) { goHome(); return; }
      const b = pet.getBounds();
      settings.set('position', { x: b.x, y: b.y });
      const hudOpen = options.moveHudWithPet?.(0, 0);
      if (!hudOpen) pet.setBounds(petBounds());
      placeChat(); placeBubble();
      // Moved onto another window: Quick Work's context follows the pet.
      if (hudOpen) options.onPetLanded?.(pet.getBounds());
      scheduleRoam();
    }
  });
  // Only the app window starts and ends a carry (its bot rail owns the drag).
  ipcMain.on('companion:carry', (event, value: unknown) => {
    const main = options.main();
    if (!main || main.isDestroyed() || event.sender !== main.webContents || !owned(event)) return;
    const { phase, agentId } = (value && typeof value === 'object' ? value : {}) as { phase?: unknown; agentId?: unknown };
    if (phase === 'start' && typeof agentId === 'string' && agentId.length > 0 && agentId.length <= 200) carryStart(agentId);
    else if (phase === 'drop' || phase === 'back' || phase === 'cancel') carryEnd(phase);
  });
  ipcMain.on('companion:height', (event, height: number) => {
    if (chat && event.sender === chat.webContents && owned(event) && Number.isFinite(height)) placeChat(height);
  });
  ipcMain.on('companion:ignore-mouse', (event, ignore: boolean) => {
    if (pet && event.sender === pet.webContents && owned(event)) pet.setIgnoreMouseEvents(ignore === true, { forward: true });
  });
  const repair = () => { pet?.setBounds(petBounds()); placeChat(); placeBubble(); };
  app.whenReady().then(() => {
    screen.on('display-removed', repair); screen.on('display-metrics-changed', repair);
  });
  app.on('before-quit', () => { quitting = true; if (carry) { clearInterval(carry.timer); clearTimeout(carry.expires); carry = null; } stopRoam(true); clearTimeout(expiryTimer); hideBubble(); });
  return { reportActivity, showBubble, system: () => system, activity: () => activity, start: () => { if (state().enabled) ensurePet(); }, show: () => update({ enabled: true }), settings: () => showPanel('settings'), reset, attachPetToHud, detachPetFromHud };
}

/** The bubble page: escaped text only, no scripts, follows the system theme. */
export function bubbleHtml(text: string): string {
  const safe = text.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]!));
  const html = `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'"><style>
:root{--bg:#fff;--fg:#1c1b1a;--edge:rgba(0,0,0,.12);color-scheme:light}
@media (prefers-color-scheme:dark){:root{--bg:#26262a;--fg:#f3f3f3;--edge:rgba(255,255,255,.16);color-scheme:dark}}
html,body{margin:0;height:100%;background:transparent;overflow:hidden}
body{display:flex;align-items:flex-end;justify-content:center;padding:0 4px 10px;box-sizing:border-box;font:500 12px/1.35 -apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif}
p{position:relative;margin:0;max-width:100%;padding:6px 10px;border-radius:12px;background:var(--bg);color:var(--fg);border:1px solid var(--edge);box-shadow:0 2px 8px rgba(0,0,0,.18);overflow:hidden;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;word-break:break-word}
</style></head><body><p role="status">${safe}</p></body></html>`;
  return `data:text/html;charset=utf-8,${encodeURIComponent(html)}`;
}
