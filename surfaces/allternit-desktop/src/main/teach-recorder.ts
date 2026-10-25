/**
 * Teach recorder (Teach v2): records a HAR plus a timeline of user actions
 * (page URLs, clicks/inputs in in-app browser webviews, frontmost-app switches,
 * local screen snapshots and optional mic narration) for the Teach view.
 *
 * Privacy: snapshots live only in memory (ring of MAX_FRAMES) and are served
 * to the local renderer on request. Nothing here sends frames anywhere.
 */
import { app, BrowserWindow, desktopCapturer, globalShortcut, ipcMain, screen, session, systemPreferences, webContents, type WebContents } from 'electron';
import { spawn, execFile, type ChildProcess } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import log from 'electron-log';
import { createCaptureSession, stopCaptureSession } from './browser-capture-manager.js';
import { voiceManager } from './voice-manager.js';

export const TEACH_HOTKEY = 'CommandOrControl+Shift+R';

export type TeachEvent =
  | { t: number; kind: 'navigate'; url: string; title?: string }
  | { t: number; kind: 'click'; url?: string; label: string; role?: string; selector?: string; id?: string; name?: string; frame?: string; tag?: string; type?: string }
  | { t: number; kind: 'input'; url?: string; label: string; field?: string; valuePreview?: string; id?: string; name?: string; frame?: string; tag?: string; type?: string }
  | { t: number; kind: 'key'; url?: string; key: string; label?: string; frame?: string }
  | { t: number; kind: 'select'; url?: string; label: string; value: string; css?: string; frame?: string }
  | { t: number; kind: 'upload'; url?: string; label: string; fileName: string; css?: string }
  | { t: number; kind: 'download'; url?: string; fileName: string; fromUrl?: string }
  | { t: number; kind: 'copy'; url?: string; textPreview: string }
  | { t: number; kind: 'paste'; url?: string; label: string; css?: string }
  | { t: number; kind: 'app'; app: string; window?: string }
  | { t: number; kind: 'snapshot'; id: string }
  | { t: number; kind: 'narration'; text: string };

export interface TeachRecording {
  sessionId: string;
  startedAt: number;
  endedAt: number;
  har: string;
  events: TeachEvent[];
  narration?: string;
  /** True when Screen Recording permission was missing, so no snapshots were taken. */
  snapshotsSkipped?: boolean;
}

export interface TeachState {
  recording: boolean;
  sessionId?: string;
  startedAt?: number;
  narration: boolean;
}

interface TeachDeps {
  getMainWindow: () => BrowserWindow | null;
  /** True for webContents that are Allternit's own UI (not the page being taught). */
  isInternalWebContents: (wc: WebContents) => boolean;
}

const MAX_FRAMES = 120;
const SNAPSHOT_EVERY_MS = 2000;
const SNAPSHOT_FORCE_MS = 10_000;
const APP_POLL_MS = 1500;
const NARRATION_CHUNK_MS = 30_000;
const MIN_WAV_BYTES = 44 + 16000 * 2; // ignore chunks shorter than ~1s
const MSG_PREFIX = '__allternit_teach__:';
const SENSITIVE_NAME = /pass|secret|token|card|cvv|ssn/i;

let deps: TeachDeps | null = null;
let active: ActiveSession | null = null;
let barWindow: BrowserWindow | null = null;
let pendingFinished: TeachRecording | null = null;
let narrationOn = false;

interface ActiveSession {
  sessionId: string;
  captureId: string;
  startedAt: number;
  events: TeachEvent[];
  frames: Map<string, Buffer>;
  frameOrder: string[];
  frameSeq: number;
  narrationParts: string[];
  chunkStartT: number;
  transcribeQueue: Promise<void>;
  snapshotsSkipped: boolean;
  lastHash: Uint8Array | null;
  lastFrameAt: number;
  lastApp: string;
  timers: NodeJS.Timeout[];
  child: ChildProcess | null;
  guests: Set<WebContents>;
  /** Sessions with a will-download listener while recording. */
  downloadSessions: Map<Electron.Session, (e: unknown, item: Electron.DownloadItem, wc?: WebContents) => void>;
  /** Tabs already open when recording began, whose page is logged only once you act in them. */
  unseenStart: Set<WebContents>;
  onCreated: (e: unknown, wc: WebContents) => void;
  snapshotBusy: boolean;
  stopping: boolean;
}

// Snapshots outlive the session so the Review/Editor can still show them.
const keptFrames = new Map<string, Buffer>();

function getState(): TeachState {
  return active
    ? { recording: true, sessionId: active.sessionId, startedAt: active.startedAt, narration: narrationOn }
    : { recording: false, narration: false };
}

function broadcastState(): void {
  const w = deps?.getMainWindow();
  if (w && !w.isDestroyed()) w.webContents.send('teach:state', getState());
}

/** Omit that keeps each union member's own fields. */
type EventInput = TeachEvent extends infer E ? (E extends unknown ? Omit<E, 't'> & { t?: number } : never) : never;

function pushEvent(ev: EventInput): void {
  if (!active || active.stopping) return;
  active.events.push({ ...ev, t: ev.t ?? Date.now() - active.startedAt } as TeachEvent);
}

// ─── Page listeners (webview / page webContents) ───────────────────────────

const PAGE_SCRIPT = `(() => {
  if (window.__allternitTeach) return;
  window.__allternitTeach = true;
  const P = ${JSON.stringify(MSG_PREFIX)};
  const SENSITIVE = /pass|secret|token|card|cvv|ssn/i;
  const send = (o) => { try { o.f = location.href; o.top = window.top === window; console.debug(P + JSON.stringify(o)); } catch (e) {} };
  const clip = (s, n) => String(s == null ? '' : s).replace(/\\s+/g, ' ').trim().slice(0, n);
  // "Customer name:" → "Customer name" (the UI adds its own colon).
  const noColon = (s) => s.replace(/\\s*:\\s*$/, '');
  const interactive = 'a,button,input,select,textarea,summary,label,[role],[onclick],[tabindex]';
  const cssEsc = (v) => (window.CSS && CSS.escape ? CSS.escape(v) : v);
  // What a person sees next to a field: its <label>, else aria/placeholder/name.
  const fieldLabel = (el) => {
    let text = el.getAttribute('aria-label') || '';
    if (!text && el.id) { const l = document.querySelector('label[for="' + cssEsc(el.id) + '"]'); if (l) text = l.innerText || l.textContent || ''; }
    if (!text) { const l = el.closest('label'); if (l) text = l.innerText || l.textContent || ''; }
    return noColon(clip(text || el.getAttribute('placeholder') || el.getAttribute('name') || el.getAttribute('value') || el.tagName.toLowerCase(), 80));
  };
  const groupLabel = (el) => {
    const fs = el.closest('fieldset');
    const legend = fs && fs.querySelector('legend');
    return noColon(clip((legend && (legend.innerText || legend.textContent)) || el.getAttribute('name') || '', 80));
  };
  // Icon-only buttons: aria-labelledby, title, an <svg><title>, or what a submit button does.
  const iconLabel = (el) => {
    const by = el.getAttribute('aria-labelledby');
    const byEl = by ? document.getElementById(by.split(' ')[0]) : null;
    const svgTitle = el.querySelector && el.querySelector('svg title');
    const text = (byEl && (byEl.innerText || byEl.textContent)) || el.getAttribute('title') || (svgTitle && svgTitle.textContent) || '';
    if (text) return clip(text, 80);
    const form = el.closest && el.closest('form');
    if (form && (el.getAttribute('type') || 'submit') === 'submit') {
      return form.querySelector('input[type=search], [role=searchbox]') ? 'Search' : 'Submit';
    }
    return '';
  };
  const textLabel = (el) => clip(el.getAttribute('aria-label') || el.innerText || el.textContent || el.getAttribute('placeholder') || el.getAttribute('name') || el.getAttribute('title') || el.getAttribute('alt') || '', 80);
  const label = (el) => {
    if (/^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName)) return fieldLabel(el);
    return textLabel(el) || iconLabel(el) || el.tagName.toLowerCase();
  };
  const selector = (el) => {
    const parts = [];
    let cur = el;
    for (let i = 0; cur && cur.nodeType === 1 && i < 3; i++) {
      if (cur.id && /^[A-Za-z][\\w-]{0,40}$/.test(cur.id)) { parts.unshift('#' + cur.id); break; }
      let s = cur.tagName.toLowerCase();
      const cls = (cur.getAttribute('class') || '').split(/\\s+/).filter((c) => /^[A-Za-z][\\w-]{0,24}$/.test(c))[0];
      if (cls) s += '.' + cls;
      const p = cur.parentElement;
      if (p) { const same = Array.prototype.filter.call(p.children, (c) => c.tagName === cur.tagName); if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')'; }
      parts.unshift(s);
      cur = p;
    }
    return parts.join(' > ').slice(0, 160);
  };
  const isSensitive = (el) => {
    const type = (el.getAttribute('type') || '').toLowerCase();
    const ac = (el.getAttribute('autocomplete') || '').toLowerCase();
    const name = (el.getAttribute('name') || '') + ' ' + (el.id || '') + ' ' + (el.getAttribute('aria-label') || '') + ' ' + (el.getAttribute('placeholder') || '');
    return type === 'password' || ac.indexOf('cc-') === 0 || ac === 'one-time-code' || SENSITIVE.test(name);
  };
  const isField = (el) => /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName);
  // A sensitive field is named by its name attribute only, never its visible text.
  const safeLabel = (el) => (isField(el) && isSensitive(el)) ? (el.getAttribute('name') || el.tagName.toLowerCase()) : label(el);
  const attrs = (o, el) => {
    if (el.id) o.id = clip(el.id, 60);
    const nm = el.getAttribute('name'); if (nm) o.name = clip(nm, 60);
    o.tag = el.tagName.toLowerCase();
    const ty = el.getAttribute('type'); if (ty) o.type = clip(ty, 20).toLowerCase();
    return o;
  };
  // Rich editors (contenteditable): one input event per editor, sent when focus leaves it.
  const dirty = new Set();
  const editorRoot = (el) => (el && el.closest) ? el.closest('[contenteditable=""],[contenteditable="true"],[contenteditable="plaintext-only"]') : null;
  const flushEditor = (ed) => {
    if (!dirty.has(ed)) return;
    dirty.delete(ed);
    const sens = isSensitive(ed);
    const o = attrs({ k: 'input', label: safeLabel(ed), field: clip(ed.getAttribute('name') || ed.id || '', 40) || undefined, css: selector(ed) }, ed);
    o.tag = 'contenteditable';
    if (!sens) o.valuePreview = clip(ed.innerText || ed.textContent, 40);
    send(o);
  };
  const flushAll = () => { Array.from(dirty).forEach(flushEditor); };
  document.addEventListener('input', (e) => {
    const ed = editorRoot(e.target);
    if (ed && !isField(e.target)) dirty.add(ed);
  }, true);
  document.addEventListener('focusout', (e) => {
    const ed = editorRoot(e.target);
    if (ed) flushEditor(ed);
  }, true);
  window.addEventListener('pagehide', flushAll, true);
  // The control that opened a listbox, when the page links them with aria-controls.
  const listboxLabel = (lb) => {
    let text = lb.getAttribute('aria-label') || '';
    const by = lb.getAttribute('aria-labelledby');
    if (!text && by) { const be = document.getElementById(by.split(' ')[0]); if (be) text = be.innerText || be.textContent || ''; }
    if (!text && lb.id) { const trig = document.querySelector('[aria-controls="' + cssEsc(lb.id) + '"]'); if (trig) text = isField(trig) ? fieldLabel(trig) : (trig.getAttribute('aria-label') || trig.getAttribute('title') || ''); }
    return noColon(clip(text || 'option list', 80));
  };
  document.addEventListener('click', (e) => {
    flushAll();
    const raw = e.target && e.target.closest ? e.target : null;
    const opt = raw && raw.closest('[role=option]');
    if (opt) {
      const lb = opt.closest('[role=listbox],[role=combobox]');
      if (lb || opt.getAttribute('aria-selected') != null || opt.id) {
        send({ k: 'select', label: lb ? listboxLabel(lb) : 'option list', value: clip(opt.innerText || opt.textContent || opt.getAttribute('aria-label'), 80), css: selector(opt) });
        return;
      }
    }
    const t = raw ? raw.closest(interactive) || raw : null;
    if (!t || t.nodeType !== 1) return;
    // A native <select> reports through its change event; its click is not a step.
    if (t.tagName === 'SELECT') return;
    send(attrs({ k: 'click', label: safeLabel(t), role: t.getAttribute('role') || t.tagName.toLowerCase(), selector: selector(t) }, t));
  }, true);
  document.addEventListener('change', (e) => {
    const t = e.target;
    if (!t || !isField(t)) return;
    const type = (t.getAttribute('type') || '').toLowerCase();
    if (type === 'hidden') return;
    if (type === 'file') {
      const names = Array.prototype.map.call(t.files || [], (f) => f.name).join(', ');
      if (names) send({ k: 'upload', label: fieldLabel(t), fileName: clip(names, 120), css: selector(t) });
      return;
    }
    if (t.tagName === 'SELECT') {
      const sens = isSensitive(t);
      send({ k: 'select', label: sens ? (t.getAttribute('name') || 'select') : fieldLabel(t), value: sens ? '' : clip(t.selectedOptions && t.selectedOptions[0] ? t.selectedOptions[0].text : t.value, 80), css: selector(t) });
      return;
    }
    const field = clip(t.getAttribute('name') || t.id || '', 40);
    const choice = type === 'checkbox' || type === 'radio';
    // A radio/checkbox is named by its group ("Pizza Size"); the value is the option picked ("Medium").
    const lbl = choice ? (groupLabel(t) || fieldLabel(t)) : fieldLabel(t);
    const o = attrs({ k: 'input', label: lbl, field: field || undefined }, t);
    if (!isSensitive(t)) {
      if (choice) o.valuePreview = clip((type === 'checkbox' && !t.checked ? 'unchecked: ' : '') + fieldLabel(t), 40);
      else o.valuePreview = clip(t.value, 40);
    }
    send(o);
  }, true);
  // Enter / Tab / Escape and modifier shortcuts only; never plain characters.
  document.addEventListener('keydown', (e) => {
    if (e.isComposing || !e.key) return;
    let key = '';
    if (e.key === 'Enter' || e.key === 'Tab' || e.key === 'Escape') key = e.key;
    else if ((e.metaKey || e.ctrlKey) && e.key.length === 1 && !e.altKey) key = (e.metaKey ? 'Meta+' : 'Ctrl+') + (e.shiftKey ? 'Shift+' : '') + e.key.toLowerCase();
    if (!key) return;
    if (key === 'Enter' || key === 'Tab') flushAll();
    const a = document.activeElement;
    const o = { k: 'key', key: key };
    if (a && a.nodeType === 1 && a !== document.body && a !== document.documentElement) o.label = safeLabel(a);
    send(o);
  }, true);
  const fileNames = (dt) => Array.prototype.map.call((dt && dt.files) || [], (f) => f.name).join(', ');
  document.addEventListener('drop', (e) => {
    const names = fileNames(e.dataTransfer);
    if (!names) return;
    const t = e.target && e.target.closest ? e.target : null;
    if (!t) return;
    send({ k: 'upload', label: safeLabel(t), fileName: clip(names, 120), css: selector(t) });
  }, true);
  document.addEventListener('copy', () => {
    const a = document.activeElement;
    if (a && a.nodeType === 1 && (isSensitive(a) || ed_sensitive(a))) return;
    let text = '';
    if (a && /^(INPUT|TEXTAREA)$/.test(a.tagName)) { try { text = String(a.value || '').slice(a.selectionStart || 0, a.selectionEnd || 0); } catch (err) {} }
    else { const sel = window.getSelection && window.getSelection(); text = sel ? sel.toString() : ''; }
    text = clip(text, 40);
    if (text) send({ k: 'copy', textPreview: text });
  }, true);
  const ed_sensitive = (el) => { const ed = editorRoot(el); return !!ed && isSensitive(ed); };
  document.addEventListener('paste', (e) => {
    const t = e.target && e.target.nodeType === 1 ? e.target : null;
    if (!t) return;
    send({ k: 'paste', label: safeLabel(t), css: selector(t) });
  }, true);
})();`;

function attachGuest(wc: WebContents): void {
  const s = active;
  if (!s || s.guests.has(wc) || wc.isDestroyed()) return;
  const internal = deps?.isInternalWebContents(wc) ?? false;
  const type = wc.getType();
  if (type !== 'webview' && type !== 'window') return;
  if (type === 'window' && internal) return;
  s.guests.add(wc);

  // Pages rewrite their own address (history.replaceState, hash updates);
  // only a change of URL is a step.
  let lastUrl = safeUrl(wc);
  const nav = (_e: unknown, url: string) => {
    if (!/^https?:/i.test(url)) return;
    if (url === lastUrl) return;
    lastUrl = url;
    s.unseenStart.delete(wc);
    const last = [...s.events].reverse().find((e) => e.kind === 'navigate') as Extract<TeachEvent, { kind: 'navigate' }> | undefined;
    if (last && last.url === url && Date.now() - s.startedAt - last.t < 800) return;
    pushEvent({ kind: 'navigate', url, title: safeTitle(wc) });
  };
  wc.on('did-navigate', nav);
  wc.on('did-navigate-in-page', (e, url, isMain) => { if (isMain !== false) nav(e, url); });

  // Every frame, sub-frames included; the page script is idempotent per frame.
  const inject = () => {
    if (!active || wc.isDestroyed()) return;
    try {
      for (const frame of wc.mainFrame.framesInSubtree) {
        if (!/^https?:/i.test(frame.url)) continue;
        frame.executeJavaScript(PAGE_SCRIPT, true).catch(() => {});
      }
    } catch { /* frame went away */ }
  };
  wc.on('dom-ready', inject);
  wc.on('did-frame-finish-load', inject);
  if (!wc.isLoading()) inject();

  // Downloads belong to the session, once per session while recording.
  const ses = wc.session;
  if (ses && !s.downloadSessions.has(ses)) {
    const onDownload = (_e: unknown, item: Electron.DownloadItem) => {
      let fromUrl: string | undefined;
      try {
        const u = new URL(item.getURL());
        if (/^https?:$/i.test(u.protocol)) fromUrl = (u.origin + u.pathname).slice(0, 300);
      } catch { /* blob: / data: */ }
      pushEvent({ kind: 'download', url: undefined, fileName: path.basename(item.getFilename() || '').slice(0, 120), fromUrl });
    };
    s.downloadSessions.set(ses, onDownload);
    ses.on('will-download', onDownload as never);
  }

  wc.on('console-message', (...args: unknown[]) => {
    // Electron <35: (event, level, message); newer: (event) with event.message.
    const first = args[0] as { message?: string; frame?: { url?: string; parent?: unknown } } | undefined;
    const message = typeof args[2] === 'string' ? (args[2] as string) : first?.message;
    if (typeof message !== 'string' || !message.startsWith(MSG_PREFIX)) return;
    // Newer Electron reports the frame on the event; the page script also tags f/top as a fallback.
    const evFrame = first?.frame && first.frame.parent ? first.frame.url : undefined;
    handlePageMessage(wc, message.slice(MSG_PREFIX.length), evFrame);
  });
  wc.once('destroyed', () => { s.guests.delete(wc); s.unseenStart.delete(wc); });

  // Every open tab is attached, but only the ones you use belong in the
  // walkthrough: a tab's starting page is logged when you first click or type
  // there (handlePageMessage), not for each background tab at Start.
  if (type === 'webview' && safeUrl(wc)) s.unseenStart.add(wc);
}

function safeTitle(wc: WebContents): string | undefined {
  try { return wc.getTitle() || undefined; } catch { return undefined; }
}
function safeUrl(wc: WebContents): string | undefined {
  try { const u = wc.getURL(); return /^https?:/i.test(u) ? u : undefined; } catch { return undefined; }
}

function handlePageMessage(wc: WebContents, raw: string, evFrame?: string): void {
  let m: {
    k?: string; label?: string; role?: string; selector?: string; field?: string; valuePreview?: string;
    id?: string; name?: string; tag?: string; type?: string; key?: string; value?: string; css?: string;
    fileName?: string; textPreview?: string; f?: string; top?: boolean;
  };
  try { m = JSON.parse(raw); } catch { return; }
  const url = safeUrl(wc);
  const str = (v: unknown, n: number): string | undefined => (typeof v === 'string' && v ? v.slice(0, n) : undefined);
  const label = String(m.label ?? '').slice(0, 80);
  const frame = evFrame ?? (m.top === false ? str(m.f, 300) : undefined);
  const kinds = ['click', 'input', 'key', 'select', 'upload', 'copy', 'paste'];
  if (m.k && kinds.includes(m.k) && active?.unseenStart.delete(wc) && url) {
    pushEvent({ kind: 'navigate', url, title: safeTitle(wc) });
  }
  const css = str(m.css, 160);
  const isSensitive = (...names: Array<string | undefined>) => names.some((n) => !!n && SENSITIVE_NAME.test(n));
  if (m.k === 'click') {
    pushEvent({ kind: 'click', url, label, role: m.role, selector: m.selector, id: str(m.id, 60), name: str(m.name, 60), frame, tag: str(m.tag, 20), type: str(m.type, 20) });
  } else if (m.k === 'input') {
    const field = m.field ? String(m.field).slice(0, 40) : undefined;
    const sensitive = isSensitive(field, label, m.name, m.id) || m.type === 'password';
    const valuePreview = !sensitive && typeof m.valuePreview === 'string' ? m.valuePreview.slice(0, 40) : undefined;
    pushEvent({ kind: 'input', url, label, field, valuePreview, id: str(m.id, 60), name: str(m.name, 60), frame, tag: str(m.tag, 20), type: str(m.type, 20) });
  } else if (m.k === 'key') {
    const key = str(m.key, 20);
    if (key) pushEvent({ kind: 'key', url, key, label: label || undefined, frame });
  } else if (m.k === 'select') {
    pushEvent({ kind: 'select', url, label, value: isSensitive(label) ? '' : String(m.value ?? '').slice(0, 80), css, frame });
  } else if (m.k === 'upload') {
    const fileName = str(m.fileName, 120);
    if (fileName) pushEvent({ kind: 'upload', url, label, fileName: fileName.split(/[\\/]/).pop() || fileName, css });
  } else if (m.k === 'copy') {
    const textPreview = str(m.textPreview, 40);
    if (textPreview) pushEvent({ kind: 'copy', url, textPreview });
  } else if (m.k === 'paste') {
    pushEvent({ kind: 'paste', url, label, css });
  }
}

// ─── Frontmost app ─────────────────────────────────────────────────────────

function emitApp(name: string, windowTitle?: string): void {
  const s = active;
  if (!s || !name) return;
  const key = `${name}|${windowTitle ?? ''}`;
  if (key === s.lastApp) return;
  s.lastApp = key;
  pushEvent({ kind: 'app', app: name, window: windowTitle || undefined });
}

function execOut(file: string, args: string[]): Promise<string> {
  return new Promise((resolve) => {
    execFile(file, args, { timeout: 2000 }, (err, stdout) => resolve(err ? '' : String(stdout)));
  });
}

async function pollMacApp(): Promise<void> {
  const asn = (await execOut('lsappinfo', ['front'])).trim();
  if (!asn) return;
  const info = await execOut('lsappinfo', ['info', '-only', 'name', asn]);
  const m = /"LSDisplayName"\s*=\s*"([^"]+)"/.exec(info);
  if (m) emitApp(m[1]);
}

async function pollLinuxApp(): Promise<void> {
  const title = (await execOut('xdotool', ['getactivewindow', 'getwindowname'])).trim();
  const pid = (await execOut('xdotool', ['getactivewindow', 'getwindowpid'])).trim();
  let name = '';
  if (pid) { try { name = fs.readFileSync(`/proc/${pid}/comm`, 'utf8').trim(); } catch { /* ignore */ } }
  if (name || title) emitApp(name || title, title || undefined);
}

const WIN_PS = `
Add-Type @"
using System; using System.Runtime.InteropServices;
public class FG { [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern int GetWindowThreadProcessId(IntPtr h, out int pid); }
"@
$last = ''
while ($true) {
  try {
    $h = [FG]::GetForegroundWindow(); $p = 0
    [void][FG]::GetWindowThreadProcessId($h, [ref]$p)
    $pr = Get-Process -Id $p -ErrorAction Stop
    $line = $pr.ProcessName + '|' + $pr.MainWindowTitle
    if ($line -ne $last) { $last = $line; [Console]::Out.WriteLine($line); [Console]::Out.Flush() }
  } catch {}
  Start-Sleep -Milliseconds 1000
}
`;

function startAppWatcher(s: ActiveSession): void {
  if (process.platform === 'darwin') {
    void pollMacApp();
    s.timers.push(setInterval(() => { void pollMacApp(); }, APP_POLL_MS));
  } else if (process.platform === 'win32') {
    try {
      const child = spawn('powershell.exe', ['-NoProfile', '-NonInteractive', '-WindowStyle', 'Hidden', '-Command', WIN_PS], { stdio: ['ignore', 'pipe', 'ignore'], windowsHide: true });
      s.child = child;
      let buf = '';
      child.stdout?.on('data', (d: Buffer) => {
        buf += d.toString('utf8');
        let i: number;
        while ((i = buf.indexOf('\n')) >= 0) {
          const line = buf.slice(0, i).trim();
          buf = buf.slice(i + 1);
          const [name, ...rest] = line.split('|');
          if (name) emitApp(name, rest.join('|').trim() || undefined);
        }
      });
      child.on('error', () => { s.child = null; });
    } catch (err) {
      log.warn('[Teach] PowerShell watcher failed', err);
    }
  } else if (process.platform === 'linux') {
    execFile('xdotool', ['version'], { timeout: 2000 }, (err) => {
      if (err || active !== s) return;
      s.timers.push(setInterval(() => { void pollLinuxApp(); }, APP_POLL_MS));
    });
  }
}

// ─── Snapshots ─────────────────────────────────────────────────────────────

function screenAllowed(): boolean {
  if (process.platform !== 'darwin') return true;
  try { return systemPreferences.getMediaAccessStatus('screen') === 'granted'; } catch { return false; }
}

function hashDiff(a: Uint8Array, b: Uint8Array): number {
  let sum = 0;
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i += 4) sum += Math.abs(a[i] - b[i]) + Math.abs(a[i + 1] - b[i + 1]) + Math.abs(a[i + 2] - b[i + 2]);
  return sum / ((n / 4) * 3);
}

async function takeSnapshot(s: ActiveSession): Promise<void> {
  if (s.snapshotBusy || s.stopping) return;
  if (!screenAllowed()) { s.snapshotsSkipped = true; return; }
  s.snapshotBusy = true;
  try {
    const disp = screen.getPrimaryDisplay();
    const width = 960;
    const height = Math.max(1, Math.round((width * disp.size.height) / Math.max(1, disp.size.width)));
    const sources = await desktopCapturer.getSources({ types: ['screen'], thumbnailSize: { width, height } });
    const src = sources.find((x) => x.display_id === String(disp.id)) ?? sources[0];
    if (!src || src.thumbnail.isEmpty()) { s.snapshotsSkipped = true; return; }
    const small = new Uint8Array(src.thumbnail.resize({ width: 16, height: 9 }).toBitmap());
    const now = Date.now();
    const changed = !s.lastHash || hashDiff(s.lastHash, small) > 6;
    if (!changed && now - s.lastFrameAt < SNAPSHOT_FORCE_MS) return;
    s.lastHash = small;
    s.lastFrameAt = now;
    const id = `${s.sessionId}-f${++s.frameSeq}`;
    const jpeg = src.thumbnail.toJPEG(60);
    s.frames.set(id, jpeg);
    keptFrames.set(id, jpeg);
    s.frameOrder.push(id);
    while (s.frameOrder.length > MAX_FRAMES) {
      const old = s.frameOrder.shift()!;
      s.frames.delete(old);
      keptFrames.delete(old);
    }
    pushEvent({ kind: 'snapshot', id });
  } catch (err) {
    log.warn('[Teach] snapshot failed', err);
  } finally {
    s.snapshotBusy = false;
  }
}

// ─── Record bar window ─────────────────────────────────────────────────────

const BAR_HTML = `<!doctype html><html><head><meta charset="utf-8"><title>Allternit Teach</title>
<style>
:root{--bg:#ffffff;--fg:#1f1f1e;--mut:#6b6b69;--line:#e4e4e2;--red:#e5484d;--fill:#f2f2f0}
@media (prefers-color-scheme: dark){:root{--bg:#1f1f1e;--fg:#f4f4f2;--mut:#a0a09c;--line:#3a3a38;--fill:#2b2b2a}}
html,body{margin:0;height:100%;background:transparent;font:500 13px -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;color:var(--fg);user-select:none;overflow:hidden}
.bar{box-sizing:border-box;height:100%;display:flex;align-items:center;gap:10px;padding:0 8px 0 14px;background:var(--bg);border:1px solid var(--line);border-radius:22px;-webkit-app-region:drag}
.dot{width:10px;height:10px;border-radius:50%;background:var(--red);animation:p 1.2s ease-in-out infinite}
@keyframes p{0%,100%{opacity:1;transform:scale(1)}50%{opacity:.35;transform:scale(.8)}}
@media (prefers-reduced-motion: reduce){.dot{animation:none}}
.t{font-variant-numeric:tabular-nums;min-width:44px}
.sp{flex:1}
button{-webkit-app-region:no-drag;font:inherit;border:0;border-radius:16px;height:30px;padding:0 12px;cursor:pointer;background:var(--fill);color:var(--fg)}
button:focus-visible{outline:2px solid var(--fg);outline-offset:1px}
#mic{width:30px;padding:0;display:grid;place-items:center}
#mic[aria-pressed=true]{background:var(--fg);color:var(--bg)}
#stop{background:var(--red);color:#fff}
</style></head><body>
<div class="bar"><span class="dot"></span><span class="t" id="t">0:00</span><span class="sp"></span>
<button id="mic" aria-pressed="false" aria-label="Narration" title="Narration"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="9" y="3" width="6" height="12" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/></svg></button>
<button id="stop">Stop</button></div>
<script>
const P=${JSON.stringify(MSG_PREFIX)};
const say=(o)=>console.debug(P+JSON.stringify(o));
let start=Date.now(),timer=null;
const fmt=(ms)=>{const s=Math.floor(ms/1000);return Math.floor(s/60)+':'+String(s%60).padStart(2,'0')};
const tick=()=>{document.getElementById('t').textContent=fmt(Date.now()-start)};
let ctx=null,stream=null,proc=null,chunks=[],total=0;
function wav(){
  if(!total)return '';
  const pcm=new Int16Array(total);let o=0;
  for(const c of chunks){for(let i=0;i<c.length;i++){const v=Math.max(-1,Math.min(1,c[i]));pcm[o++]=v<0?v*0x8000:v*0x7fff}}
  chunks=[];total=0;
  const buf=new ArrayBuffer(44+pcm.length*2),dv=new DataView(buf);
  const w=(p,s)=>{for(let i=0;i<s.length;i++)dv.setUint8(p+i,s.charCodeAt(i))};
  w(0,'RIFF');dv.setUint32(4,36+pcm.length*2,true);w(8,'WAVE');w(12,'fmt ');dv.setUint32(16,16,true);dv.setUint16(20,1,true);dv.setUint16(22,1,true);
  dv.setUint32(24,16000,true);dv.setUint32(28,32000,true);dv.setUint16(32,2,true);dv.setUint16(34,16,true);w(36,'data');dv.setUint32(40,pcm.length*2,true);
  new Int16Array(buf,44).set(pcm);
  const u=new Uint8Array(buf);let s='';for(let i=0;i<u.length;i+=0x8000)s+=String.fromCharCode.apply(null,u.subarray(i,i+0x8000));
  return btoa(s);
}
async function setMic(on){
  const b=document.getElementById('mic');
  if(on&&!stream){
    try{
      stream=await navigator.mediaDevices.getUserMedia({audio:{channelCount:1,echoCancellation:true,noiseSuppression:true}});
      ctx=new AudioContext({sampleRate:16000});
      const src=ctx.createMediaStreamSource(stream);
      proc=ctx.createScriptProcessor(4096,1,1);
      proc.onaudioprocess=(e)=>{const d=e.inputBuffer.getChannelData(0);chunks.push(new Float32Array(d));total+=d.length};
      src.connect(proc);proc.connect(ctx.destination);
    }catch(err){stream=null;b.setAttribute('aria-pressed','false');say({k:'mic-error',message:String(err&&err.message||err)});return false}
  }
  if(!on&&stream){
    try{proc&&proc.disconnect()}catch(e){}
    stream.getTracks().forEach((t)=>t.stop());
    try{ctx&&ctx.close()}catch(e){}
    stream=null;ctx=null;proc=null;
  }
  b.setAttribute('aria-pressed',String(!!on));
  return true;
}
window.__teach={begin(at){start=at;tick();clearInterval(timer);timer=setInterval(tick,500)},setMic,flush:wav,end(){clearInterval(timer);return setMic(false)}};
document.getElementById('mic').onclick=()=>say({k:'mic',on:document.getElementById('mic').getAttribute('aria-pressed')!=='true'});
document.getElementById('stop').onclick=()=>say({k:'stop'});
</script></body></html>`;

function ensureBar(): BrowserWindow | null {
  if (barWindow && !barWindow.isDestroyed()) return barWindow;
  const isMac = process.platform === 'darwin';
  const wa = screen.getPrimaryDisplay().workArea;
  const width = 280;
  const height = 44;
  const win = new BrowserWindow({
    width,
    height,
    x: Math.round(wa.x + (wa.width - width) / 2),
    y: wa.y + 12,
    frame: false,
    transparent: true,
    resizable: false,
    movable: true,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    skipTaskbar: true,
    hasShadow: true,
    alwaysOnTop: true,
    focusable: true,
    show: false,
    backgroundColor: '#00000000',
    type: isMac ? 'panel' : undefined,
    title: 'Allternit Teach',
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      partition: 'teach-record-bar',
      backgroundThrottling: false,
    },
  });
  win.setAlwaysOnTop(true, isMac ? 'floating' : 'screen-saver');
  try { win.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true, skipTransformProcessType: true }); } catch { /* best effort */ }
  try { win.setContentProtection(true); } catch { /* best effort */ }

  const sess = session.fromPartition('teach-record-bar');
  sess.setPermissionRequestHandler((_wc, permission, cb) => cb(permission === 'media'));
  sess.setPermissionCheckHandler((_wc, permission) => permission === 'media');

  win.webContents.on('will-navigate', (e) => e.preventDefault());
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  win.webContents.on('console-message', (...args: unknown[]) => {
    const first = args[0] as { message?: string } | undefined;
    const message = typeof args[2] === 'string' ? (args[2] as string) : first?.message;
    if (typeof message !== 'string' || !message.startsWith(MSG_PREFIX)) return;
    void handleBarMessage(message.slice(MSG_PREFIX.length));
  });
  win.on('closed', () => { if (barWindow === win) barWindow = null; });

  // file:// is a secure context for getUserMedia; data: URLs are not.
  const file = path.join(app.getPath('userData'), 'teach-record-bar.html');
  try {
    fs.writeFileSync(file, BAR_HTML, 'utf8');
    void win.loadFile(file);
  } catch (err) {
    log.error('[Teach] could not write record bar page', err);
    win.destroy();
    return null;
  }
  barWindow = win;
  return win;
}

async function handleBarMessage(raw: string): Promise<void> {
  let m: { k?: string; on?: boolean; message?: string };
  try { m = JSON.parse(raw); } catch { return; }
  if (m.k === 'stop') {
    await finishFromUi();
  } else if (m.k === 'mic') {
    await setNarration(!!m.on);
  } else if (m.k === 'mic-error') {
    log.warn('[Teach] mic unavailable:', m.message);
    narrationOn = false;
    broadcastState();
  }
}

function barRun<T = unknown>(code: string): Promise<T | undefined> {
  if (!barWindow || barWindow.isDestroyed()) return Promise.resolve(undefined);
  return barWindow.webContents.executeJavaScript(code, true).catch(() => undefined) as Promise<T | undefined>;
}

function hideBar(): void {
  if (barWindow && !barWindow.isDestroyed()) {
    void barRun('window.__teach && window.__teach.end()');
    barWindow.destroy();
  }
  barWindow = null;
}

// ─── Narration ─────────────────────────────────────────────────────────────

async function flushNarration(s: ActiveSession): Promise<void> {
  const chunkT = s.chunkStartT;
  s.chunkStartT = Date.now() - s.startedAt;
  const b64 = await barRun<string>('window.__teach ? window.__teach.flush() : ""');
  if (!b64) return;
  const wav = Buffer.from(b64, 'base64');
  if (wav.length < MIN_WAV_BYTES) return;
  s.transcribeQueue = s.transcribeQueue.then(async () => {
    try {
      const res = await voiceManager.transcribe(wav);
      const text = res.text?.trim();
      if (text) {
        s.narrationParts.push(text);
        s.events.push({ t: chunkT, kind: 'narration', text });
      } else if (res.error || res.status) {
        log.info('[Teach] narration chunk skipped:', res.error ?? res.status);
      }
    } catch (err) {
      log.warn('[Teach] transcribe failed', err);
    }
  });
}

export async function setNarration(on: boolean): Promise<void> {
  if (!active) { narrationOn = false; return; }
  if (on && process.platform === 'darwin') {
    try { if (!(await systemPreferences.askForMediaAccess('microphone'))) on = false; } catch { on = false; }
  }
  if (!on && narrationOn) await flushNarration(active);
  if (on && !narrationOn) active.chunkStartT = Date.now() - active.startedAt;
  const ok = await barRun<boolean>(`window.__teach ? window.__teach.setMic(${on}) : false`);
  narrationOn = on && ok !== false;
  broadcastState();
}

// ─── Start / stop ──────────────────────────────────────────────────────────

/** `*.csv` / `invoices*.pdf` → a case-insensitive match on the whole file name. */
function globToRegExp(pattern: string): RegExp {
  const escaped = pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.');
  return new RegExp(`^${escaped}$`, 'i');
}

/**
 * A Teach run's `expect_download` step: wait for the next finished download
 * whose file name matches `pattern` in any session a page uses (the default
 * session and every webview's). Resolves with where it was saved.
 */
export function waitForDownload(pattern: string, timeoutMs: number): Promise<{ ok: boolean; fileName?: string; savePath?: string; error?: string }> {
  // Steps store a regex source (digit runs generalised); a plain glob like
  // `*.csv` also works.
  let re: RegExp;
  try {
    re = /[*?]/.test(pattern) && !/[\\^$]/.test(pattern) ? globToRegExp(pattern) : new RegExp(pattern || '.*', 'i');
  } catch {
    re = globToRegExp(pattern || '*');
  }
  const sessions = new Set<Electron.Session>([session.defaultSession]);
  for (const wc of webContents.getAllWebContents()) {
    try { if (!wc.isDestroyed()) sessions.add(wc.session); } catch { /* ignore */ }
  }
  return new Promise((resolve) => {
    const listeners: Array<[Electron.Session, (e: unknown, item: Electron.DownloadItem) => void]> = [];
    const finish = (result: { ok: boolean; fileName?: string; savePath?: string; error?: string }) => {
      clearTimeout(timer);
      for (const [ses, fn] of listeners) { try { ses.removeListener('will-download', fn as never); } catch { /* ignore */ } }
      resolve(result);
    };
    const timer = setTimeout(() => finish({ ok: false, error: `No download matching ${pattern} within ${Math.round(timeoutMs / 1000)}s.` }), timeoutMs);
    for (const ses of sessions) {
      const fn = (_e: unknown, item: Electron.DownloadItem) => {
        const fileName = item.getFilename();
        if (!re.test(fileName)) return;
        item.once('done', (_ev, state) => {
          if (state === 'completed') finish({ ok: true, fileName, savePath: item.getSavePath() });
          else finish({ ok: false, fileName, error: `The download of ${fileName} ${state}.` });
        });
      };
      ses.on('will-download', fn as never);
      listeners.push([ses, fn]);
    }
  });
}

export function isTeachAvailable(): boolean {
  return true;
}

export async function startTeach(opts?: { narration?: boolean; filterUrls?: string[] }): Promise<{ success: boolean; sessionId?: string; error?: string }> {
  if (active) return { success: false, error: 'A Teach recording is already running.' };
  let capture: { sessionId: string };
  try {
    capture = createCaptureSession({ filterUrls: opts?.filterUrls });
  } catch (err) {
    return { success: false, error: err instanceof Error ? err.message : 'Could not start capture' };
  }
  const startedAt = Date.now();
  const s: ActiveSession = {
    sessionId: `teach-${startedAt}-${Math.random().toString(36).slice(2, 6)}`,
    captureId: capture.sessionId,
    startedAt,
    events: [],
    frames: new Map(),
    frameOrder: [],
    frameSeq: 0,
    narrationParts: [],
    chunkStartT: 0,
    transcribeQueue: Promise.resolve(),
    snapshotsSkipped: false,
    lastHash: null,
    lastFrameAt: 0,
    lastApp: '',
    timers: [],
    child: null,
    guests: new Set(),
    downloadSessions: new Map(),
    unseenStart: new Set(),
    snapshotBusy: false,
    stopping: false,
    onCreated: (_e, wc) => {
      // Attach once the type is known (webviews report 'webview' right away).
      attachGuest(wc);
      wc.once('did-start-loading', () => attachGuest(wc));
    },
  };
  active = s;
  narrationOn = false;
  pendingFinished = null;
  keptFrames.clear();

  for (const wc of webContents.getAllWebContents()) attachGuest(wc);
  app.on('web-contents-created', s.onCreated);
  startAppWatcher(s);
  void takeSnapshot(s);
  s.timers.push(setInterval(() => { void takeSnapshot(s); }, SNAPSHOT_EVERY_MS));
  s.timers.push(setInterval(() => { if (narrationOn) void flushNarration(s); }, NARRATION_CHUNK_MS));

  const bar = ensureBar();
  if (bar) {
    const begin = () => { void barRun(`window.__teach && window.__teach.begin(${startedAt})`); bar.showInactive(); };
    if (bar.webContents.isLoading()) bar.webContents.once('did-finish-load', begin); else begin();
  }
  broadcastState();
  if (opts?.narration) {
    const go = () => { void setNarration(true); };
    if (bar && bar.webContents.isLoading()) bar.webContents.once('did-finish-load', () => setTimeout(go, 100)); else go();
  }
  log.info('[Teach] started', s.sessionId);
  return { success: true, sessionId: s.sessionId };
}

function teardown(s: ActiveSession): void {
  s.stopping = true;
  for (const t of s.timers) clearInterval(t);
  s.timers = [];
  app.removeListener('web-contents-created', s.onCreated);
  try { s.child?.kill(); } catch { /* ignore */ }
  s.child = null;
  s.guests.clear();
  for (const [ses, fn] of s.downloadSessions) { try { ses.removeListener('will-download', fn as never); } catch { /* ignore */ } }
  s.downloadSessions.clear();
}

export async function stopTeach(): Promise<{ success: boolean; recording?: TeachRecording; error?: string }> {
  const s = active;
  if (!s) return { success: false, error: 'No Teach recording is running.' };
  s.stopping = true; // stop appending new timeline events
  const endedAt = Date.now();
  teardown(s);
  if (narrationOn) {
    try { await Promise.race([flushNarration(s), new Promise((r) => setTimeout(r, 5000))]); } catch { /* keep going */ }
  }
  try { await Promise.race([s.transcribeQueue, new Promise((r) => setTimeout(r, 25_000))]); } catch { /* keep going */ }
  hideBar();
  active = null;
  narrationOn = false;

  const stopped = stopCaptureSession(s.captureId);
  const events = [...s.events].sort((a, b) => a.t - b.t);
  const recording: TeachRecording = {
    sessionId: s.sessionId,
    startedAt: s.startedAt,
    endedAt,
    har: stopped?.har ?? '',
    events,
    narration: s.narrationParts.join(' ').trim(),
    snapshotsSkipped: s.snapshotsSkipped || undefined,
  };
  broadcastState();
  log.info('[Teach] stopped', s.sessionId, 'events:', events.length);
  return { success: true, recording };
}

async function finishFromUi(): Promise<void> {
  const res = await stopTeach();
  if (!res.success || !res.recording) return;
  pendingFinished = res.recording;
  const w = deps?.getMainWindow();
  if (!w || w.isDestroyed()) return;
  if (w.isMinimized()) w.restore();
  w.show();
  w.focus();
  const send = () => {
    w.webContents.send('teach:finished', res.recording);
    void w.webContents.executeJavaScript(
      `window.dispatchEvent(new CustomEvent('allternit:open-view',{detail:{viewType:'site-apis'}}))`,
      true,
    ).catch(() => {});
  };
  if (w.webContents.isLoading()) w.webContents.once('did-finish-load', send); else send();
}

export async function toggleTeachFromHotkey(): Promise<void> {
  if (active) await finishFromUi();
  else await startTeach({ narration: false });
}

/** Stop and discard everything (app quit or main window closed mid-recording). */
export function abortTeach(): void {
  const s = active;
  if (!s) { hideBar(); return; }
  teardown(s);
  try { stopCaptureSession(s.captureId); } catch { /* ignore */ }
  hideBar();
  active = null;
  narrationOn = false;
  keptFrames.clear();
  log.info('[Teach] aborted', s.sessionId);
}

export function registerTeachHotkey(): void {
  try {
    if (globalShortcut.register(TEACH_HOTKEY, () => { void toggleTeachFromHotkey(); })) {
      log.info(`[Main] Registered global Teach hotkey: ${TEACH_HOTKEY}`);
    } else {
      log.warn(`[Main] Failed to register global hotkey: ${TEACH_HOTKEY}`);
    }
  } catch (err) {
    log.warn('[Main] Teach hotkey registration threw', err);
  }
}

export function installTeachRecorder(d: TeachDeps): void {
  deps = d;
  ipcMain.handle('teach:is-available', () => isTeachAvailable());
  ipcMain.handle('teach:start', (_e, opts?: { narration?: boolean; filterUrls?: string[] }) => startTeach(opts));
  ipcMain.handle('teach:stop', async () => {
    const res = await stopTeach();
    return res;
  });
  ipcMain.handle('teach:set-narration', (_e, on: boolean) => setNarration(!!on));
  ipcMain.handle('teach:get-snapshot', (_e, id: string) => {
    const buf = typeof id === 'string' ? keptFrames.get(id) : undefined;
    return buf ? `data:image/jpeg;base64,${buf.toString('base64')}` : null;
  });
  ipcMain.handle('teach:get-state', () => getState());
  ipcMain.handle('teach:take-pending', () => pendingFinished);
  ipcMain.handle('teach:wait-download', (_e, pattern: string, timeoutMs?: number) => waitForDownload(String(pattern ?? ''), Number(timeoutMs) || 60_000));
  ipcMain.on('teach:ack', (_e, sessionId: string) => {
    if (pendingFinished?.sessionId === sessionId) pendingFinished = null;
  });
  app.on('before-quit', abortTeach);
}
