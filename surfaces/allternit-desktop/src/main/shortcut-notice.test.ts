import { describe, expect, it, vi } from 'vitest';
import { createShortcutNotifier, shortcutNoticeBody } from './shortcut-notice.js';

describe('shortcut notice', () => {
  it('notifies once per session, only on failure, only on linux', () => {
    const notify = vi.fn();
    const note = createShortcutNotifier({ platform: 'linux', sessionType: 'x11', notify });
    expect(note('Ctrl+Shift+A', true)).toBe(false);
    expect(note('Ctrl+Shift+A', false)).toBe(true);
    expect(note('Ctrl+Shift+H', false)).toBe(false);
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify.mock.calls[0][1]).toContain('Ctrl+Shift+A');
  });
  it('never notifies on mac or windows', () => {
    for (const platform of ['darwin', 'win32']) {
      const notify = vi.fn();
      createShortcutNotifier({ platform, sessionType: undefined, notify })('X', false);
      expect(notify).not.toHaveBeenCalled();
    }
  });
  it('mentions desktop keyboard settings on Wayland', () => {
    expect(shortcutNoticeBody('Ctrl+K', 'wayland')).toContain("desktop's own keyboard settings");
    expect(shortcutNoticeBody('Ctrl+K', 'x11')).not.toContain('Wayland');
  });
});
