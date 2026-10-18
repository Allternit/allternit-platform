/**
 * The hidden window that carries call audio for the call bridge
 * (phone-call-bridge.ts, CONTRACT-linked-phone-voice C5).
 *
 * One window, created on first need and kept while Desktop runs: it lists the
 * Mac's audio devices, joins a LiveKit room per live call as the caller
 * (livekit-client, src/renderer/call-bridge/bridge.ts), publishes the cable's
 * input with echo cancellation, noise suppression and gain control off, and
 * plays the bot into the cable's output with setSinkId.
 *
 * Isolation: its own in-memory partition, and a permission handler on that
 * partition that grants microphone/speaker access to this window only. It
 * never navigates and opens no windows. Main ⇄ window messages go over the
 * window's own IPC scope (webContents.ipc), so no other renderer can drive it.
 */

import { BrowserWindow, session as electronSession, systemPreferences } from 'electron';
import { randomUUID } from 'node:crypto';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { MIC_PROBLEM_PREFIX, type CallMedia, type CallMediaJoin } from './phone-call-bridge.js';
import type { CallBridgeConfig } from './phone-call-bridge-config.js';

const here = dirname(fileURLToPath(import.meta.url));
const PARTITION = 'call-bridge';
const COMMAND_TIMEOUT_MS = 20_000;
const LOAD_TIMEOUT_MS = 15_000;

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout> };

export interface CallBridgeWindowOptions {
  /** The room went away on its own (hangup deleted it, the bot left, network). */
  onDisconnected: (serial: string, reason: string) => void;
  log?: (message: string) => void;
}

export class CallBridgeWindow implements CallMedia {
  private win: BrowserWindow | null = null;
  private ready: Promise<BrowserWindow> | null = null;
  private readonly pending = new Map<string, Pending>();

  constructor(private readonly opts: CallBridgeWindowOptions) {}

  /** macOS microphone access for the app; asks once while the person is in the settings sheet. */
  static async ensureMicAccess(ask: boolean): Promise<boolean> {
    if (process.platform !== 'darwin') return true;
    const status = systemPreferences.getMediaAccessStatus('microphone');
    if (status === 'granted') return true;
    if (status === 'not-determined' && ask) return systemPreferences.askForMediaAccess('microphone');
    return false;
  }

  async checkDevices(cfg: CallBridgeConfig): Promise<{ input: boolean; output: boolean; problem?: string }> {
    if (!(await CallBridgeWindow.ensureMicAccess(false))) {
      return { input: false, output: false, problem: `${MIC_PROBLEM_PREFIX} in System Settings › Privacy & Security › Microphone` };
    }
    return (await this.command('devices', cfg)) as { input: boolean; output: boolean };
  }

  async join(serial: string, join: CallMediaJoin): Promise<void> {
    if (!(await CallBridgeWindow.ensureMicAccess(false))) throw new Error(`${MIC_PROBLEM_PREFIX} in System Settings`);
    await this.command('join', { serial, ...join }, 30_000);
  }

  async leave(serial: string): Promise<void> {
    if (!this.win || this.win.isDestroyed()) return;
    await this.command('leave', { serial }).catch(() => {});
  }

  destroy(): void {
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.reject(new Error('The call audio window closed.'));
    }
    this.pending.clear();
    if (this.win && !this.win.isDestroyed()) this.win.destroy();
    this.win = null;
    this.ready = null;
  }

  private async command(op: 'devices' | 'join' | 'leave', args: unknown, timeoutMs = COMMAND_TIMEOUT_MS): Promise<unknown> {
    const win = await this.ensureWindow();
    const id = randomUUID();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`The call audio window did not answer (${op}).`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      win.webContents.send('call-bridge:cmd', { id, op, args });
    });
  }

  private ensureWindow(): Promise<BrowserWindow> {
    if (this.win && !this.win.isDestroyed() && this.ready) return this.ready;
    this.ready = this.createWindow();
    this.ready.catch(() => {
      this.ready = null;
    });
    return this.ready;
  }

  private async createWindow(): Promise<BrowserWindow> {
    const ses = electronSession.fromPartition(PARTITION);
    const win = new BrowserWindow({
      show: false,
      width: 320,
      height: 200,
      skipTaskbar: true,
      webPreferences: {
        preload: join(here, '../preload/call-bridge.js'),
        partition: PARTITION,
        sandbox: true,
        contextIsolation: true,
        nodeIntegration: false,
        backgroundThrottling: false,
        autoplayPolicy: 'no-user-gesture-required',
      },
    });
    this.win = win;
    const mine = (wc: Electron.WebContents | null | undefined) => !!wc && !win.isDestroyed() && wc.id === win.webContents.id;
    // Microphone (the cable's input) and speaker selection (setSinkId) for this window only.
    // 'speaker-selection' isn't in Electron's typings on every version; compare as a string.
    const allowed = (permission: string) => permission === 'media' || permission === 'speaker-selection';
    ses.setPermissionRequestHandler((wc, permission, callback) => callback(mine(wc) && allowed(permission)));
    ses.setPermissionCheckHandler((wc, permission) => mine(wc) && allowed(permission));
    win.webContents.on('will-navigate', (event) => event.preventDefault());
    win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));

    win.webContents.ipc.on('call-bridge:reply', (_e, msg: { id?: string; ok?: boolean; value?: unknown; error?: string }) => {
      const p = msg?.id ? this.pending.get(msg.id) : undefined;
      if (!p || !msg.id) return;
      this.pending.delete(msg.id);
      clearTimeout(p.timer);
      if (msg.ok) p.resolve(msg.value);
      else p.reject(new Error(String(msg.error ?? 'The call audio window failed.')));
    });
    win.webContents.ipc.on('call-bridge:event', (_e, msg: { type?: string; serial?: string; reason?: string; message?: string }) => {
      if (msg?.type === 'disconnected' && typeof msg.serial === 'string') this.opts.onDisconnected(msg.serial, String(msg.reason ?? 'disconnected'));
      else if (msg?.type === 'log' && typeof msg.message === 'string') this.opts.log?.(`[CallBridgeWindow] ${msg.message.slice(0, 500)}`);
    });
    win.on('closed', () => {
      if (this.win === win) {
        this.win = null;
        this.ready = null;
      }
    });
    // A crashed renderer drops every room: the bridge ends those carrier calls.
    win.webContents.on('render-process-gone', (_e, details) => {
      this.opts.log?.(`[CallBridgeWindow] renderer gone (${details.reason})`);
      this.destroy();
      this.opts.onDisconnected('*', `renderer ${details.reason}`);
    });

    const ready = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('The call audio window did not load.')), LOAD_TIMEOUT_MS);
      win.webContents.ipc.once('call-bridge:ready', () => {
        clearTimeout(timer);
        resolve();
      });
    });
    await win.loadFile(join(here, '../renderer/call-bridge/index.html'));
    await ready;
    return win;
  }
}
