/**
 * IPC for the user's own voice provider keys (Settings › Voice › Your own
 * provider). The web app keeps these in localStorage; on Desktop the renderer
 * swaps in a store backed by these channels (allternit-ai
 * src/lib/voice/byok/desktopStore.ts), so the keys sit in secure-store
 * (safeStorage in packaged builds, the local AES-GCM envelope in dev).
 *
 * Channels (all trusted-sender guarded by the caller's handleGuarded, and
 * additionally limited to a window's top frame):
 *   voiceKeys:get           -> the stored JSON string, or null
 *   voiceKeys:set (json)    -> stores it; must parse to a plain object, max 16 KB
 *   voiceKeys:clear         -> deletes it
 *   voiceKeys:readClipboard -> clipboard text, trimmed, or null when empty or
 *                              over 1 KB. Backs the Paste button: the renderer's
 *                              clipboard-read permission stays denied (security.ts).
 *
 * The keys only go to the vendor from the renderer; nothing here logs them.
 */

import { clipboard, type IpcMainInvokeEvent } from 'electron';
import { deleteSecret, readSecret, writeSecret } from './secure-store.js';

type Guarded = (channel: string, fn: (event: IpcMainInvokeEvent, ...args: any[]) => unknown) => void;

/** secure-store entry name. */
export const VOICE_KEYS_SECRET = 'voice-api-keys';
export const VOICE_KEYS_MAX_BYTES = 16 * 1024;
export const VOICE_CLIPBOARD_MAX_CHARS = 1024;

/** Only a window's own page, never an embedded frame (even a same-origin one). */
function assertTopFrame(event: IpcMainInvokeEvent, channel: string): void {
  const frame = event.senderFrame;
  if (!frame || frame !== event.sender?.mainFrame) {
    throw new Error(`IPC "${channel}" is only available to the app's top frame`);
  }
}

export function validateVoiceKeysJson(json: unknown): string {
  if (typeof json !== 'string') throw new Error('voice keys must be a JSON string');
  if (Buffer.byteLength(json, 'utf8') > VOICE_KEYS_MAX_BYTES) throw new Error('voice keys are too large');
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    throw new Error('voice keys are not valid JSON');
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('voice keys must be a JSON object');
  }
  return json;
}

export function readVoiceKeysClipboard(): string | null {
  const text = (clipboard.readText() ?? '').trim();
  if (!text || text.length > VOICE_CLIPBOARD_MAX_CHARS) return null;
  return text;
}

export function registerVoiceKeysIpc(handleGuarded: Guarded): void {
  const guarded = (channel: string, fn: (...args: any[]) => unknown) =>
    handleGuarded(channel, (event, ...args) => {
      assertTopFrame(event, channel);
      return fn(...args);
    });

  guarded('voiceKeys:get', () => readSecret(VOICE_KEYS_SECRET));
  guarded('voiceKeys:set', (json?: unknown) => {
    writeSecret(VOICE_KEYS_SECRET, validateVoiceKeysJson(json));
    return true;
  });
  guarded('voiceKeys:clear', () => {
    deleteSecret(VOICE_KEYS_SECRET);
    return true;
  });
  guarded('voiceKeys:readClipboard', () => readVoiceKeysClipboard());
}
