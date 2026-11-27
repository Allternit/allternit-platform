import { app, dialog, ipcMain, Notification, type BrowserWindow, type IpcMainEvent, type IpcMainInvokeEvent } from 'electron';
import { execFile } from 'node:child_process';
import * as fs from 'node:fs';
import { join } from 'node:path';
import {
  attentionNotice,
  cleanText,
  parseAttentionInput,
  parseNeedsYou,
  resumeTaskMessage,
  safariProfileCardText,
  SAFARI_PROFILE_NAME,
  type AttentionInput,
} from './agent-attention.js';
import { createAgentPillWindow, type PillAction, type PillBounds } from './agent-pill-window.js';

export interface AgentAttentionOptions {
  /** The agent cursor window; only its main frame may call these channels. */
  cursorWindow: () => BrowserWindow | null;
  /** Bring the app forward on a run (`/computers/<id>?run=<run>`). */
  openRoute: (route: string | null) => void;
  /** Windows of the Safari the driver targets, front to back (screen points). */
  safariWindows: () => Promise<PillBounds[]>;
  /** Send a message to the Chrome extension over the native host. */
  sendToExtension: (message: unknown) => boolean;
  /** Tell every app window (renderer) something. */
  broadcast: (channel: string, payload: unknown) => void;
  /** Main-window senders allowed to answer an extension pause. */
  isAppSender: (event: IpcMainEvent | IpcMainInvokeEvent) => boolean;
  /** Folder for the one-time Safari profile flag. Defaults to userData. */
  stateDir?: () => string;
}

const validBounds = (b: unknown): b is PillBounds => {
  if (!b || typeof b !== 'object') return false;
  const r = b as Record<string, unknown>;
  return ['x', 'y', 'width', 'height'].every((k) => typeof r[k] === 'number' && Number.isFinite(r[k] as number)) && (r.width as number) > 0 && (r.height as number) > 0;
};

function osa(script: string): Promise<void> {
  return new Promise((resolve) => execFile('/usr/bin/osascript', ['-e', script], { timeout: 8000 }, () => resolve()));
}

/**
 * Everything the agent browser needs from main on this Mac besides the cursor
 * badge: the status pill on the agent's Safari window, the one-time Safari
 * profile card, native "needs you" notices, and the extension's `needs_you` /
 * `resume_task` pair. The cursor page owns the lease and the run, so button
 * clicks are handed back to it (`agent-pill:action`).
 */
export function installAgentAttention(options: AgentAttentionOptions) {
  const fromCursor = (event: IpcMainEvent | IpcMainInvokeEvent) => {
    const win = options.cursorWindow();
    return !!win && !win.isDestroyed() && event.sender === win.webContents && event.senderFrame === event.sender.mainFrame;
  };
  const pill = createAgentPillWindow((action: PillAction) => {
    const win = options.cursorWindow();
    if (win && !win.isDestroyed()) win.webContents.send('agent-pill:action', action);
  });

  function notify(input: AttentionInput) {
    const notice = attentionNotice(input);
    if (!Notification.isSupported()) return;
    const n = new Notification({ title: notice.title, body: notice.body, silent: false });
    n.on('click', () => options.openRoute(notice.route));
    n.show();
  }

  ipcMain.on('agent-pill:update', async (event, raw: unknown) => {
    if (!fromCursor(event) || !raw || typeof raw !== 'object') return;
    const r = raw as Record<string, unknown>;
    const botName = cleanText(r.botName) ?? 'Your bot';
    const step = typeof r.step === 'number' && Number.isFinite(r.step) ? r.step : undefined;
    const bounds = validBounds(r.bounds) ? r.bounds : (await options.safariWindows().catch(() => []))[0];
    if (!bounds) { pill.hide(); return; }
    pill.show({ botName, step }, bounds);
  });
  ipcMain.on('agent-pill:hide', (event) => { if (fromCursor(event)) pill.hide(); });

  ipcMain.on('agent-attention:notify', (event, raw: unknown) => {
    if (!fromCursor(event)) return;
    const input = parseAttentionInput(raw);
    if (input) notify(input);
  });

  // --- Safari "Allternit Agent" profile: one-time setup card -----------------
  const flagFile = () => join((options.stateDir ?? (() => app.getPath('userData')))(), 'agent-safari-profile.json');
  const profileDone = () => { try { return JSON.parse(fs.readFileSync(flagFile(), 'utf8')).done === true; } catch { return false; } };
  const markDone = () => { try { fs.writeFileSync(flagFile(), JSON.stringify({ done: true, at: new Date().toISOString() })); } catch { /* shown again next run */ } };

  ipcMain.handle('agent-safari-profile:ensure', async (event): Promise<'ready' | 'card-shown' | 'not-applicable'> => {
    if (!fromCursor(event)) throw new Error('Untrusted sender');
    if (process.platform !== 'darwin') return 'not-applicable';
    if (profileDone()) return 'ready';
    const card = safariProfileCardText();
    for (;;) {
      const { response } = await dialog.showMessageBox({
        type: 'info', title: 'Allternit', message: card.message, detail: card.detail,
        buttons: ['Open Safari Settings', `I added "${SAFARI_PROFILE_NAME}"`, 'Not now'], defaultId: 0, cancelId: 2,
      });
      if (response === 0) {
        await osa('tell application "Safari" to activate\ndelay 0.6\ntell application "System Events" to keystroke "," using command down');
        continue;
      }
      if (response === 1) { markDone(); return 'ready'; }
      return 'card-shown';
    }
  });

  // --- Chrome extension: needs_you out, resume_task back ---------------------
  const pending = new Map<string, { kind: AttentionInput['kind'] }>();
  ipcMain.handle('extension:resume-task', (event, handoffId: unknown, approved: unknown) => {
    if (!options.isAppSender(event)) throw new Error('Untrusted sender');
    const message = resumeTaskMessage(handoffId, approved);
    if (!message) return false;
    pending.delete(message.payload.handoffId);
    return options.sendToExtension(message);
  });

  return {
    /** Feed every message from the native host; returns true when it was a `needs_you`. */
    handleExtensionMessage(message: unknown): boolean {
      const needs = parseNeedsYou(message);
      if (!needs) return false;
      pending.set(needs.handoffId, { kind: needs.kind });
      options.broadcast('agent-attention:needs-you', needs);
      notify({ kind: needs.kind, domain: needs.domain, botName: needs.botName, handoffId: needs.handoffId });
      return true;
    },
    hidePill: () => pill.hide(),
    pillWindow: () => pill.window(),
    destroy: () => pill.destroy(),
  };
}
