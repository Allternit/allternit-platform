import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  status: 'denied',
  getSources: vi.fn(),
  windows: [] as any[],
}));
vi.mock('electron', () => ({
  desktopCapturer: { getSources: mocks.getSources },
  systemPreferences: { getMediaAccessStatus: () => mocks.status },
  ipcMain: { handle: vi.fn() },
}));
vi.mock('./window-below.js', () => ({
  enumerateWindowsFrontToBack: async () => mocks.windows,
  enumerationFailed: () => false,
}));

import { captureLastWindow } from './screen-context.js';

const thumb = (url: string): any => ({ isEmpty: () => false, getSize: () => ({ width: 100, height: 50 }), resize: () => thumb(url), toDataURL: () => url });

describe('captureLastWindow', () => {
  beforeEach(() => {
    mocks.getSources.mockReset();
    mocks.windows = [
      { app: 'Allternit', title: 'self', pid: process.pid, id: 1, bounds: { x: 0, y: 0, width: 100, height: 50 } },
      { app: 'Xcode', title: 'Build', pid: 99, id: 7, bounds: { x: 0, y: 0, width: 100, height: 50 } },
    ];
  });

  it.runIf(process.platform === 'darwin')('does not capture without Screen Recording permission', async () => {
    mocks.status = 'denied';
    expect(await captureLastWindow()).toEqual({ kind: 'unavailable', reason: 'screen_recording_not_granted' });
    expect(mocks.getSources).not.toHaveBeenCalled();
  });

  it('captures the first non-Allternit window', async () => {
    mocks.status = 'granted';
    mocks.getSources.mockResolvedValue([{ id: 'window:7:0', thumbnail: thumb('data:image/png;base64,AAAA') }]);
    expect(await captureLastWindow()).toEqual({ kind: 'window', app: 'Xcode', title: 'Build', image: 'data:image/png;base64,AAAA' });
  });
});
