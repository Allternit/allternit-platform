import { mkdtempSync, readFileSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const env = vi.hoisted(() => ({ clipboardText: '', userData: '' }));

vi.mock('electron', () => ({
  app: { getPath: () => env.userData, isPackaged: false },
  safeStorage: { isEncryptionAvailable: () => false },
  clipboard: { readText: () => env.clipboardText },
}));

import { registerVoiceKeysIpc } from './voice-keys-ipc.js';

type Handler = (event: any, ...args: unknown[]) => unknown;

function setup() {
  const handlers = new Map<string, Handler>();
  registerVoiceKeysIpc((channel, fn) => handlers.set(channel, fn as Handler));
  const mainFrame = { url: 'allternit://app/' };
  const topEvent = { sender: { mainFrame }, senderFrame: mainFrame };
  const call = (channel: string, ...args: unknown[]) => handlers.get(channel)!(topEvent, ...args);
  return { handlers, call, mainFrame };
}

const KEYS = JSON.stringify({ openai: 'sk-test-123', elevenlabsVoiceId: 'v1' });

describe('voice keys IPC', () => {
  beforeEach(() => {
    env.userData = mkdtempSync(join(tmpdir(), 'voice-keys-ipc-'));
    env.clipboardText = '';
  });

  it('stores keys encrypted at rest and reads them back after a re-register (restart)', () => {
    const first = setup();
    expect(first.call('voiceKeys:get')).toBeNull();
    expect(first.call('voiceKeys:set', KEYS)).toBe(true);

    const files = readdirSync(join(env.userData, 'secrets'));
    expect(files).toEqual(['voice-api-keys.bin']);
    expect(readFileSync(join(env.userData, 'secrets', files[0]), 'utf8')).not.toContain('sk-test-123');

    const second = setup();
    expect(second.call('voiceKeys:get')).toBe(KEYS);
    second.call('voiceKeys:clear');
    expect(second.call('voiceKeys:get')).toBeNull();
  });

  it('rejects writes that are not a JSON object or are too large, keeping the old value', () => {
    const { call } = setup();
    call('voiceKeys:set', KEYS);
    for (const bad of ['not json', '[1,2]', 'null', '"str"', 42, JSON.stringify({ openai: 'x'.repeat(17 * 1024) })]) {
      expect(() => call('voiceKeys:set', bad)).toThrow();
    }
    expect(call('voiceKeys:get')).toBe(KEYS);
  });

  it('refuses embedded frames, even same-origin ones', () => {
    const { handlers, mainFrame } = setup();
    const iframe = { url: 'allternit://app/embed' };
    const event = { sender: { mainFrame }, senderFrame: iframe };
    for (const channel of ['voiceKeys:get', 'voiceKeys:set', 'voiceKeys:clear', 'voiceKeys:readClipboard']) {
      expect(() => handlers.get(channel)!(event, KEYS)).toThrow(/top frame/);
    }
  });

  it('reads the clipboard trimmed, and returns null when empty or over 1 KB', () => {
    const { call } = setup();
    env.clipboardText = '  sk-abc\n';
    expect(call('voiceKeys:readClipboard')).toBe('sk-abc');
    env.clipboardText = '   ';
    expect(call('voiceKeys:readClipboard')).toBeNull();
    env.clipboardText = 'a'.repeat(1025);
    expect(call('voiceKeys:readClipboard')).toBeNull();
  });
});
