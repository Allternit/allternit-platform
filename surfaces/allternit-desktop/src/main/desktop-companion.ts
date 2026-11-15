import { app, BrowserWindow, ipcMain, Menu, screen, type IpcMainEvent, type IpcMainInvokeEvent } from 'electron';
import { NO_SYSTEM, bubbleText, dotTab, dotTone, sanitizeSummary, type SystemSummary, type SystemTab } from './system-summary.js';
import Store from 'electron-store';
import { randomUUID } from 'node:crypto';
import { watch } from 'node:fs';
import path from 'node:path';
import { PetActivityHub, isPetState, pickRoamTarget, roamStep, type PetState } from './pet-activity-hub.js';
import { LookMemo, lookAt, pickLookTarget, type LookWindow } from './pet-look.js';
import { OfferGate, listNamesApp, matchOffer, normalizeOfferText, offerCopy, offerMuteLabel, type OfferSource } from './pet-notice.js';
import { PET_OFFER_COPY, PET_OFFER_KINDS, PET_OFFER_LIMITS, petOfferSettingsDefaults, type PetOffer, type PetOfferKind, type PetOfferSettings } from './pet-offers.gen.js';

export interface CompanionState {
  enabled: boolean;
  size: number;
  agentId: string | null;
  panel: 'chat' | 'settings';
  position?: { x: number; y: number };
  /** Walk along its row while nothing is happening (Petdex-style roaming). */
  roam: boolean;
  /** Pet offers (pet-offers-v1 settings): device-local, contract defaults fill any gap. */
  offers?: Partial<PetOfferSettings>;
  /** The one-time "Turn on reading?" bubble has been shown. */
  offersAsked?: boolean;
}

/** Sent to the pet window on every change: what to play, the unread badge, and which way it is walking. */
export interface CompanionActivity {
  state: PetState;
  unread: boolean;
  moving: 'left' | 'right' | null;
  /** True for a moment while the pet reads another app's window (the eye). */
  reading: boolean;
}

/** How the pet reads other apps (pet-look.ts). Absent = the pet never reads windows. */
export interface CompanionLookOptions {
  /** Other apps' windows, front to back (never Allternit's own); null when the OS can't list them. */
  windows: () => Promise<LookWindow[] | null>;
  /** One Allternit Driver call; rejects when the driver is not running. */
  driver: (method: string, params: Record<string, unknown>) => Promise<unknown>;
  /** True while a computer-use run is driving this computer. */
  computerUseActive?: () => boolean;
}

/** What Fix it hands Quick Work: the attachment text and the draft. */
export interface OfferCarry {
  app: string;
  title: string;
  text: string;
  /** The bubble line, used as the draft. Never sent on its own. */
  intent: string;
  /** Read from another app: the turn may use the computer tools. */
  computer: boolean;
  kind: PetOfferKind;
}

/** What renderers hear about the offer showing. Never the screen text. */
export interface CompanionOfferNotice {
  id: string;
  kind: PetOfferKind;
  app: string;
  appId: string;
  copy: string;
  muteLabel: string;
  botId: string;
  createdAt: number;
  source: OfferSource;
}

/** Bounds-checked settings: a patch over `current`, unknown keys and bad values ignored. */
export function mergeOfferSettings(current: PetOfferSettings, patch: unknown): PetOfferSettings {
  const p = (patch && typeof patch === 'object' ? patch : {}) as Record<string, unknown>;
  const num = (v: unknown, low: number, high: number, fallback: number) =>
    typeof v === 'number' && Number.isFinite(v) ? Math.max(low, Math.min(Math.round(v), high)) : fallback;
  // Trimmed, capped, and one entry per app however it is capitalized (matching ignores case).
  const list = (v: unknown, fallback: string[]) => {
    if (!Array.isArray(v)) return fallback;
    const seen = new Map<string, string>();
    for (const x of v) {
      const entry = typeof x === 'string' ? x.trim().slice(0, 200) : '';
      if (entry && !seen.has(entry.toLowerCase())) seen.set(entry.toLowerCase(), entry);
    }
    return [...seen.values()].slice(0, 200);
  };
  const maxTextChars = num(p.maxTextChars, 500, 8000, current.maxTextChars);
  return {
    enabled: typeof p.enabled === 'boolean' ? p.enabled : current.enabled,
    readOtherApps: typeof p.readOtherApps === 'boolean' ? p.readOtherApps : current.readOtherApps,
    cooldownMs: num(p.cooldownMs, 30_000, 86_400_000, current.cooldownMs),
    bubbleMs: num(p.bubbleMs, 3000, 30_000, current.bubbleMs),
    maxTextChars,
    tailChars: Math.min(num(p.tailChars, 200, 8000, current.tailChars), maxTextChars),
    blockList: list(p.blockList, current.blockList),
    muted: list(p.muted, current.muted),
  };
}

/** Terminal agents' hook client names, as the bubble and mute label say them. */
const HOOK_APPS: Record<string, string> = { gizzi: 'Gizzi', claude: 'Claude Code', codex: 'Codex', cli: 'Terminal' };
const OFFER_LINK = /^allternit-offer:\/\/(accept|mute|reading)\/([A-Za-z0-9_-]{1,80})$/;

export function installDesktopCompanion(options: { origin: () => string; main: () => BrowserWindow | null; preload: string; showMain?: () => void; openHud?: (agentId: string | null, anchor?: Electron.Rectangle) => void; closeHud?: () => void; look?: CompanionLookOptions; acceptOffer?: (carry: OfferCarry, anchor: Electron.Rectangle) => void; requestAccessibility?: () => void; moveHudWithPet?: (dx: number, dy: number) => boolean; onPetLanded?: (petBounds: Electron.Rectangle) => void; onSystem?: (summary: SystemSummary) => void; openSystem?: (tab: SystemTab) => void }) {
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
  let activity: CompanionActivity = { state: 'idle', unread: false, moving: null, reading: false };
  // Unknown until the pet page says it can play walk rows (sends its motion preference); older pages never roam.
  let reducedMotion: boolean | null = null;
  let expiryTimer: ReturnType<typeof setTimeout> | undefined;
  const mainAway = () => {
    const main = options.main();
    return !main || main.isDestroyed() || !main.isVisible() || !main.isFocused();
  };
  function sendActivity(patch: Partial<CompanionActivity>) {
    const next = { ...activity, ...patch };
    if (next.state === activity.state && next.unread === activity.unread && next.moving === activity.moving && next.reading === activity.reading) return;
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
    // A terminal agent's run failed: offer to fix it (run_failed, no screen text).
    if (value === 'failed' && source.startsWith('hook:')) {
      const client = source.slice(5);
      const app = HOOK_APPS[client] ?? client;
      presentOffer({ source: 'hook', kind: 'run_failed', ruleId: 'event', line: '', app, appId: `hook:${client}`, title: '', windowKey: `hook:${client}`, text: '', botId: state().agentId ?? '' });
    }
    return true;
  }
  const clearUnread = () => sendActivity({ unread: false });

  // ─── Speech bubbles: a small click-through window above the pet ───────────
  let bubble: BrowserWindow | null = null;
  let bubbleTimer: ReturnType<typeof setTimeout> | undefined;
  // ─── Offers state (pet-offers-v1): the offer showing, the gate, the look memo ─
  let liveOffer: { offer: PetOffer; source: OfferSource; copy: string; muteLabel: string } | null = null;
  const gate = new OfferGate();
  const memo = new LookMemo();
  const BUBBLE_WIDTH = 280;
  const BUBBLE_HEIGHT = 84;
  // An offer has a line of links under its text.
  const OFFER_BUBBLE_HEIGHT = 112;
  let bubbleHeight = BUBBLE_HEIGHT;
  function placeBubble() {
    if (!bubble || bubble.isDestroyed() || !pet || pet.isDestroyed()) return;
    const p = pet.getBounds();
    const area = screen.getDisplayMatching(p).workArea;
    bubble.setBounds({ width: BUBBLE_WIDTH, height: bubbleHeight,
      x: Math.round(clamp(p.x + p.width / 2 - BUBBLE_WIDTH / 2, area.x, area.x + area.width - BUBBLE_WIDTH)),
      y: Math.round(clamp(p.y - bubbleHeight + 6, area.y, area.y + area.height - bubbleHeight)),
    });
  }
  function hideBubble() {
    clearTimeout(bubbleTimer); bubbleTimer = undefined;
    if (bubble && !bubble.isDestroyed()) { bubble.hide(); bubble.setIgnoreMouseEvents(true); }
    // The offer lives as long as its bubble: hidden, faded or replaced, it is gone.
    if (liveOffer) { liveOffer = null; pushOffer(); }
  }
  /**
   * Show a short line above the pet (hook clients, "Reply ready"); never while the pet is hidden or quick chat is open.
   * With `links` (an offer, the reading question) the bubble takes clicks until it hides; otherwise it is click-through.
   */
  function showBubble(text: string, ttlMs = 5000, links?: BubbleLink[]) {
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
      // The bubble never navigates. Its only links are offer actions, caught here.
      bubble.webContents.on('will-navigate', (event, url) => { event.preventDefault(); onBubbleLink(url); });
      bubble.on('close', event => { if (!quitting) { event.preventDefault(); bubble?.hide(); } });
    }
    const win = bubble;
    clearTimeout(bubbleTimer);
    // A new line replaces whatever the bubble showed, an offer included.
    if (liveOffer) { liveOffer = null; pushOffer(); }
    bubbleHeight = links?.length ? OFFER_BUBBLE_HEIGHT : BUBBLE_HEIGHT;
    win.setIgnoreMouseEvents(!links?.length);
    void win.loadURL(bubbleHtml(text, links)).then(() => {
      if (win.isDestroyed() || !pet || pet.isDestroyed() || !pet.isVisible()) return;
      placeBubble(); win.showInactive();
    }, () => {});
    bubbleTimer = setTimeout(hideBubble, Math.max(1500, Math.min(ttlMs, 30_000)));
    return true;
  }

  let roamTimer: ReturnType<typeof setTimeout> | undefined;
  let walkTimer: ReturnType<typeof setInterval> | undefined;
  let offerWalk: ReturnType<typeof setInterval> | undefined;
  const canRoam = () => state().enabled && state().roam !== false && reducedMotion === false && activity.state === 'idle'
    && !!pet && !pet.isDestroyed() && pet.isVisible() && ready.has(pet) && !drag && !carry && !attachedHudBounds && !offerWalk
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

  // ─── Offers: look, notice, offer, act (pet-offers-v1) ─────────────────────
  // Sources: the pet's own look at another app (readOtherApps on), a terminal
  // agent's hook "failed", and the app window reporting a failed run. Each
  // goes through the gate; an offer is a bubble with two links. Nothing is
  // sent anywhere: Fix it opens Quick Work with the text attached.
  const offerSettings = () => mergeOfferSettings(petOfferSettingsDefaults(), state().offers ?? {});
  function setOfferSettings(patch: unknown, fromUser = false) {
    const before = offerSettings();
    const next = mergeOfferSettings(before, patch);
    settings.set('offers', next);
    // Turning reading on is a click: the only moment macOS lets us ask for Accessibility.
    if (fromUser && next.readOtherApps && !before.readOtherApps) { settings.set('offersAsked', true); options.requestAccessibility?.(); }
    if (!next.enabled || (liveOffer && listNamesApp(next.muted, liveOffer.offer.app, liveOffer.offer.appId))) hideBubble();
    broadcast();
    return next;
  }
  const offerNotice = (): CompanionOfferNotice | null => liveOffer && {
    id: liveOffer.offer.id, kind: liveOffer.offer.trigger.kind, app: liveOffer.offer.app, appId: liveOffer.offer.appId,
    copy: liveOffer.copy, muteLabel: liveOffer.muteLabel, botId: liveOffer.offer.botId, createdAt: liveOffer.offer.createdAt, source: liveOffer.source,
  };
  function pushOffer() {
    const notice = offerNotice();
    for (const win of [pet, chat, options.main()]) if (win && !win.isDestroyed()) win.webContents.send('companion:offer-state', notice);
  }
  let readingTimer: ReturnType<typeof setTimeout> | undefined;
  function markReading() {
    clearTimeout(readingTimer);
    sendActivity({ reading: true });
    readingTimer = setTimeout(() => sendActivity({ reading: false }), 1200);
  }
  /** Where the pet sits on a window: on its title bar, clear of the window buttons. */
  function titleBarSeat(raw: Electron.Rectangle) {
    const b = process.platform === 'win32' && typeof screen.screenToDipRect === 'function' ? screen.screenToDipRect(null, raw) : raw;
    const side = pet && !pet.isDestroyed() ? pet.getBounds().width : state().size + 16;
    const area = screen.getDisplayMatching(b).workArea;
    // Traffic lights sit left on a Mac; minimize/maximize/close sit right on Windows.
    const x = clamp(b.x + b.width - side - (process.platform === 'win32' ? 160 : 24), b.x + 8, b.x + Math.max(8, b.width - side - 8));
    return {
      x: Math.round(clamp(x, area.x, area.x + area.width - side)),
      y: Math.round(clamp(b.y - side + 14, area.y, area.y + area.height - side)),
    };
  }
  /** Walk (or, with reduced motion, step) to `to`, then run `done`. */
  function walkTo(to: { x: number; y: number }, done: () => void) {
    stopRoam();
    clearInterval(offerWalk); offerWalk = undefined;
    if (!pet || pet.isDestroyed()) return;
    const arrive = () => {
      offerWalk && clearInterval(offerWalk); offerWalk = undefined;
      sendActivity({ moving: null });
      settings.set('position', { ...to });
      placeBubble();
      done();
    };
    if (reducedMotion !== false) { pet.setPosition(to.x, to.y); arrive(); return; }
    const from = pet.getBounds();
    let x = from.x, y = from.y, last = Date.now();
    const started = last;
    sendActivity({ moving: to.x >= x ? 'right' : 'left' });
    offerWalk = setInterval(() => {
      if (!pet || pet.isDestroyed() || drag || carry) { clearInterval(offerWalk); offerWalk = undefined; sendActivity({ moving: null }); return; }
      const now = Date.now();
      const step = (240 * (now - last)) / 1000;
      last = now;
      const dx = to.x - x, dy = to.y - y, dist = Math.hypot(dx, dy);
      if (dist <= step || now - started > 4000) { x = to.x; y = to.y; } else { x += (dx / dist) * step; y += (dy / dist) * step; }
      pet.setPosition(Math.round(x), Math.round(y));
      placeBubble();
      if (x === to.x && y === to.y) arrive();
    }, 33);
  }
  interface OfferInput {
    source: OfferSource; kind: PetOfferKind; ruleId: string; line: string;
    app: string; appId: string; title: string; windowKey: string; text: string; botId: string;
  }
  /** Gate an offer and show it (walking to `seat` first). True when it will show. */
  function presentOffer(input: OfferInput, windowBounds?: Electron.Rectangle): boolean {
    const now = Date.now();
    const s = offerSettings();
    const candidate = { source: input.source, app: input.app, appId: input.appId, windowKey: input.windowKey, line: input.line };
    if (gate.check(candidate, s, now) !== 'ok') return false;
    // Not while the pet is hidden or busy with you: Quick Work open, dragged, carried.
    if (!state().enabled || !pet || pet.isDestroyed() || !pet.isVisible() || drag || carry || attachedHudBounds
      || (chat && !chat.isDestroyed() && chat.isVisible())) return false;
    gate.record(candidate, now);
    const app = input.app.trim() || PET_OFFER_COPY.appFallback;
    const offer: PetOffer = {
      id: randomUUID(), surface: 'desktop', app, appId: input.appId, title: input.title, windowKey: input.windowKey,
      trigger: { kind: input.kind, ruleId: input.ruleId, line: input.line }, text: input.text, botId: input.botId, createdAt: now,
    };
    const copy = offerCopy(input.kind, app);
    const muteLabel = offerMuteLabel(app);
    const show = () => {
      const shown = showBubble(copy, s.bubbleMs, [
        { href: `allternit-offer://accept/${offer.id}`, label: PET_OFFER_COPY.actions.fixIt },
        { href: `allternit-offer://mute/${offer.id}`, label: muteLabel },
      ]);
      if (shown) { liveOffer = { offer, source: input.source, copy, muteLabel }; pushOffer(); }
    };
    if (windowBounds) walkTo(titleBarSeat(windowBounds), show); else show();
    return true;
  }
  /** Fix it: Quick Work opens by the pet with the text attached and the line as the draft. Nothing is sent. */
  function acceptOffer(id: string) {
    const live = liveOffer;
    if (!live || live.offer.id !== id || !pet || pet.isDestroyed()) return false;
    hideBubble();
    clearUnread(); stopRoam(true);
    const anchor = pet.getBounds();
    const agentId = live.offer.botId || state().agentId;
    if (options.openHud) options.openHud(agentId, anchor); else showPanel('chat');
    // After openHud: switching bots there drops the old carry context.
    options.acceptOffer?.({
      app: live.offer.app, title: live.offer.title, text: live.offer.text, intent: live.copy,
      computer: live.source === 'look', kind: live.offer.trigger.kind,
    }, anchor);
    return true;
  }
  // The app the pet last looked at (name and id only), for a long-press mute with no offer up.
  let lastLooked: { app: string; appId: string } | null = null;
  /** Mute an app (an id/name, else the offer's app, else the last one looked at). Returns the name muted. */
  function muteApp(which?: unknown): string | null {
    let entry = '';
    let label = '';
    if (typeof which === 'string' && which.trim()) entry = label = which.trim().slice(0, 200);
    else {
      const target = liveOffer ? { app: liveOffer.offer.app, appId: liveOffer.offer.appId } : lastLooked;
      if (!target) return null;
      const named = target.app && target.app !== PET_OFFER_COPY.appFallback;
      entry = named ? target.app : target.appId;
      label = named ? target.app : PET_OFFER_COPY.appFallback;
    }
    if (!entry) return null;
    setOfferSettings({ muted: [...offerSettings().muted, entry] });
    showBubble(`Muted for ${label}`, 3000);
    return label;
  }
  function onBubbleLink(url: string) {
    const m = OFFER_LINK.exec(url);
    if (!m) return;
    const [, action, arg] = m;
    if (action === 'accept') acceptOffer(arg);
    else if (action === 'mute') { if (liveOffer?.offer.id === arg) muteApp(); }
    else if (action === 'reading') {
      hideBubble();
      settings.set('offersAsked', true);
      if (arg === 'on') { setOfferSettings({ readOtherApps: true }, true); showBubble('Reading is on. Turn it off in pet settings.', 4000); }
    }
  }

  // The look: one window per tick (decisions 9 and 10), at the roam cadence.
  // Paused while any bot is busy, a computer-use run is active, Quick Work or
  // quick chat is open, the pet is moving, or an offer is already up.
  let lookTimer: ReturnType<typeof setTimeout> | undefined;
  let looking = false;
  const installedAt = Date.now();
  const lookPlatform = process.platform === 'darwin' || process.platform === 'win32';
  const petFree = () => state().enabled && !!pet && !pet.isDestroyed() && pet.isVisible() && ready.has(pet) && activity.state === 'idle'
    && !drag && !carry && !attachedHudBounds && !offerWalk && !liveOffer && !bubbleTimer
    && !(chat && !chat.isDestroyed() && chat.isVisible());
  function scheduleLook() {
    if (lookTimer || !options.look || !lookPlatform || quitting) return;
    lookTimer = setTimeout(() => { lookTimer = undefined; void lookTick().finally(scheduleLook); }, 6000 + Math.random() * 12000);
  }
  async function lookTick() {
    const look = options.look;
    if (!look || looking || !petFree()) return;
    const s = offerSettings();
    if (!s.enabled) return;
    if (!s.readOtherApps) {
      // Ask once, a couple of minutes in, on a click only.
      if (!state().offersAsked && Date.now() - installedAt > 120_000) {
        settings.set('offersAsked', true);
        showBubble("Turn on reading?\nI'll spot errors in your other apps. Nothing is sent until you say so.", 20_000, [
          { href: 'allternit-offer://reading/on', label: 'Turn on reading' },
          { href: 'allternit-offer://reading/off', label: 'Not now' },
        ]);
      }
      return;
    }
    if (look.computerUseActive?.()) return;
    looking = true;
    try {
      const windows = await look.windows().catch(() => null);
      if (!windows || !pet || pet.isDestroyed()) return;
      const p = pet.getBounds();
      const target = pickLookTarget(windows, { x: p.x + p.width / 2, y: p.y + p.height / 2 });
      if (!target) return;
      const out = await lookAt(target, s, memo, { driver: look.driver, onRead: markReading });
      if ('skip' in out) return;
      lastLooked = { app: out.app, appId: out.appId };
      const trigger = matchOffer(out.text, { dialog: out.dialog, maxTextChars: s.maxTextChars });
      // Things may have changed during the read (a bot started, you opened Quick Work).
      if (!trigger || !petFree() || look.computerUseActive?.()) return;
      presentOffer({
        source: 'look', kind: trigger.kind, ruleId: trigger.ruleId, line: trigger.line,
        app: out.app, appId: out.appId, title: out.title, windowKey: out.windowKey, text: out.text, botId: state().agentId ?? '',
      }, out.bounds);
    } finally {
      looking = false;
    }
  }
  app.whenReady().then(() => scheduleLook());

  /** An in-app failure from the app window. Shown by the pet only while you are away from the app (the app shows its own card). */
  function offerFromApp(value: unknown): boolean {
    if (!value || typeof value !== 'object' || !mainAway()) return false;
    const v = value as Record<string, unknown>;
    const kind = PET_OFFER_KINDS.find(k => k === v.kind);
    if (!kind) return false;
    const cut = (x: unknown, max: number) => (typeof x === 'string' ? x.trim().slice(0, max) : '');
    const text = typeof v.text === 'string' ? normalizeOfferText(v.text, offerSettings().maxTextChars) : '';
    const lastLine = text.split('\n').map(l => l.trim()).filter(Boolean).at(-1) ?? '';
    const botId = cut(v.botId, 200) || (state().agentId ?? '');
    return presentOffer({
      source: 'app', kind, ruleId: 'event', line: (cut(v.line, 4000) || lastLine).slice(0, PET_OFFER_LIMITS.lineMaxChars),
      app: cut(v.app, 80) || 'Allternit', appId: cut(v.appId, 200) || 'allternit', title: cut(v.title, 300),
      windowKey: cut(v.windowKey, 300) || `app:${botId}`, text, botId,
    });
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
  // Offers. Settings and mute come from the pet, quick chat or the app window;
  // only the app window reports an in-app failure.
  handle('companion:offer', (event, value) => {
    const main = options.main();
    if (!main || main.isDestroyed() || event.sender !== main.webContents) return false;
    return offerFromApp(value);
  });
  handle('companion:offer-current', () => offerNotice());
  handle('companion:offer-accept', (_event, id) => typeof id === 'string' && acceptOffer(id));
  handle('companion:offer-mute', (_event, appId) => muteApp(appId));
  handle('companion:offer-settings-get', () => offerSettings());
  handle('companion:offer-settings-set', (_event, patch) => setOfferSettings(patch, true));
  handle('companion:apps', async () => {
    const windows = await options.look?.windows().catch(() => null);
    const apps = new Map<string, { name: string; appId: string }>();
    for (const w of windows ?? []) if (w.app && !apps.has(w.app.toLowerCase())) apps.set(w.app.toLowerCase(), { name: w.app, appId: w.appId });
    return [...apps.values()].sort((a, b) => a.name.localeCompare(b.name));
  });

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
  app.on('before-quit', () => { quitting = true; if (carry) { clearInterval(carry.timer); clearTimeout(carry.expires); carry = null; } stopRoam(true); clearTimeout(expiryTimer); clearTimeout(lookTimer); clearTimeout(readingTimer); clearInterval(offerWalk); hideBubble(); });
  return { reportActivity, showBubble, system: () => system, activity: () => activity, start: () => { if (state().enabled) ensurePet(); }, show: () => update({ enabled: true }), settings: () => showPanel('settings'), reset, attachPetToHud, detachPetFromHud };
}

/** A link in an offer bubble: only allternit-offer:// actions, caught by the bubble's will-navigate. */
export interface BubbleLink { href: string; label: string }

/** The bubble page: escaped text and plain offer links only, no scripts, follows the system theme. */
export function bubbleHtml(text: string, links: BubbleLink[] = []): string {
  const esc = (v: string) => v.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]!));
  const safe = esc(text);
  const actions = links.filter(l => l.href.startsWith('allternit-offer://'));
  // One line wraps to two at most. Several lines (the system summary) get a
  // row each, cut with an ellipsis. The clamp sits on a span inside the
  // padded box, so a cut line never shows in the padding.
  const lines = safe.split('\n').filter(Boolean);
  const html = `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'"><style>
:root{--bg:#fff;--fg:#1c1b1a;--edge:rgba(0,0,0,.12);color-scheme:light}
@media (prefers-color-scheme:dark){:root{--bg:#26262a;--fg:#f3f3f3;--edge:rgba(255,255,255,.16);color-scheme:dark}}
html,body{margin:0;height:100%;background:transparent;overflow:hidden}
body{display:flex;align-items:flex-end;justify-content:center;padding:0 4px 10px;box-sizing:border-box;font:500 12px/1.35 -apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif}
p{position:relative;margin:0;max-width:100%;min-width:0;padding:6px 10px;border-radius:12px;background:var(--bg);color:var(--fg);border:1px solid var(--edge);box-shadow:0 2px 8px rgba(0,0,0,.18);box-sizing:border-box}
span{display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden;word-break:break-word}
p.lines span{display:block;white-space:nowrap;text-overflow:ellipsis}
p.offer span{-webkit-line-clamp:3}
p.offer span+span{font-weight:400;opacity:.8}
nav{display:flex;gap:12px;margin-top:6px}
a{color:inherit;font-weight:600;text-decoration:underline;text-underline-offset:2px;cursor:pointer}
a:focus-visible{outline:2px solid currentColor;outline-offset:2px;border-radius:3px}
</style></head><body><p role="status"${actions.length ? ' class="offer"' : lines.length > 1 ? ' class="lines"' : ''}>${lines.map(line => `<span>${line}</span>`).join('')}${actions.length ? `<nav>${actions.map(l => `<a href="${esc(l.href)}">${esc(l.label)}</a>`).join('')}</nav>` : ''}</p></body></html>`;
  return `data:text/html;charset=utf-8,${encodeURIComponent(html)}`;
}
