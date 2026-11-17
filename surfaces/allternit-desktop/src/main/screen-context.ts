// screen-context.ts — one picture of the window the user was last using, for
// chat "screen context". Never prompts: macOS needs Screen Recording already
// granted, otherwise the answer is { kind: 'unavailable' }.

import { desktopCapturer, ipcMain, systemPreferences } from 'electron';

import { enumerateWindowsFrontToBack, enumerationFailed } from './window-below.js';

export type ScreenContextResult =
  | { kind: 'window'; app: string; title: string; text?: string; image: string }
  | { kind: 'unavailable'; reason: string };

const MAX_BYTES = 1_000_000;
const EDGES = [1600, 1280, 1024, 800, 640];

const unavailable = (reason: string): ScreenContextResult => ({ kind: 'unavailable', reason });

/** Data URL byte size, approximated from its base64 payload. */
const dataUrlBytes = (url: string): number => Math.floor(((url.length - url.indexOf(',') - 1) * 3) / 4);

export async function captureLastWindow(): Promise<ScreenContextResult> {
  if (process.platform === 'darwin' && systemPreferences.getMediaAccessStatus('screen') !== 'granted') {
    return unavailable('screen_recording_not_granted');
  }
  try {
    const windows = await enumerateWindowsFrontToBack(process.pid, true);
    if (enumerationFailed(windows)) return unavailable('window_enumeration_failed');
    // Front-to-back, so the first window that is not Allternit is the last one in use.
    const target = windows.find(w => w.pid !== process.pid && w.bounds.width > 0 && w.bounds.height > 0);
    if (!target) return unavailable('no_other_window');

    const longest = Math.max(target.bounds.width, target.bounds.height);
    const first = EDGES[0] / longest;
    const sources = await desktopCapturer.getSources({
      types: ['window'],
      thumbnailSize: { width: Math.round(target.bounds.width * first), height: Math.round(target.bounds.height * first) },
      fetchWindowIcons: false,
    });
    const source = sources.find(s => s.id.startsWith(`window:${target.id}:`));
    if (!source || source.thumbnail.isEmpty()) return unavailable('capture_failed');

    const size = source.thumbnail.getSize();
    for (const edge of EDGES) {
      const fit = Math.min(1, edge / Math.max(size.width, size.height));
      const image = fit < 1 ? source.thumbnail.resize({ width: Math.round(size.width * fit), quality: 'good' }) : source.thumbnail;
      const url = image.toDataURL();
      if (dataUrlBytes(url) <= MAX_BYTES) {
        return { kind: 'window', app: target.app || 'App', title: target.title, image: url };
      }
    }
    return unavailable('image_too_large');
  } catch {
    return unavailable('capture_failed');
  }
}

export function registerScreenContextIpc(): void {
  ipcMain.handle('screen-context:capture-window', () => captureLastWindow());
}
