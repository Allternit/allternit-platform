import { describe, expect, it } from 'vitest';
import {
  createIslandHotkey,
  islandHotkeyLabel,
  toAccelerator,
  type IslandHotkeyDeps,
} from './island-hotkey.js';

/** A fake OS keyboard: `taken` keys belong to another app; `held` are ours. */
function fakeOs(opts: { taken?: string[]; saved?: string; platform?: string; sessionType?: string } = {}) {
  const taken = new Set(opts.taken ?? []);
  const held = new Set<string>();
  const notices: string[] = [];
  let saved = opts.saved;
  let noticeFor: string | undefined;
  const deps: IslandHotkeyDeps = {
    platform: opts.platform ?? 'darwin',
    sessionType: opts.sessionType,
    register: (accel) => {
      if (taken.has(accel) || held.has(accel)) return false;
      held.add(accel);
      return true;
    },
    unregister: (accel) => { held.delete(accel); },
    readSaved: () => saved,
    save: (v) => { saved = v; },
    readNoticeShownFor: () => noticeFor,
    saveNoticeShownFor: (v) => { noticeFor = v; },
    notify: (_title, body) => { notices.push(body); },
    onPress: () => {},
  };
  return { deps, held, taken, notices, saved: () => saved };
}

describe('island bar hotkey', () => {
  it('registers ⌥Space by default', () => {
    const os = fakeOs();
    const s = createIslandHotkey(os.deps).start();
    expect(s).toEqual({ shortcut: 'alt+space', active: 'alt+space', reason: 'ok' });
    expect([...os.held]).toEqual(['Alt+Space']);
    expect(os.notices).toEqual([]);
  });

  it('falls back to ⌥⇧Space when another app holds ⌥Space, and says so once', () => {
    const os = fakeOs({ taken: ['Alt+Space'] });
    const s = createIslandHotkey(os.deps).start();
    expect(s).toEqual({ shortcut: 'alt+space', active: 'alt+shift+space', reason: 'fallback' });
    expect([...os.held]).toEqual(['Alt+Shift+Space']);
    expect(os.notices).toEqual(['⌥Space is used by another app, so ⌥⇧Space opens the Island bar. Pick your shortcut in Settings > Shortcuts.']);

    // Next launch, same conflict: no second notice.
    os.held.clear();
    createIslandHotkey(os.deps).start();
    expect(os.notices).toHaveLength(1);
  });

  it('reports taken with nothing live when both keys are held elsewhere', () => {
    const os = fakeOs({ taken: ['Alt+Space', 'Alt+Shift+Space'], platform: 'win32' });
    const s = createIslandHotkey(os.deps).start();
    expect(s).toEqual({ shortcut: 'alt+space', active: null, reason: 'taken' });
    expect(os.notices[0]).toContain('Alt+Space is used by another app, so the Island bar has no shortcut.');
  });

  it('treats a register() that throws as taken', () => {
    const os = fakeOs();
    os.deps.register = () => { throw new Error('bad accelerator'); };
    expect(createIslandHotkey(os.deps).start().reason).toBe('taken');
  });

  it('uses the saved pick first, then the default when the pick is taken', () => {
    expect(createIslandHotkey(fakeOs({ saved: 'mod+shift+k' }).deps).start())
      .toEqual({ shortcut: 'mod+shift+k', active: 'mod+shift+k', reason: 'ok' });
    expect(createIslandHotkey(fakeOs({ saved: 'mod+shift+k', taken: ['CommandOrControl+Shift+K'] }).deps).start())
      .toEqual({ shortcut: 'mod+shift+k', active: 'alt+space', reason: 'fallback' });
  });

  it('keeps the old key live when the key picked in Settings is taken', () => {
    const os = fakeOs({ taken: ['CommandOrControl+K'] });
    const hk = createIslandHotkey(os.deps);
    hk.start();
    const s = hk.set('mod+k');
    expect(s).toEqual({ shortcut: 'alt+space', active: 'alt+space', reason: 'ok', rejected: { shortcut: 'mod+k', why: 'taken' } });
    expect([...os.held]).toEqual(['Alt+Space']);
    expect(os.saved()).toBeUndefined();
  });

  it('swaps to a free key picked in Settings and saves it; null restores the default', () => {
    const os = fakeOs({ taken: ['Alt+Space'] });
    const hk = createIslandHotkey(os.deps);
    hk.start();
    expect(hk.set('ctrl+alt+b')).toEqual({ shortcut: 'ctrl+alt+b', active: 'ctrl+alt+b', reason: 'ok' });
    expect([...os.held]).toEqual(['Control+Alt+B']);
    expect(os.saved()).toBe('ctrl+alt+b');

    os.taken.delete('Alt+Space');
    expect(hk.set(null)).toEqual({ shortcut: 'alt+space', active: 'alt+space', reason: 'ok' });
    expect(os.saved()).toBeUndefined();
  });

  it('rejects a key with no modifier', () => {
    const hk = createIslandHotkey(fakeOs().deps);
    hk.start();
    expect(hk.set('shift+k').rejected).toEqual({ shortcut: 'shift+k', why: 'invalid' });
    expect(hk.set('k').rejected).toEqual({ shortcut: 'k', why: 'invalid' });
  });

  it('releases the key while Settings records, and takes it back after', () => {
    const os = fakeOs();
    const hk = createIslandHotkey(os.deps);
    hk.start();
    hk.suspend(true);
    expect(os.held.size).toBe(0);
    // A pick made while recording is checked, then released until resume.
    expect(hk.set('mod+j').active).toBe('mod+j');
    expect(os.held.size).toBe(0);
    hk.suspend(false);
    expect([...os.held]).toEqual(['CommandOrControl+J']);
  });

  it('holds nothing while the bar is off, and lets go when it turns off', () => {
    const os = fakeOs();
    const hk = createIslandHotkey(os.deps);
    expect(hk.status()).toEqual({ shortcut: 'alt+space', active: null, reason: 'off' });
    expect(hk.set('mod+j').reason).toBe('off');
    expect(os.held.size).toBe(0);
    hk.start();
    expect(hk.stop()).toEqual({ shortcut: 'alt+space', active: null, reason: 'off' });
    expect(os.held.size).toBe(0);
  });

  it('says wayland on Linux Wayland', () => {
    expect(createIslandHotkey(fakeOs({ platform: 'linux', sessionType: 'wayland' }).deps).start().reason).toBe('wayland');
  });
});

describe('recorder spelling → Electron accelerator', () => {
  it('maps modifiers and keys', () => {
    expect(toAccelerator('alt+space')).toBe('Alt+Space');
    expect(toAccelerator('mod+shift+k')).toBe('CommandOrControl+Shift+K');
    expect(toAccelerator('ctrl+alt+f12')).toBe('Control+Alt+F12');
    expect(toAccelerator('meta+,')).toBe('Super+,');
    expect(toAccelerator('alt+capslock')).toBeNull();
  });

  it('labels per platform', () => {
    expect(islandHotkeyLabel('alt+shift+space', 'darwin')).toBe('⌥⇧Space');
    expect(islandHotkeyLabel('alt+shift+space', 'linux')).toBe('Alt+Shift+Space');
  });
});
