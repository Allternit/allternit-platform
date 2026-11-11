/**
 * The real USB Agent firmware (built as a Mac program, hardware/usb-agent/native)
 * against the real Desktop code: UsbAgentLink for the desk, and a raw socket
 * built from phone-usb-agent-protocol.ts where the desk must be silent or
 * hostile. The board process is the only fake; every assertion is about what
 * the user gets: the reports the phone would receive, the reply Desktop gets,
 * the LED level, the socket closing.
 *
 * Plan: docs/research-allternit-link-native-board-PLAN.md, step 5.
 */

import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import { UsbAgentLink } from './phone-usb-agent-link.js';
import { newKeyHex, type BoardCommand } from './phone-usb-agent-protocol.js';
import { until } from './phone-usb-agent.testkit.js';
import { NativeBoard, RawDesk, buildNative, closeRawDesks, deadPort, removeBuild, type Variant } from './phone-usb-agent.native.testkit.js';

const SSID = 'HomeNet';
const PASS = 'homepass123';
const DESK_ID = 'desk-test-1';
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
// Wall-clock budget per test: the Mac running this is often heavily loaded.
const SLOW = 90_000;

const links: UsbAgentLink[] = [];

beforeAll(async () => {
  // Build once. A failure is rethrown by every NativeBoard.start() with the compiler output.
  await buildNative().catch(() => undefined);
}, 600_000);

afterEach(async () => {
  await NativeBoard.stopAll();
  for (const l of links.splice(0)) l.close();
  closeRawDesks();
});

afterAll(() => removeBuild());

interface Rig {
  board: NativeBoard;
  key: string;
  id: string;
  port: number;
  link?: UsbAgentLink;
  raw?: RawDesk;
}

interface RigOptions {
  desk: 'link' | 'raw' | 'dead';
  variant?: Variant;
  /** Put the home network on the air before setup (default true). */
  air?: boolean;
  networks?: Array<{ ssid: string; password: string }>;
  /** Wait for a link session before returning (link desks only; default true). */
  session?: boolean;
  linkOpts?: ConstructorParameters<typeof UsbAgentLink>[1];
}

/** Starts a desk and a board, and sets the board up over its pty with the real `prov` lines. */
async function bringUp(o: RigOptions): Promise<Rig> {
  const key = newKeyHex();
  const board = await NativeBoard.start({ variant: o.variant });
  const id = board.id;
  const rig: Rig = { board, key, id, port: 0 };
  if (o.desk === 'link') {
    const link = new UsbAgentLink((b) => (b === id ? key : null), { heartbeatMs: 2_000, ...o.linkOpts });
    links.push(link);
    rig.link = link;
    rig.port = await link.listen(0, '127.0.0.1');
  } else if (o.desk === 'raw') {
    rig.raw = await RawDesk.listen(key);
    rig.port = rig.raw.port;
  } else {
    rig.port = await deadPort();
  }
  if (o.air !== false) board.wifiAir(SSID, PASS);
  await board.provision({ ssid: SSID, password: PASS, host: '127.0.0.1', port: rig.port, deskId: DESK_ID, keyHex: key, networks: o.networks });
  if (rig.link && o.session !== false) await until(() => rig.link!.session(id) !== null, 20_000);
  return rig;
}

/** Presses the arm button for a clearly long time and waits until the LED is on (and, with a link, Desktop knows). */
async function arm(r: Rig): Promise<void> {
  await r.board.pressButton(300);
  await until(() => r.board.led() === 0, 10_000);
  if (r.link) await until(() => r.link!.session(r.id)?.state.armed === true, 10_000);
}

const mouse = (b: NativeBoard, from = 0) => b.hid(from).filter((r) => r.dev === 'mouse');

describe('native board with the real UsbAgentLink', () => {
  it('boots disarmed and no command Desktop can send arms it', async () => {
    const r = await bringUp({ desk: 'link' });
    const link = r.link!;
    expect(link.session(r.id)?.state.armed).toBe(false);
    expect(r.board.led()).toBe(1); // active low: high is dark
    const mk = r.board.mark();

    const input: BoardCommand[] = [
      { op: 'home' },
      { op: 'move', dx: 10, dy: 10 },
      { op: 'press' },
      { op: 'release' },
      { op: 'click' },
      { op: 'scroll', v: 3, h: 0 },
      { op: 'key', combo: 'enter' },
      { op: 'type', text: 'abc' },
    ];
    for (const cmd of input) expect(await link.send(r.id, cmd), cmd.op).toMatchObject({ ok: false, error: 'disarmed' });
    // The display commands are allowed while disarmed and must not arm.
    expect(await link.send(r.id, { op: 'face', name: 'working' })).toEqual({ ok: true });
    expect(await link.send(r.id, { op: 'prog', n: 2 })).toEqual({ ok: true });
    expect(await link.send(r.id, { op: 'ping' })).toEqual({ ok: true });
    await until(() => link.session(r.id)?.state.face === 'working');

    await sleep(300);
    const state = link.session(r.id)!.state;
    expect(state).toMatchObject({ armed: false, face: 'working', prog: 2 });
    expect(r.board.led()).toBe(1);
    expect(mouse(r.board, mk)).toEqual([]);
    expect(r.board.hidTotals(mk)).toEqual({ dx: 0, dy: 0, clicks: 0, typed: '' });
  }, SLOW);

  it('a button press arms: LED pin goes low, Desktop sees armed with face default; a second press disarms', async () => {
    const r = await bringUp({ desk: 'link' });
    await arm(r);
    expect(r.board.led()).toBe(0);
    expect(r.link!.session(r.id)?.state).toMatchObject({ armed: true, face: 'default', usb: 'mounted' });

    // A press within 40 ms of the last release is bounce to the firmware, so let the button settle first.
    await sleep(200);
    await r.board.pressButton(300);
    await until(() => r.link!.session(r.id)?.state.armed === false, 10_000);
    expect(r.board.led()).toBe(1);
    expect(r.link!.session(r.id)?.state).toMatchObject({ armed: false, face: 'stopped', prog: 0 });
  }, SLOW);

  it('move 37 -12 gives reports that sum to (37, -12), each step at most 10, at least 8 ms apart', async () => {
    const r = await bringUp({ desk: 'link' });
    await arm(r);
    const mk = r.board.mark();
    expect(await r.link!.send(r.id, { op: 'move', dx: 37, dy: -12 })).toEqual({ ok: true });
    const reports = mouse(r.board, mk).filter((m) => m.sent === 1);
    expect(r.board.hidTotals(mk)).toMatchObject({ dx: 37, dy: -12 });
    expect(reports.length).toBe(4);
    for (const m of reports) {
      expect(Math.abs(m.x ?? 0)).toBeLessThanOrEqual(10);
      expect(Math.abs(m.y ?? 0)).toBeLessThanOrEqual(10);
    }
    for (let i = 1; i < reports.length; i += 1) expect(reports[i].t - reports[i - 1].t).toBeGreaterThanOrEqual(8);
  }, SLOW);

  it('the real Desktop types text (sent sealed as typex) and the board types exactly that text', async () => {
    const r = await bringUp({ desk: 'link' });
    await arm(r);
    const mk = r.board.mark();
    const text = 'Hello, World! ~ 123 {}';
    expect(await r.link!.send(r.id, { op: 'type', text })).toEqual({ ok: true });
    expect(r.board.hidTotals(mk).typed).toBe(text);
  }, SLOW);

  it('the real link accepts the board opening state lines (n=1 before ready, n=2 at ready)', async () => {
    const key = newKeyHex();
    const board = await NativeBoard.start();
    const link = new UsbAgentLink((b) => (b === board.id ? key : null), { heartbeatMs: 2_000 });
    links.push(link);
    const states: unknown[] = [];
    const ended: unknown[] = [];
    const rejected: unknown[] = [];
    link.on('state', (s) => states.push(s));
    link.on('ended', (e) => ended.push(e));
    link.on('rejected', (e) => rejected.push(e));
    const port = await link.listen(0, '127.0.0.1');
    board.wifiAir(SSID, PASS);
    await board.provision({ ssid: SSID, password: PASS, host: '127.0.0.1', port, deskId: DESK_ID, keyHex: key });
    await until(() => link.session(board.id) !== null, 30_000);
    await until(() => states.length >= 2, 10_000);
    await sleep(500);
    expect(states).toHaveLength(2);
    for (const s of states as Array<{ state: unknown }>) {
      expect(s.state).toMatchObject({ armed: false, usb: 'mounted', dock: false, hid: 'usb', link: 'lan', face: 'default', prog: 0 });
    }
    expect(ended).toEqual([]);
    expect(rejected).toEqual([]);
  }, SLOW);

});
