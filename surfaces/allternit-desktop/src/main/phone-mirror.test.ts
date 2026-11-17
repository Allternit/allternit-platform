import { describe, expect, it, vi } from 'vitest';
import { NEEDS_YOU_SENTENCES } from './phone-link-health.js';
import {
  bestLabelMatch,
  MirrorClient,
  mirrorBinaryPath,
  mirrorLane,
  MirrorUnavailable,
  onlyIphone,
  probeHealth,
  replyFailure,
  type MirrorProcess,
  type OcrElement,
} from './phone-mirror.js';
import { PhoneRouter, type PhoneLane } from './phone-route.js';

const SERIAL = 'app:iphone-1';

type Answer = Record<string, unknown> | 'hang' | 'exit';

/**
 * A fake allternit-mirror process (the edge: the native binary). `answer`
 * gets each request and returns the reply, 'hang' (never answers) or 'exit'
 * (the process dies).
 */
function fakeBinary(answer: (req: Record<string, unknown>) => Answer) {
  const spawned: Array<{ requests: Array<Record<string, unknown>>; closed: boolean; killed: boolean }> = [];
  const spawn = (): MirrorProcess => {
    const me = { requests: [] as Array<Record<string, unknown>>, closed: false, killed: false };
    spawned.push(me);
    let onLine: (l: string) => void = () => {};
    let onExit: (c: number | null) => void = () => {};
    return {
      write: (line) => {
        const req = JSON.parse(line) as Record<string, unknown>;
        me.requests.push(req);
        const a = answer(req);
        if (a === 'hang') return;
        if (a === 'exit') return void queueMicrotask(() => onExit(1));
        queueMicrotask(() => onLine(JSON.stringify({ ...a, id: req.id })));
      },
      onLine: (h) => void (onLine = h),
      onExit: (h) => void (onExit = h),
      close: () => {
        me.closed = true;
        queueMicrotask(() => onExit(0));
      },
      kill: () => {
        me.killed = true;
        queueMicrotask(() => onExit(null));
      },
    };
  };
  return { spawn, spawned };
}

const gray = (fill: number) => ({ width: 32, height: 64, base64: Buffer.alloc(32 * 64, fill).toString('base64') });

describe('probe → route health', () => {
  it('is healthy only when the probe itself says healthy', () => {
    expect(probeHealth({ ok: true, health: 'healthy' }, { locked: null })).toEqual({ state: 'healthy' });
    expect(probeHealth({ ok: true, health: 'healthy' }, { locked: true })).toEqual({ state: 'healthy' });
  });

  it('never assumes healthy: no answer, a failed probe or an unknown state are unavailable', () => {
    for (const probe of [null, { ok: false, message: 'boom' }, { ok: true }, { ok: true, health: 'SOMETHING_NEW' }]) {
      expect(probeHealth(probe, { locked: null }).state).toBe('unavailable');
    }
  });

  it('maps the contract states to their fixed needs-you sentence', () => {
    for (const code of ['MAC_LOCKED', 'MIRROR_WINDOW_HIDDEN', 'MIRROR_PHONE_IN_USE'] as const) {
      expect(probeHealth({ ok: true, health: code }, { locked: null })).toEqual({
        state: 'needs_you',
        needsYou: { code, message: NEEDS_YOU_SENTENCES[code] },
      });
    }
  });

  it('says the phone is in use when the router knows it is unlocked, even if the window looks fine', () => {
    expect(probeHealth({ ok: true, health: 'healthy' }, { locked: false })).toMatchObject({ state: 'needs_you', needsYou: { code: 'MIRROR_PHONE_IN_USE' } });
  });

  it('asks for a missing Mac permission with MAC_PERMISSION_MISSING, naming the System Settings pane', () => {
    const pane = 'System Settings > Privacy & Security > Screen & System Audio Recording';
    const h = probeHealth({ ok: true, health: 'NEEDS_SCREEN_RECORDING', needs_you: { permission: 'screen_recording', pane } }, { locked: null });
    expect(h).toEqual({
      state: 'needs_you',
      needsYou: { code: 'MAC_PERMISSION_MISSING', message: 'Allow Allternit in System Settings > Privacy & Security > Screen & System Audio Recording on your Mac.' },
    });
    const ax = probeHealth({ ok: true, health: 'NEEDS_ACCESSIBILITY' }, { locked: null });
    expect(ax).toMatchObject({ state: 'needs_you', needsYou: { code: 'MAC_PERMISSION_MISSING', message: expect.stringContaining('Privacy & Security > Accessibility on your Mac') } });
  });

  it('turns a permission_missing reply into MAC_PERMISSION_MISSING', () => {
    const r = replyFailure({ ok: false, error: 'permission_missing', health: 'NEEDS_ACCESSIBILITY', pane: 'System Settings > Privacy & Security > Accessibility', message: 'Allow Accessibility.' });
    expect(r).toMatchObject({ ok: false, error: 'needs_you', needs_you: { code: 'MAC_PERMISSION_MISSING', message: 'Allow Allternit in System Settings > Privacy & Security > Accessibility on your Mac.' } });
  });
});

describe('the allternit-mirror process', () => {
  it('keeps one process for many commands and matches replies by id', async () => {
    const bin = fakeBinary((req) => ({ ok: true, echo: req.cmd }));
    const client = new MirrorClient({ binary: () => '/bin/allternit-mirror', spawn: bin.spawn });
    const replies = await Promise.all([client.request('probe'), client.request('capture'), client.request('home')]);
    expect(replies.map((r) => r.echo)).toEqual(['probe', 'capture', 'home']);
    expect(bin.spawned).toHaveLength(1);
    expect(bin.spawned[0].requests.map((r) => r.cmd)).toEqual(['probe', 'capture', 'home']);
    client.close();
  });

  it('kills a process that runs past the time limit and starts a fresh one next time', async () => {
    let first = true;
    const bin = fakeBinary(() => {
      if (first) {
        first = false;
        return 'hang';
      }
      return { ok: true };
    });
    const client = new MirrorClient({ binary: () => '/bin/allternit-mirror', spawn: bin.spawn, timeoutMs: () => 20 });
    await expect(client.request('read')).rejects.toMatchObject({ code: 'timeout' });
    expect(bin.spawned[0].killed).toBe(true);
    await expect(client.request('probe')).resolves.toMatchObject({ ok: true });
    expect(bin.spawned).toHaveLength(2);
    client.close();
  });

  it('fails the waiting command when the process dies, then respawns', async () => {
    let n = 0;
    const bin = fakeBinary(() => (n++ === 0 ? 'exit' : { ok: true }));
    const client = new MirrorClient({ binary: () => '/bin/allternit-mirror', spawn: bin.spawn });
    await expect(client.request('tap', { x: 1, y: 1 })).rejects.toBeInstanceOf(MirrorUnavailable);
    expect(client.running).toBe(false);
    await expect(client.request('probe')).resolves.toMatchObject({ ok: true });
    expect(bin.spawned).toHaveLength(2);
    client.close();
  });

  it('closes stdin after a quiet minute so the process exits', async () => {
    vi.useFakeTimers();
    try {
      const bin = fakeBinary(() => ({ ok: true }));
      const client = new MirrorClient({ binary: () => '/bin/allternit-mirror', spawn: bin.spawn, idleMs: 60_000 });
      await client.request('probe');
      expect(bin.spawned[0].closed).toBe(false);
      await vi.advanceTimersByTimeAsync(59_000);
      expect(bin.spawned[0].closed).toBe(false);
      await vi.advanceTimersByTimeAsync(2_000);
      expect(bin.spawned[0].closed).toBe(true);
      expect(client.running).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it('says the build has no binary instead of spawning nothing', async () => {
    const client = new MirrorClient({ binary: () => null });
    await expect(client.request('probe')).rejects.toMatchObject({ code: 'no_binary' });
  });
});

describe('binary location', () => {
  it('ships in Resources/bin on a Mac and nowhere on Windows or Linux', () => {
    const all = () => true;
    expect(mirrorBinaryPath({ platform: 'darwin', packaged: true, resourcesPath: '/App/Contents/Resources', env: {}, exists: all })).toBe(
      '/App/Contents/Resources/bin/allternit-mirror',
    );
    expect(mirrorBinaryPath({ platform: 'win32', packaged: true, resourcesPath: 'C:/r', env: {}, exists: all })).toBeNull();
    expect(mirrorBinaryPath({ platform: 'linux', packaged: true, resourcesPath: '/r', env: {}, exists: all })).toBeNull();
    expect(mirrorBinaryPath({ platform: 'darwin', packaged: true, resourcesPath: '/r', env: {}, exists: () => false })).toBeNull();
  });
});

describe('find_and_tap on OCR text', () => {
  const el = (label: string, x = 0): OcrElement => ({ class: 'Text', label, id: '', bounds: [x, 0, x + 10, 10] });
  it('prefers an exact label, then one that contains the text, narrowed by match', () => {
    const els = [el('Send later', 0), el('Send', 20), el('Cancel', 40)];
    expect(bestLabelMatch(els, 'send')?.bounds[0]).toBe(20);
    expect(bestLabelMatch(els, 'canc')?.label).toBe('Cancel');
    expect(bestLabelMatch(els, 'send', 'later')?.label).toBe('Send later');
    expect(bestLabelMatch(els, 'delete')).toBeNull();
  });
});

describe('the mirror lane behind the router', () => {
  function routed(answer: (req: Record<string, unknown>) => Answer, extra: Partial<Record<'usb-agent', PhoneLane>> = {}) {
    const bin = fakeBinary(answer);
    const client = new MirrorClient({ binary: () => '/bin/allternit-mirror', spawn: bin.spawn });
    let t = 0;
    const router = new PhoneRouter({
      lanes: { mirror: mirrorLane({ client: () => client, platform: 'darwin', sleep: async () => {} }), ...extra },
      now: () => (t += 50),
      sleep: async () => {},
    });
    return { router, bin, client };
  }

  it('taps, sees the window change and reports verified', async () => {
    let captures = 0;
    const { router, bin, client } = routed((req) => {
      if (req.cmd === 'probe') return { ok: true, health: 'healthy' };
      if (req.cmd === 'capture') return { ok: true, gray: gray(captures++ === 0 ? 0 : 200) };
      return { ok: true };
    });
    const res = await router.run('phone.tap', SERIAL, { x: 100, y: 300 });
    expect(res).toMatchObject({ ok: true, route: 'mirror', verified: true, detail: { attempts: 1, changed: true } });
    expect(bin.spawned[0].requests.filter((r) => r.cmd === 'tap')).toEqual([expect.objectContaining({ x: 100, y: 300 })]);
    client.close();
  });

  it('a point outside the window is refused and never tried on another route', async () => {
    const usb: PhoneLane = {
      id: 'usb-agent',
      covers: ['screenshot', 'tap', 'type', 'swipe', 'key'],
      verify: 'self',
      health: async () => ({ state: 'healthy' }),
      act: async () => ({ ok: true, via: 'usb' }),
    };
    const { router, client } = routed((req) => {
      if (req.cmd === 'probe') return { ok: true, health: 'healthy' };
      if (req.cmd === 'capture') return { ok: true, gray: gray(0) };
      return { ok: false, error: 'outside_window', message: '(5000, 5000) is outside the iPhone Mirroring window (600×1300 pixels).' };
    }, { 'usb-agent': usb });
    const res = await router.run('phone.tap', SERIAL, { x: 5000, y: 5000 });
    expect(res).toMatchObject({ ok: false, route: 'mirror', error: 'invalid_args', verified: false });
    expect(res?.message).toContain('outside the iPhone Mirroring window');
    client.close();
  });

  it('a hidden window at send time is the contract needs-you, not a silent failure', async () => {
    const { router, client } = routed((req) => {
      if (req.cmd === 'probe') return { ok: true, health: 'healthy' };
      if (req.cmd === 'capture') return { ok: true, gray: gray(0) };
      return { ok: false, error: 'not_ready', health: 'MIRROR_WINDOW_HIDDEN', message: 'iPhone Mirroring has no window showing.' };
    });
    const res = await router.run('phone.home', SERIAL, {});
    expect(res).toMatchObject({ ok: false, route: 'mirror', needs_you: { code: 'MIRROR_WINDOW_HIDDEN', message: NEEDS_YOU_SENTENCES.MIRROR_WINDOW_HIDDEN } });
    client.close();
  });

  it('is unavailable on Windows and Linux without starting anything', async () => {
    const spawn = vi.fn();
    const client = new MirrorClient({ binary: () => '/x', spawn });
    for (const platform of ['win32', 'linux'] as const) {
      const lane = mirrorLane({ client: () => client, platform });
      expect(await lane.health({ serial: SERIAL, locked: true })).toEqual({ state: 'unavailable', reason: 'iPhone Mirroring works only on a Mac.' });
    }
    expect(spawn).not.toHaveBeenCalled();
  });
});

describe('which iPhone Mirroring shows', () => {
  it('does not count a second iPhone or iPad in keyboard mode as another iPhone', () => {
    const device = (serial: string, extra: Record<string, unknown> = {}) => ({ serial, platform: 'ios', via: 'allternit-app', route: 'iphone-app-store', name: serial, canTap: false, screen: true, note: '', ...extra });
    const apps = (list: Array<Record<string, unknown>>) => ({ devices: () => list }) as unknown as Parameters<typeof onlyIphone>[1]['apps'];
    expect(onlyIphone('app:main', { usb: null, links: null, apps: apps([device('app:main'), device('app:pad', { keyboard_for: 'app:main' })]) })).toBe(true);
    expect(onlyIphone('app:main', { usb: null, links: null, apps: apps([device('app:main'), device('app:other')]) })).toBe(false);
  });
});
