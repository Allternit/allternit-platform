import { describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({ app: { isPackaged: false, getPath: () => '/tmp' } }));
vi.mock('electron-log', () => ({ default: { info: vi.fn(), warn: vi.fn(), error: vi.fn() } }));

import { parseWindowsNetstatListeners } from './local-engine-manager.js';

describe('parseWindowsNetstatListeners', () => {
  it('returns the PIDs listening on the port and ignores other rows', () => {
    const out = [
      '',
      'Active Connections',
      '',
      '  Proto  Local Address          Foreign Address        State           PID',
      '  TCP    127.0.0.1:3015         0.0.0.0:0              LISTENING       4242',
      '  TCP    0.0.0.0:30150          0.0.0.0:0              LISTENING       7',
      '  TCP    127.0.0.1:51000        127.0.0.1:3015         ESTABLISHED     99',
      '  TCP    [::1]:3015             [::]:0                 LISTENING       4242',
    ].join('\r\n');
    expect(parseWindowsNetstatListeners(out, 3015)).toEqual([4242]);
  });
});
