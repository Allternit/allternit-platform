import { describe, expect, it, vi } from 'vitest';
import { createIslandPillVisibility, islandPillBounds, sanitizeBarJobs } from './island-pill-window.js';

vi.mock('electron', () => ({
  app: {},
  BrowserWindow: class {},
  Menu: {},
  Notification: class {},
  Tray: class {},
  nativeImage: {},
  screen: {},
}));
vi.mock('electron-log', () => ({ default: { info: () => {}, warn: () => {}, error: () => {} } }));


const pill = { width: 360, height: 168 };

describe('islandPillBounds', () => {
  it('Mac with a notch: centred on the whole display, under the 38 pt menu bar', () => {
    const display = { bounds: { x: 0, y: 0, width: 1512, height: 982 }, workArea: { x: 0, y: 38, width: 1512, height: 919 } };
    expect(islandPillBounds('darwin', 'x11', display, pill)).toEqual({ x: 576, y: 44, width: 360, height: 168 });
  });

  it('Mac with an auto-hiding menu bar still clears the notch', () => {
    const display = { bounds: { x: 0, y: 0, width: 1512, height: 982 }, workArea: { x: 0, y: 0, width: 1512, height: 982 } };
    expect(islandPillBounds('darwin', 'x11', display, pill)?.y).toBe(44);
  });

  it('Mac second display to the left keeps its own origin', () => {
    const display = { bounds: { x: -1920, y: -200, width: 1920, height: 1080 }, workArea: { x: -1920, y: -175, width: 1920, height: 1055 } };
    expect(islandPillBounds('darwin', 'x11', display, pill)).toMatchObject({ x: -1140, y: -169 });
  });

  it('Windows with the taskbar on top: under the taskbar, centred on the work area', () => {
    const display = { bounds: { x: 0, y: 0, width: 1920, height: 1080 }, workArea: { x: 0, y: 48, width: 1920, height: 1032 } };
    expect(islandPillBounds('win32', 'x11', display, pill)).toEqual({ x: 780, y: 60, width: 360, height: 168 });
  });

  it('narrow display: the pill shrinks to fit with a 16 px gutter', () => {
    const display = { bounds: { x: 0, y: 0, width: 300, height: 600 }, workArea: { x: 0, y: 0, width: 300, height: 600 } };
    expect(islandPillBounds('win32', 'x11', display, pill)).toMatchObject({ x: 16, width: 268 });
  });

  it('Linux: a window on X11, the tray on native Wayland', () => {
    const display = { bounds: { x: 0, y: 0, width: 1920, height: 1080 }, workArea: { x: 0, y: 32, width: 1920, height: 1048 } };
    expect(islandPillBounds('linux', 'x11', display, pill)).toEqual({ x: 780, y: 42, width: 360, height: 168 });
    expect(islandPillBounds('linux', 'wayland', display, pill)).toBeNull();
  });
});

describe('createIslandPillVisibility', () => {
  function harness() {
    const changes: boolean[] = [];
    const timers: Array<{ fn: () => void; ms: number; cleared: boolean }> = [];
    const v = createIslandPillVisibility({
      onChange: (visible) => changes.push(visible),
      setTimer: (fn, ms) => { const t = { fn, ms, cleared: false }; timers.push(t); return t; },
      clearTimer: (t) => { (t as { cleared: boolean }).cleared = true; },
    });
    return { v, changes, timers };
  }

  it('shows while a job is live and hides 4 s after the receipt', () => {
    const { v, changes, timers } = harness();
    v.update(1);
    v.update(0);
    expect(changes).toEqual([true]);
    expect(timers[0]?.ms).toBe(4000);
    timers[0]!.fn();
    expect(changes).toEqual([true, false]);
  });

  it('a new job during the receipt keeps the pill up', () => {
    const { v, changes, timers } = harness();
    v.update(1);
    v.update(0);
    v.update(1);
    expect(timers[0]?.cleared).toBe(true);
    expect(changes).toEqual([true]);
    expect(v.visible()).toBe(true);
  });

  it('finished jobs alone never pop the pill', () => {
    const { v, changes, timers } = harness();
    v.update(0);
    expect(changes).toEqual([]);
    expect(timers).toHaveLength(0);
  });
});

describe('sanitizeBarJobs', () => {
  it('keeps known states, trims titles, drops junk', () => {
    expect(sanitizeBarJobs([
      { id: 'job_1', state: 'working', title: '  Refunding   €320 ' },
      { id: 'bad id!', state: 'working' },
      { id: 'job_2', state: 'exploded' },
      null,
    ])).toEqual([{ id: 'job_1', state: 'working', title: 'Refunding €320' }]);
    expect(sanitizeBarJobs('nope')).toEqual([]);
  });
});
