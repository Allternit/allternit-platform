import { describe, expect, it } from 'vitest';
import {
  APP_SHARE_SCREEN_MESSAGE,
  AppPhoneBackend,
  AppPhoneViewerSession,
  type AppPhoneNode,
  type AppRpcResult,
  type AppViewerSocket,
  type StillInput,
} from './phone-app-store.js';
import { encodeIosVideoPacket, IOS_PACKET_CONFIG, IOS_PACKET_ENDED, IOS_PACKET_FRAME } from './phone-ios-video.js';
import { PhoneStreamManager } from './phone-stream.js';
import { PhoneToolRunner } from './phone-tools.js';

const SERIAL = 'app:5b1f0c3e-8d2a-4c11-9a7e-0f6b2d4c9e21';
// SPS (profile 0x42, compat 0xe0, level 0x1f) + PPS + IDR slice, then one P slice.
const IDR = Buffer.from([0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0xaa, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88, 0x84]);
const P = (n: number) => Buffer.from([0, 0, 0, 1, 0x41, 0x9a, n]);

/** The node's viewer socket: the test pushes what the node would send. */
function fakeSocket() {
  const handlers: { message?: (d: Buffer | string, bin: boolean) => void; close?: (c: number, r: string) => void } = {};
  const socket: AppViewerSocket = {
    send: () => undefined,
    close: () => handlers.close?.(1000, ''),
    onMessage: (cb) => (handlers.message = cb),
    onClose: (cb) => (handlers.close = cb),
    onError: () => undefined,
  };
  return { socket, packet: (type: number, payload: Buffer | object) => handlers.message?.(encodeIosVideoPacket(type, payload), true) };
}

function setup(opts: { online?: boolean; rpc?: AppRpcResult } = {}) {
  const sock = fakeSocket();
  const decoded: StillInput[] = [];
  const node: AppPhoneNode = {
    listPhones: async () => [{ serial: SERIAL, name: 'Eoj iPhone', model: 'iPhone', online: opts.online ?? true }],
    rpc: async () => opts.rpc ?? { ok: true, result: {} },
    openViewer: async () => sock.socket,
    decidePair: async () => ({ ok: true }),
  };
  const streams = new PhoneStreamManager({
    createSession: () => new AppPhoneViewerSession({ open: node.openViewer.bind(null, SERIAL), deviceName: 'Eoj iPhone', startTimeoutMs: 1_000 }),
  });
  const apps = new AppPhoneBackend({
    node,
    streams: {
      start: (s) => streams.start(s),
      stop: (s) => streams.stop(s),
      status: (s) => streams.status(s),
      subscribe: (s, c) => streams.subscribe(s, c),
      consumerCount: (s) => streams.consumerCount(s),
    },
    decodeStill: async (input) => {
      decoded.push(input);
      return { jpeg: Buffer.from('jpeg-bytes'), width: 1179, height: 2556 };
    },
    frameTimeoutMs: 200,
  });
  const runner = new PhoneToolRunner({
    adb: Object.assign(async () => ({ stdout: '', stderr: '', code: 0 }), { binary: async () => Buffer.alloc(0) }),
    gate: { request: async () => 'denied' },
    rates: { load: () => [], save: () => undefined },
    artemis: () => null,
    onlineSerials: () => [],
    apps: () => apps,
  });
  return { sock, streams, runner, decoded };
}

describe('iPhones with the Allternit app', () => {
  it('shows the shared screen live and ends when the person stops sharing', async () => {
    const { sock, streams } = setup();
    const starting = streams.start(SERIAL);
    await new Promise((r) => setTimeout(r, 0));
    sock.packet(IOS_PACKET_CONFIG, { width: 1179, height: 2556, scale: 3, rotation: 90 });
    expect(await starting).toEqual({ ok: true });
    expect(streams.status(SERIAL)).toEqual({ state: 'live', meta: { deviceName: 'Eoj iPhone', width: 1179, height: 2556, codec: 'h264', rotation: 90 } });
    sock.packet(IOS_PACKET_ENDED, { reason: 'You stopped sharing.' });
    expect(streams.status(SERIAL)).toEqual({ state: 'ended', reason: 'You stopped sharing.' });
  });

  it('phone.screenshot decodes from the latest keyframe on', async () => {
    const { sock, runner, decoded } = setup();
    const shot = runner.call('phone.screenshot', { serial: SERIAL });
    await new Promise((r) => setTimeout(r, 0));
    sock.packet(IOS_PACKET_CONFIG, { width: 1179, height: 2556, scale: 3, rotation: 0 });
    await new Promise((r) => setTimeout(r, 0));
    sock.packet(IOS_PACKET_FRAME, P(1)); // before any keyframe: can't be decoded alone
    sock.packet(IOS_PACKET_FRAME, IDR);
    sock.packet(IOS_PACKET_FRAME, P(2));
    const result = await shot;
    expect(result).toMatchObject({ ok: true, mimeType: 'image/jpeg', dataBase64: Buffer.from('jpeg-bytes').toString('base64'), width: 1179 });
    expect(decoded[0].codec).toBe('avc1.42e01f');
    expect(decoded[0].units.map((u) => u.toString('hex'))).toEqual([IDR.toString('hex'), P(2).toString('hex')]);
  });

  it('says how to share the screen when the iPhone is not sharing', async () => {
    const { runner } = setup({ online: false });
    const res = await runner.call('phone.screenshot', { serial: SERIAL });
    expect(res).toMatchObject({ ok: false, error: 'no_screen' });
    expect((res as { message: string }).message.startsWith(APP_SHARE_SCREEN_MESSAGE)).toBe(true);
  });

  it('refuses taps with the Bluetooth board message', async () => {
    const { runner } = setup();
    const res = await runner.call('phone.tap', { serial: SERIAL, x: 10, y: 10 });
    expect(res).toMatchObject({ ok: false, error: 'not_supported' });
    expect((res as { message: string }).message).toMatch(/^Taps on native apps need the Bluetooth board/);
  });

  it('phone.web.* tell the bot to open the app when it is closed', async () => {
    const { runner } = setup({ online: false, rpc: { ok: false, status: 409, code: 'app_not_open', message: '' } });
    expect(await runner.call('phone.web.click', { serial: SERIAL, ref: 'e12' })).toMatchObject({ ok: false, error: 'app_not_open' });
  });

  it('phone.web.snapshot returns the page picture as an image', async () => {
    const elements = [{ ref: 'e1', role: 'link', name: 'Sign in', text: 'Sign in', bounds: { x: 1, y: 2, w: 3, h: 4 } }];
    const { runner } = setup({ rpc: { ok: true, result: { url: 'https://example.com/', title: 'Example', elements, jpegBase64: 'AAAA' } } });
    expect(await runner.call('phone.web.snapshot', { serial: SERIAL })).toMatchObject({
      ok: true,
      url: 'https://example.com/',
      elements,
      mimeType: 'image/jpeg',
      dataBase64: 'AAAA',
    });
  });

  it('explains a browser that was closed and a stale ref', async () => {
    const closed = setup({ rpc: { ok: false, status: 200, code: 'browser_not_open', message: '' } });
    expect(await closed.runner.call('phone.web.snapshot', { serial: SERIAL })).toMatchObject({ ok: false, error: 'app_not_open' });
    const stale = setup({ rpc: { ok: false, status: 200, code: 'stale_ref', message: '' } });
    expect(await stale.runner.call('phone.web.click', { serial: SERIAL, ref: 'e1' })).toMatchObject({ ok: false, error: 'not_found' });
  });

  it('rejects a non-web address for phone.web.open', async () => {
    const { runner } = setup();
    expect(await runner.call('phone.web.open', { serial: SERIAL, url: 'file:///etc/passwd' })).toMatchObject({ ok: false, error: 'invalid_args' });
  });
});
