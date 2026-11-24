import { describe, expect, it } from 'vitest';
import { shouldWarnNoKeyring } from './keyring-notice.js';

describe('keyring warning decision', () => {
  it('warns only on linux basic_text, once', () => {
    expect(shouldWarnNoKeyring('linux', 'basic_text', false)).toBe(true);
    expect(shouldWarnNoKeyring('linux', 'basic_text', true)).toBe(false);
    expect(shouldWarnNoKeyring('linux', 'gnome_libsecret', false)).toBe(false);
    expect(shouldWarnNoKeyring('linux', 'kwallet5', false)).toBe(false);
    expect(shouldWarnNoKeyring('linux', undefined, false)).toBe(false);
    expect(shouldWarnNoKeyring('darwin', 'basic_text', false)).toBe(false);
    expect(shouldWarnNoKeyring('win32', 'basic_text', false)).toBe(false);
  });
});
