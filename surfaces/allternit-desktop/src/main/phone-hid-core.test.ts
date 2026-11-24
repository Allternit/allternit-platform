import { describe, expect, it } from 'vitest';
import {
  BT_MAX_STEP,
  CORNER_REACH,
  HidCore,
  HidError,
  REHOME_EVERY,
  cornerSweep,
  hidRouteHealth,
  senderFromTransport,
  splitMove,
  type Calibration,
  type HidTransport,
} from './phone-hid-core.js';
import { MOD, charStroke, parseCombo, textStrokes } from './phone-hid-keys.js';
import { BoardError, UsbPointer, boardSender } from './phone-usb-agent-pointer.js';
import type { SendResult } from './phone-usb-agent-link.js';
import type { BoardCommand } from './phone-usb-agent-protocol.js';

const CAL: Calibration = { w: 1000, h: 2000, seen: { x: [], y: [] } };

/** A Bluetooth-style transport (the contract's §6 shape) that records what reaches the phone. */
function btTransport() {
  const log: string[] = [];
  const t: HidTransport = {
    status: async () => ({ paired: true, connected: true }),
    pointerMove: async (dx, dy) => void log.push(`move ${dx} ${dy}`),
    pointerClick: async (button, down = true, up = true) => void log.push(`click ${button} ${down ? 'd' : ''}${up ? 'u' : ''}`),
    pointerReset: async () => void log.push('reset'),
    key: async (usage, modifiers, down) => void log.push(`key ${usage.toString(16)} ${modifiers} ${down ? 'down' : 'up'}`),
    typeText: async (text) => void log.push(`type ${text}`),
  };
  return { t, log };
}

function btCore(cal: Calibration | null = CAL) {
  const { t, log } = btTransport();
  const waits: number[] = [];
  const core = new HidCore({
    sender: senderFromTransport('bt-android', t),
    screen: null,
    load: () => (cal ? structuredClone(cal) : null),
    save: () => undefined,
    sleep: async (ms) => void waits.push(ms),
  });
  return { core, log, waits };
}

const moves = (log: string[]) => log.filter((l) => l.startsWith('move ')).map((l) => l.split(' ').slice(1).map(Number) as [number, number]);

describe('step sizing and the corner', () => {
  it('splits a long move into steps the sender accepts that add up exactly, along a straight line', () => {
    const steps = splitMove(1000, -333, BT_MAX_STEP);
    expect(steps.length).toBe(Math.ceil(1000 / BT_MAX_STEP));
    for (const [dx, dy] of steps) {
      expect(Math.abs(dx)).toBeLessThanOrEqual(BT_MAX_STEP);
      expect(Math.abs(dy)).toBeLessThanOrEqual(BT_MAX_STEP);
    }
    expect(steps.reduce((s, [dx]) => s + dx, 0)).toBe(1000);
    expect(steps.reduce((s, [, dy]) => s + dy, 0)).toBe(-333);
    // Evenly spread: no step is more than one unit off the average.
    for (const [dx] of steps) expect(Math.abs(dx - 1000 / steps.length)).toBeLessThanOrEqual(1);
  });

  it('keeps a short or zero move as one step', () => {
    expect(splitMove(40, -7, BT_MAX_STEP)).toEqual([[40, -7]]);
    expect(splitMove(0, 0, BT_MAX_STEP)).toEqual([[0, 0]]);
  });

  it('a corner sweep drives further up and left than any screen we calibrate', () => {
    const sweep = cornerSweep(BT_MAX_STEP);
    expect(sweep.reduce((s, [dx]) => s + dx, 0)).toBe(-CORNER_REACH);
    expect(sweep.reduce((s, [, dy]) => s + dy, 0)).toBe(-CORNER_REACH);
    expect(sweep.every(([dx, dy]) => dx <= 0 && dy <= 0 && dx >= -BT_MAX_STEP && dy >= -BT_MAX_STEP)).toBe(true);
  });

  it('starts from the corner, sends a Bluetooth-sized path to the target, and re-homes every REHOME_EVERY gestures', async () => {
    const { core, log } = btCore();
    await core.point(0.5, 0.5);
    expect(log[0]).toBe('click left u'); // first use of a link lets go of everything
    expect(log[2]).toBe('reset');
    const path = moves(log);
    expect(path.every(([dx, dy]) => Math.abs(dx) <= BT_MAX_STEP && Math.abs(dy) <= BT_MAX_STEP)).toBe(true);
    expect(path.reduce((s, [dx]) => s + dx, 0)).toBe(500);
    expect(path.reduce((s, [, dy]) => s + dy, 0)).toBe(1000);

    for (let i = 1; i < REHOME_EVERY; i += 1) await core.point(i % 2 ? 0.25 : 0.5, 0.5);
    expect(log.filter((l) => l === 'reset').length).toBe(1);
    await core.point(0.75, 0.5);
    expect(log.filter((l) => l === 'reset').length).toBe(2);
  });

  it('reset lets go, drives to the corner, and the next gesture starts from there', async () => {
    const { core, log } = btCore();
    await core.reset();
    expect(log).toEqual(['click left u', 'key 0 0 up', 'reset']);
    expect(core.where()).toEqual([0, 0]);
  });

  it('a send that fails makes the core forget where the pointer is, so the next gesture re-homes', async () => {
    const { t, log } = btTransport();
    let fail = false;
    const core = new HidCore({
      sender: senderFromTransport('bt-android', {
        ...t,
        pointerMove: async (dx, dy) => {
          if (fail) throw new Error('link dropped');
          await t.pointerMove(dx, dy);
        },
      }),
      screen: null,
      load: () => structuredClone(CAL),
      save: () => undefined,
      sleep: async () => undefined,
    });
    await core.point(0.1, 0.1);
    fail = true;
    await expect(core.point(0.2, 0.2)).rejects.toThrow('link dropped');
    fail = false;
    const resetsBefore = log.filter((l) => l === 'reset').length;
    await core.point(0.3, 0.3);
    expect(log.filter((l) => l === 'reset').length).toBe(resetsBefore + 1);
  });

  it('refuses to aim before calibration', async () => {
    const { core, log } = btCore(null);
    await expect(core.tap(0.5, 0.5)).rejects.toMatchObject({ code: 'not_calibrated' });
    expect(log).toEqual([]);
  });
});

describe('keys', () => {
  it('parses combos into USB HID usages and modifier bits, with the board’s grammar', () => {
    expect(parseCombo('cmd+space')).toEqual({ combo: 'cmd+space', usage: 0x2c, modifiers: MOD.cmd });
    expect(parseCombo('cmd+h')).toEqual({ combo: 'cmd+h', usage: 0x0b, modifiers: MOD.cmd });
    expect(parseCombo('shift+tab')).toEqual({ combo: 'shift+tab', usage: 0x2b, modifiers: MOD.shift });
    expect(parseCombo('enter')).toMatchObject({ usage: 0x28, modifiers: 0 });
    expect(parseCombo('0')).toMatchObject({ usage: 0x27 });
    expect(parseCombo('cmd+rm -rf')).toBeNull();
    expect(parseCombo('a+b')).toBeNull();
    expect(parseCombo('cmd+shift+alt+ctrl+x')).toBeNull();
  });

  it('maps text onto the US layout and refuses what the layout can’t send', () => {
    expect(charStroke('A')).toMatchObject({ usage: 0x04, modifiers: MOD.shift });
    expect(charStroke('?')).toMatchObject({ usage: 0x38, modifiers: MOD.shift });
    expect(charStroke(' ')).toMatchObject({ usage: 0x2c, modifiers: 0 });
    expect(textStrokes('Hi!')?.map((s) => [s.usage, s.modifiers])).toEqual([
      [0x0b, MOD.shift],
      [0x0c, 0],
      [0x1e, MOD.shift],
    ]);
    expect(textStrokes('héllo')).toBeNull();
  });

  it('opens an app with Spotlight: ⌘Space, wait, the name, wait, Return', async () => {
    const { core, log, waits } = btCore();
    await core.openApp('Notes');
    expect(log).toEqual(['key 2c 8 down', 'key 2c 8 up', 'type Notes', 'key 28 0 down', 'key 28 0 up']);
    expect(waits).toEqual([600, 500]);
  });

  it('goes Home with ⌘H and moves through Full Keyboard Access with Tab and Space', async () => {
    const { core, log } = btCore();
    await core.pressHome();
    await core.fka('previous');
    await core.fka('activate');
    expect(log).toEqual(['key b 8 down', 'key b 8 up', 'key 2b 2 down', 'key 2b 2 up', 'key 2c 0 down', 'key 2c 0 up']);
  });

  it('refuses text and combos it can’t send before anything reaches the phone', async () => {
    const { core, log } = btCore();
    await expect(core.type('héllo')).rejects.toBeInstanceOf(HidError);
    await expect(core.key('cmd+rm -rf')).rejects.toMatchObject({ code: 'bad_params' });
    expect(log).toEqual([]);
  });
});

describe('the USB Agent board as a sender', () => {
  function recordingBoard(failOn?: BoardCommand['op']) {
    const sent: BoardCommand[] = [];
    const send = async (cmd: BoardCommand): Promise<SendResult> => {
      if (cmd.op === failOn) return { ok: false, error: 'offline', message: 'The USB Agent is not connected.' };
      sent.push(cmd);
      return { ok: true };
    };
    return { sent, send };
  }

  it('turns every sender call into the one USBA/1 command the board already speaks', async () => {
    const { sent, send } = recordingBoard();
    const s = boardSender({ send, linkGeneration: () => 3, connected: () => false });
    expect(s.id).toBe('usb-agent');
    expect(s.linkGeneration()).toBe(3);
    expect(await s.status()).toEqual({ paired: true, connected: false, name: 'USB Agent' });
    await s.pointerMove(12, -4);
    await s.pointerButton('click');
    await s.pointerButton('press');
    await s.pointerButton('release');
    await s.pointerReset();
    await s.scroll(3);
    await s.key(parseCombo('cmd+space')!);
    await s.typeText('hi');
    await s.releaseAll();
    expect(sent).toEqual([
      { op: 'move', dx: 12, dy: -4 },
      { op: 'click' },
      { op: 'press' },
      { op: 'release' },
      { op: 'home' },
      { op: 'scroll', v: 3, h: 0 },
      { op: 'key', combo: 'cmd+space' },
      { op: 'type', text: 'hi' },
      { op: 'release' },
    ]);
  });

  it('in Bluetooth mode the same sender sends the same commands, and reports pairing as the link state', async () => {
    const { sent, send } = recordingBoard();
    let bt: { paired: boolean; connected: boolean } | null = { paired: false, connected: false };
    const s = boardSender({ send, linkGeneration: () => 0, connected: () => true, bluetooth: () => bt });
    expect(await s.status()).toEqual({ paired: false, connected: false, name: 'USB Agent (Bluetooth)' });
    // Bonded but away: paired, not connected. Both read as BT_NOT_PAIRED in hidRouteHealth.
    bt = { paired: true, connected: false };
    const away = await s.status();
    expect(hidRouteHealth({ link: away, broadcast: true, pointerSeen: true, locked: false })).toMatchObject({ needsYou: { code: 'BT_NOT_PAIRED' } });
    bt = { paired: true, connected: true };
    expect(await s.status()).toEqual({ paired: true, connected: true, name: 'USB Agent (Bluetooth)' });
    // The board's Wi-Fi link down means the commands can't arrive, whatever Bluetooth says.
    const offline = boardSender({ send, linkGeneration: () => 0, connected: () => false, bluetooth: () => bt });
    expect((await offline.status()).connected).toBe(false);
    await s.pointerMove(5, 5);
    await s.scroll(-2);
    await s.typeText('ok');
    expect(sent).toEqual([{ op: 'move', dx: 5, dy: 5 }, { op: 'scroll', v: -2, h: 0 }, { op: 'type', text: 'ok' }]);
    // Cable mode (bluetooth() is null) is always paired.
    bt = null;
    expect(await s.status()).toEqual({ paired: true, connected: true, name: 'USB Agent' });
  });

  it('a failed board command throws BoardError with the link’s result', async () => {
    const { send } = recordingBoard('home');
    const s = boardSender({ send, linkGeneration: () => 0 });
    await expect(s.pointerReset()).rejects.toBeInstanceOf(BoardError);
    await expect(s.pointerReset()).rejects.toMatchObject({ result: { error: 'offline' } });
  });

  it('drives the board with the same commands as before the core moved out: Spotlight, Home swipe, tap', async () => {
    const { sent, send } = recordingBoard();
    const pointer = new UsbPointer({ send, screen: null, load: () => structuredClone(CAL), save: () => undefined, linkGeneration: () => 0, sleep: async () => undefined });
    await pointer.openApp('Notes');
    expect(sent.splice(0)).toEqual([
      { op: 'key', combo: 'cmd+space' },
      { op: 'type', text: 'Notes' },
      { op: 'key', combo: 'enter' },
    ]);
    await pointer.tap(0.5, 0.25);
    // A board move fits in one command, so the path is a single move.
    expect(sent.splice(0)).toEqual([{ op: 'release' }, { op: 'home' }, { op: 'move', dx: 500, dy: 500 }, { op: 'click' }]);
    await pointer.goHome();
    expect(sent.map((c) => c.op)).toEqual(['move', 'press', 'move', 'release']);
  });
});

describe('Bluetooth route health from the sender’s state', () => {
  const ok = { link: { paired: true, connected: true }, broadcast: true, pointerSeen: true, locked: false } as const;

  it('maps pairing, lock, broadcast and pointer to one state, most blocking first', () => {
    expect(hidRouteHealth({ ...ok, link: null })).toMatchObject({ state: 'unavailable' });
    expect(hidRouteHealth({ ...ok, link: { paired: false, connected: false }, broadcast: false })).toMatchObject({ state: 'needs_you', needsYou: { code: 'BT_NOT_PAIRED' } });
    expect(hidRouteHealth({ ...ok, link: { paired: true, connected: false } })).toMatchObject({ state: 'needs_you', needsYou: { code: 'BT_NOT_PAIRED' } });
    expect(hidRouteHealth({ ...ok, locked: true, broadcast: false })).toMatchObject({ state: 'unavailable' });
    expect(hidRouteHealth({ ...ok, broadcast: false, pointerSeen: false })).toMatchObject({ state: 'needs_you', needsYou: { code: 'BROADCAST_STOPPED' } });
    expect(hidRouteHealth({ ...ok, pointerSeen: false })).toMatchObject({ state: 'needs_you', needsYou: { code: 'ASSISTIVETOUCH_OFF' } });
    expect(hidRouteHealth({ ...ok, pointerSeen: null, locked: null })).toEqual({ state: 'healthy' });
  });

  it('says the user-facing sentence for BT_NOT_PAIRED', () => {
    const h = hidRouteHealth({ ...ok, link: { paired: false, connected: false } });
    expect(h.state === 'needs_you' && h.needsYou.message).toBe('Pair your iPhone with this Bluetooth keyboard and mouse in Settings > Bluetooth.');
  });
});

describe('a contract transport as a sender', () => {
  it('scroll is unsupported when the transport has none', async () => {
    const { t } = btTransport();
    await expect(senderFromTransport('bt-apple', t).scroll(2)).rejects.toMatchObject({ code: 'unsupported' });
  });
});
