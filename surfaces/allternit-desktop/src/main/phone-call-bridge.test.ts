import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  androidCarrierLine,
  CallBridge,
  CALL_BRIDGE_POLL_MS,
  END_RETRY_MS,
  iosCarrierLine,
  ONE_RING_MS,
  parseIosCallScreen,
  requestLinkedCallRoom,
  type AndroidCallLane,
  type CallBridgeEvent,
  type CallBridgePhone,
  type CarrierLine,
  type CarrierRead,
  type LinkedCallRequest,
} from './phone-call-bridge.js';
import type { CallBridgeConfig } from './phone-call-bridge-config.js';

const CFG: CallBridgeConfig = {
  enabled: true,
  botId: 'bot_1',
  inputDeviceId: 'in-1',
  inputLabel: 'USB Audio Device',
  outputDeviceId: 'out-1',
  outputLabel: 'USB Audio Device',
};

const MOTO: CallBridgePhone = { serial: '192.168.1.20:37000', deviceId: 'ZY22ABC', label: 'Moto', platform: 'android', online: true, config: CFG };

function fakeCarrier() {
  const line: { state: CarrierRead['state']; number?: string; answerWorks: boolean; reads: number; unreadable: boolean } = {
    state: 'idle',
    answerWorks: true,
    reads: 0,
    unreadable: false,
  };
  const calls: string[] = [];
  const carrier: CarrierLine = {
    read: async () => {
      line.reads += 1;
      if (line.unreadable) return null;
      return line.number ? { state: line.state, number: line.number } : { state: line.state };
    },
    answer: async () => {
      calls.push('answer');
      if (!line.answerWorks) return false;
      line.state = 'offhook';
      return true;
    },
    end: async () => {
      calls.push('end');
      line.state = 'idle';
      return true;
    },
  };
  return { line, calls, carrier };
}

function setup(opts: { phone?: CallBridgePhone; cable?: { input: boolean; output: boolean }; cloudFails?: boolean; joinFails?: boolean } = {}) {
  let phone = opts.phone ?? MOTO;
  const { line, calls, carrier } = fakeCarrier();
  const rooms: LinkedCallRequest[] = [];
  const media = { joins: [] as string[], leaves: [] as string[], checks: 0, cable: opts.cable ?? { input: true, output: true } };
  const bridge = new CallBridge({
    phones: () => [phone],
    carrier: { android: carrier, ios: carrier },
    cloud: {
      createRoom: async (req) => {
        rooms.push(req);
        if (opts.cloudFails) throw new Error('voice is not on your plan');
        return { room: 'call-lp-1', livekitUrl: 'wss://lk.example', token: 'tok', identity: `linked-phone-${req.line.deviceId}` };
      },
    },
    media: {
      checkDevices: async () => {
        media.checks += 1;
        return media.cable;
      },
      join: async (serial) => {
        if (opts.joinFails) throw new Error('getUserMedia failed');
        media.joins.push(serial);
      },
      leave: async (serial) => {
        media.leaves.push(serial);
      },
    },
  });
  const events: CallBridgeEvent[] = [];
  bridge.on('event', (e: CallBridgeEvent) => events.push(e));
  const states = () => events.filter((e) => e.type === 'status').map((e) => (e.type === 'status' ? e.status.state : ''));
  const state = () => bridge.status()[0].state;
  return { bridge, line, calls, rooms, media, events, states, state, setPhone: (p: CallBridgePhone) => (phone = p) };
}

const step = (ms = CALL_BRIDGE_POLL_MS) => vi.advanceTimersByTimeAsync(ms);

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe('watcher: inbound', () => {
  it('rings, waits one ring, answers, joins the room, then leaves when the carrier call ends', async () => {
    const t = setup();
    t.bridge.start();
    await step();
    expect(t.state()).toBe('idle');

    t.line.state = 'ringing';
    t.line.number = '+15559876543';
    await step();
    expect(t.state()).toBe('ringing');
    expect(t.calls).toEqual([]);

    await step(ONE_RING_MS - 1_000);
    expect(t.calls).toEqual([]); // still inside the first ring
    await step(1_000);
    expect(t.calls).toEqual(['answer']);
    expect(t.state()).toBe('live');
    expect(t.rooms).toEqual([
      {
        botId: 'bot_1',
        line: { kind: 'linked_phone', deviceId: 'ZY22ABC', serial: MOTO.serial, label: 'Moto', platform: 'android' },
        direction: 'inbound',
        from: '+15559876543',
        to: null,
      },
    ]);
    expect(t.media.joins).toEqual([MOTO.serial]);
    expect(t.bridge.status()[0]).toMatchObject({ room: 'call-lp-1', direction: 'inbound', remoteNumber: '+15559876543', platformSupport: 'full' });

    t.line.state = 'idle';
    t.line.number = undefined;
    await step();
    expect(t.media.leaves).toEqual([MOTO.serial]);
    expect(t.state()).toBe('idle');
    expect(t.states()).toEqual(['ringing', 'answering', 'live', 'ending', 'idle']);
    t.bridge.stop();
  });

  it('does nothing when the call stops ringing before one ring', async () => {
    const t = setup();
    t.bridge.start();
    t.line.state = 'ringing';
    await step();
    t.line.state = 'idle';
    await step();
    expect(t.state()).toBe('idle');
    await step(ONE_RING_MS);
    expect(t.calls).toEqual([]);
    expect(t.rooms).toEqual([]);
    t.bridge.stop();
  });

  it('leaves a call answered by hand alone until the line is idle again', async () => {
    const t = setup();
    t.bridge.start();
    t.line.state = 'ringing';
    await step();
    t.line.state = 'offhook';
    await step(ONE_RING_MS + 2_000);
    expect(t.calls).toEqual([]);
    expect(t.rooms).toEqual([]);
    expect(t.state()).toBe('idle');
    // Next call is ours again.
    t.line.state = 'idle';
    await step();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.calls).toEqual(['answer']);
    t.bridge.stop();
  });

  it('withheld caller: from is null', async () => {
    const t = setup();
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.rooms[0]).toMatchObject({ from: null, direction: 'inbound' });
    t.bridge.stop();
  });

  it('ends the carrier call when the room disconnects (hangup from the UI)', async () => {
    const t = setup();
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.state()).toBe('live');
    t.bridge.mediaDisconnected(MOTO.serial, 'room deleted');
    expect(t.state()).toBe('ending');
    await step(0);
    expect(t.calls).toEqual(['answer', 'end']);
    await step();
    expect(t.state()).toBe('idle');
    t.bridge.stop();
  });

  it('presses End once more when the carrier call does not end', async () => {
    const t = setup();
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    t.bridge.mediaDisconnected(MOTO.serial, 'bot left');
    await step(0);
    t.line.state = 'offhook';
    await step(END_RETRY_MS + 1_000);
    expect(t.calls.filter((c) => c === 'end')).toHaveLength(2);
    t.bridge.stop();
  });

  it('a caller hanging up while the room is being set up leaves the room', async () => {
    const t = setup();
    let release: () => void = () => {};
    const slow = new Promise<void>((r) => (release = r));
    const media = (t.bridge as unknown as { deps: { media: { join: (s: string) => Promise<void> } } }).deps.media;
    media.join = async (serial) => {
      await slow;
      t.media.joins.push(serial);
    };
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.state()).toBe('answering');
    t.line.state = 'idle';
    await step();
    release();
    await step(0);
    expect(t.media.leaves.length).toBeGreaterThan(0);
    expect(t.state()).toBe('idle');
    t.bridge.stop();
  });
});

describe('watcher: refusals', () => {
  it('missing cable: does not answer, raises the needs-you notice once', async () => {
    const t = setup({ cable: { input: true, output: false } });
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.calls).toEqual([]);
    expect(t.rooms).toEqual([]);
    expect(t.bridge.status()[0]).toMatchObject({ state: 'error', error: 'Call audio cable not found for Moto' });
    await step(3_000);
    const notices = t.events.filter((e) => e.type === 'notice');
    expect(notices).toHaveLength(1);
    expect(notices[0]).toMatchObject({ title: 'Call audio cable not found for Moto' });
    // Line idle again: phone reads idle, the cable message stays visible.
    t.line.state = 'idle';
    await step();
    expect(t.bridge.status()[0]).toMatchObject({ state: 'idle', error: 'Call audio cable not found for Moto' });
    t.bridge.stop();
  });

  it('no bot picked: never polls the phone, status says what is missing', async () => {
    const t = setup({ phone: { ...MOTO, config: { ...CFG, botId: null } } });
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.line.reads).toBe(0);
    expect(t.bridge.status()[0]).toMatchObject({ enabled: true, state: 'idle', error: expect.stringMatching(/bot/) });
    t.bridge.stop();
  });

  it('disabled or offline phones are not polled', async () => {
    const t = setup({ phone: { ...MOTO, config: { ...CFG, enabled: false } } });
    t.bridge.start();
    await step(3_000);
    expect(t.line.reads).toBe(0);
    t.setPhone({ ...MOTO, online: false });
    await step(3_000);
    expect(t.line.reads).toBe(0);
    t.bridge.stop();
  });

  it('cloud refusal after answering ends the carrier call (no dead air)', async () => {
    const t = setup({ cloudFails: true });
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.calls).toEqual(['answer', 'end']);
    expect(t.bridge.status()[0]).toMatchObject({ state: 'error', error: expect.stringContaining('voice is not on your plan') });
    await step();
    expect(t.state()).toBe('idle');
    t.bridge.stop();
  });

  it('a failed join ends the carrier call too', async () => {
    const t = setup({ joinFails: true });
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.calls).toEqual(['answer', 'end']);
    expect(t.bridge.status()[0].error).toMatch(/call audio/);
    t.bridge.stop();
  });

  it('verifyCable flags and clears the cable error on an idle phone', async () => {
    const t = setup({ cable: { input: false, output: true } });
    expect(await t.bridge.verifyCable(MOTO.serial)).toMatchObject({ state: 'idle', error: 'Call audio cable not found for Moto' });
    t.media.cable = { input: true, output: true };
    expect((await t.bridge.verifyCable(MOTO.serial))?.error).toBeUndefined();
    expect(t.events.filter((e) => e.type === 'notice')).toEqual([]);
  });
});

describe('watcher: outbound', () => {
  it('an approved dial on an enabled phone joins as outbound once off-hook', async () => {
    const t = setup();
    t.bridge.start();
    expect(t.bridge.noteOutboundDial(MOTO.serial, '+15551234567')).toBe(true);
    t.line.state = 'offhook';
    await step();
    expect(t.state()).toBe('live');
    expect(t.rooms[0]).toMatchObject({ direction: 'outbound', from: null, to: '+15551234567' });
    expect(t.calls).toEqual([]); // no answer key on our own call
    t.line.state = 'idle';
    await step();
    expect(t.state()).toBe('idle');
    t.bridge.stop();
  });

  it('a dial on a phone without the bridge is not armed; an off-hook by hand is ignored', async () => {
    const t = setup({ phone: { ...MOTO, config: null } });
    expect(t.bridge.noteOutboundDial(MOTO.serial, '+15551234567')).toBe(false);
    const u = setup();
    u.bridge.start();
    u.line.state = 'offhook';
    await step(2_000);
    expect(u.rooms).toEqual([]);
    u.bridge.stop();
  });

  it('an arm older than a minute expires', async () => {
    const t = setup();
    t.bridge.start();
    t.bridge.noteOutboundDial(MOTO.serial, '+15551234567');
    await step(61_000);
    t.line.state = 'offhook';
    await step();
    expect(t.rooms).toEqual([]);
    t.bridge.stop();
  });
});

describe('watcher: iPhone', () => {
  const IPHONE: CallBridgePhone = { serial: 'link:ictrl-1', deviceId: '00008110-AAAA', label: 'Eoj iPhone', platform: 'ios', online: true, config: CFG };

  it('reports best_effort and needs several idle reads before ending', async () => {
    const t = setup({ phone: IPHONE });
    expect(t.bridge.status()[0]).toMatchObject({ platformSupport: 'best_effort', platform: 'ios' });
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    expect(t.state()).toBe('live');
    t.line.state = 'idle';
    await step();
    await step();
    expect(t.state()).toBe('live'); // a flicker of the call screen isn't the end
    await step();
    expect(t.state()).toBe('idle');
    expect(t.media.leaves).toEqual([IPHONE.serial]);
    t.bridge.stop();
  });

  it('an unreadable screen (locked) never ends a live call', async () => {
    const t = setup({ phone: IPHONE });
    t.bridge.start();
    t.line.state = 'ringing';
    await step(ONE_RING_MS + 1_000);
    t.line.unreadable = true;
    await step(10_000);
    expect(t.state()).toBe('live');
    t.bridge.stop();
  });
});

describe('parseIosCallScreen', () => {
  const items = (...labels: [string, number?, number?][]) => ({ items: labels.map(([label, x = 100, y = 700]) => ({ label, center_x: x, center_y: y })) });

  it('unlocked incoming call: Accept / Decline', () => {
    expect(parseIosCallScreen(items(['+1 (555) 987-6543', 196, 120], ['mobile'], ['Remind Me'], ['Message'], ['Decline', 80, 760], ['Accept', 310, 760]))).toEqual({
      state: 'ringing',
      number: '+15559876543',
      accept: { x: 310, y: 760 },
    });
  });

  it('banner while in another app', () => {
    expect(parseIosCallScreen(items(['Mom'], ['Decline', 260, 80], ['Accept', 330, 80])).state).toBe('ringing');
  });

  it('locked: slide to answer', () => {
    expect(parseIosCallScreen(items(['Unknown Caller'], ['slide to answer', 196, 760]))).toMatchObject({ state: 'ringing', slide: { x: 196, y: 760 } });
  });

  it('in a call: End call or the call controls', () => {
    expect(parseIosCallScreen(items(['00:05'], ['mute'], ['keypad'], ['speaker'], ['End call', 196, 780]))).toMatchObject({ state: 'offhook', end: { x: 196, y: 780 } });
    expect(parseIosCallScreen(items(['Audio'], ['FaceTime'], ['Add Call'])).state).toBe('offhook');
  });

  it('anything else is idle; a non-list answer is unknown', () => {
    expect(parseIosCallScreen(items(['Messages'], ['Mail'], ['Decline the invite?']))).toEqual({ state: 'idle' });
    expect(parseIosCallScreen(items(['Phone'], ['Keypad']))).toEqual({ state: 'idle' }); // the Phone app is not a call
    expect(parseIosCallScreen('raw text')).toEqual({ state: 'unknown' });
  });
});

describe('carrier lines', () => {
  it('Android: KEYCODE_CALL, then the headset button when the dialer ignores it', async () => {
    vi.useRealTimers();
    const keys: string[] = [];
    let state: 'ringing' | 'offhook' | 'idle' = 'ringing';
    const lane: AndroidCallLane = {
      telephonyState: async () => ({ ok: true, state }),
      callAnswer: async (_s, key = 'KEYCODE_CALL') => {
        keys.push(key);
        if (key === 'KEYCODE_HEADSETHOOK') state = 'offhook';
        return { ok: true };
      },
      callEnd: async () => {
        keys.push('END');
        state = 'idle';
        return { ok: true };
      },
    };
    const line = androidCarrierLine(async () => lane, async () => {});
    expect(await line.read(MOTO)).toEqual({ state: 'ringing' });
    expect(await line.answer(MOTO)).toBe(true);
    expect(keys).toEqual(['KEYCODE_CALL', 'KEYCODE_HEADSETHOOK']);
    expect(await line.end(MOTO)).toBe(true);
  });

  it('iPhone: taps Accept, then End', async () => {
    vi.useRealTimers();
    const IPHONE: CallBridgePhone = { serial: 'link:x', deviceId: 'u', label: 'iPhone', platform: 'ios', online: true, config: CFG };
    let screen: unknown = { items: [{ label: 'Decline', center_x: 80, center_y: 760 }, { label: 'Accept', center_x: 310, center_y: 760 }] };
    const taps: unknown[] = [];
    const line = iosCarrierLine(async (_p, method, params) => {
      if (method === 'screen_text') return screen;
      taps.push({ method, ...params });
      screen = { items: [{ label: 'mute' }, { label: 'keypad' }, { label: 'End call', center_x: 196, center_y: 780 }] };
      return {};
    }, async () => {});
    expect(await line.answer(IPHONE)).toBe(true);
    expect(await line.end(IPHONE)).toBe(true);
    expect(taps).toEqual([
      { method: 'tap', x: 310, y: 760 },
      { method: 'tap', x: 196, y: 780 },
    ]);
  });

  it('iPhone: a locked helper reads as unreadable, not idle', async () => {
    vi.useRealTimers();
    const line = iosCarrierLine(async () => {
      throw new Error('device locked');
    });
    expect(await line.read({ serial: 'link:x', deviceId: 'u', label: 'iPhone', platform: 'ios', online: true, config: CFG })).toBeNull();
  });
});

describe('requestLinkedCallRoom', () => {
  const REQ: LinkedCallRequest = {
    botId: 'bot_1',
    line: { kind: 'linked_phone', deviceId: 'ZY22ABC', serial: MOTO.serial, label: 'Moto', platform: 'android' },
    direction: 'inbound',
    from: '+15559876543',
    to: null,
  };
  const ROOM = { room: 'call-lp-1', livekitUrl: 'wss://lk', token: 't', identity: 'linked-phone-ZY22ABC' };

  it('posts with the device token and returns the room', async () => {
    vi.useRealTimers();
    const seen: { url: string; init: RequestInit }[] = [];
    const fetchFn = (async (url: string, init: RequestInit) => {
      seen.push({ url, init });
      return new Response(JSON.stringify(ROOM), { status: 200 });
    }) as unknown as typeof fetch;
    const room = await requestLinkedCallRoom({ baseUrl: () => 'https://api.example/', deviceToken: () => 'dev-tok', clerkToken: async () => null, fetch: fetchFn }, REQ);
    expect(room).toEqual(ROOM);
    expect(seen[0].url).toBe('https://api.example/api/v1/voice/linked-calls');
    expect((seen[0].init.headers as Record<string, string>).authorization).toBe('Bearer dev-tok');
    expect(JSON.parse(String(seen[0].init.body))).toEqual(REQ);
  });

  it('falls back to the Clerk token, and refuses when signed out', async () => {
    vi.useRealTimers();
    const auths: string[] = [];
    const fetchFn = (async (_u: string, init: RequestInit) => {
      auths.push((init.headers as Record<string, string>).authorization);
      return new Response(JSON.stringify(ROOM), { status: 200 });
    }) as unknown as typeof fetch;
    await requestLinkedCallRoom({ baseUrl: () => 'https://a', deviceToken: () => null, clerkToken: async () => 'clerk', fetch: fetchFn }, REQ);
    expect(auths).toEqual(['Bearer clerk']);
    await expect(requestLinkedCallRoom({ baseUrl: () => 'https://a', deviceToken: () => null, clerkToken: async () => null, fetch: fetchFn }, REQ)).rejects.toThrow(/Sign in/);
    expect(auths).toHaveLength(1);
  });

  it('surfaces the refusal message (no voice on the plan, not your bot)', async () => {
    vi.useRealTimers();
    const fetchFn = (async () => new Response(JSON.stringify({ error: 'voice is not included in your plan' }), { status: 402 })) as unknown as typeof fetch;
    await expect(requestLinkedCallRoom({ baseUrl: () => 'https://a', deviceToken: () => 'd', clerkToken: async () => null, fetch: fetchFn }, REQ)).rejects.toThrow('voice is not included in your plan');
  });
});
