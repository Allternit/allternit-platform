import { afterEach, describe, expect, it } from 'vitest';
import { APP_TAPS_MESSAGE, AppPhoneBackend } from './phone-app-store.js';
import { LinkBoardLane } from './phone-link-board.js';
import { PhoneToolRunner, type PhoneHoldNotice } from './phone-tools.js';
import { UsbAgentManager, serialOf } from './phone-usb-agent.js';
import type { GuardAnswer, GuardCheckBody, PhoneHoldClient } from './phone-hold-client.js';
import type { PhoneApi } from './phone-runs.js';
import { SimBoard, SimPhone, fakeDecode, fakeJpeg, fakeUxplaySpawner, until } from './phone-usb-agent.testkit.js';
import { APP_SERIAL, FakeAppPhone, fakeSerial, memoryStore } from './phone-link-board.testkit.js';

// Real PhoneToolRunner, AppPhoneBackend, UsbAgentManager, UsbAgentLink and LinkBoardLane.
// Edges only: the node (app phone list, viewer socket), the guard client, the node task route,
// UxPlay, the serial port, Chromium's H.264 decoder. The board is a SimBoard over TCP.

const BOARD = 'usba-00aa11bb22cc';

const managers: UsbAgentManager[] = [];
const boards: SimBoard[] = [];
const backends: AppPhoneBackend[] = [];
afterEach(() => {
  for (const b of boards.splice(0)) b.close();
  for (const m of managers.splice(0)) m.stopAll();
  for (const a of backends.splice(0)) a.dispose();
});

function fakeGuard(decide: (b: GuardCheckBody) => GuardAnswer) {
  const bodies: GuardCheckBody[] = [];
  const client: PhoneHoldClient = {
    check: async (b) => {
      bodies.push(JSON.parse(JSON.stringify(b)) as GuardCheckBody);
      return decide(b);
    },
    getHold: async () => null,
    resolve: async () => true,
  };
  return { client, bodies };
}

function fakeNodeApi() {
  const tasks: Array<{ serial: string; goal: string }> = [];
  const api: PhoneApi = {
    postStep: async () => undefined,
    task: async (serial, body) => {
      tasks.push({ serial, goal: body.goal });
      return { ok: true, status: 'succeeded', steps: 3 };
    },
    skills: async () => ({ skills: [] }),
    saveSkill: async () => ({}),
    runSkill: async () => ({ ok: true }),
  };
  return { api, tasks };
}

async function rig(opts: { pair?: boolean; connect?: boolean; guard?: PhoneHoldClient; api?: PhoneApi } = {}) {
  const phone = new SimPhone(600, 1300, 1.04);
  const app = new FakeAppPhone(phone);
  const store = memoryStore();
  const ux = fakeUxplaySpawner();
  const serial = fakeSerial(BOARD);
  let apps: AppPhoneBackend | null = null;
  const manager = new UsbAgentManager({
    store,
    sealer: { seal: (h) => `test:${h}`, open: (s) => s.slice(5) },
    spawnUxplay: ux.spawn,
    advertise: null,
    decode: (jpeg) => fakeDecode(jpeg),
    registerFile: (slot) => `/tmp/reg-${slot}`,
    openSerial: async () => serial.port,
    listSerial: () => ['/dev/cu.usbmodem101'],
    lanAddresses: () => ['192.168.1.20'],
    linkPort: 0,
    eyes: (pairedPhone) => apps?.frameSource(pairedPhone) ?? null,
  });
  managers.push(manager);
  const lane = new LinkBoardLane({ usb: () => manager });
  apps = new AppPhoneBackend({ node: app.node, streams: app.streams, decodeStill: app.decodeStill, hands: lane, frameTimeoutMs: 1_000 });
  backends.push(apps);
  const port = await manager.start();
  const info = await manager.setUp('/dev/cu.usbmodem101', { ssid: 'Home Wi-Fi', password: 'correct horse' }, 'Eoj iPhone');
  const board = new SimBoard(info.boardId, serial.settings.key, phone);
  boards.push(board);
  board.onCommand = () => setTimeout(() => app.push(), 5);
  if (opts.pair !== false) manager.pair(BOARD, APP_SERIAL);
  if (opts.connect !== false) await board.connect(port);
  const notices: PhoneHoldNotice[] = [];
  const runner = new PhoneToolRunner({
    adb: Object.assign(async () => ({ stdout: '', stderr: '', code: 0 }), { binary: async () => Buffer.alloc(0) }),
    gate: { request: async () => 'denied' },
    rates: { load: () => [], save: () => undefined },
    artemis: () => null,
    onlineSerials: () => [],
    usb: () => manager,
    apps: () => apps,
    ...(opts.guard ? { guard: () => opts.guard ?? null, onHold: (n: PhoneHoldNotice) => void notices.push(n) } : {}),
    ...(opts.api ? { api: () => opts.api ?? null } : {}),
  });
  /** Desktop's lines to the board, heartbeats left out. */
  const linesToBoard: string[] = [];
  board.socket?.on('data', (chunk: Buffer) => {
    for (const l of chunk.toString('latin1').split('\n')) if (l.startsWith('c ') && !/^c \d+ ping /.test(l)) linesToBoard.push(l);
  });
  return { manager, store, app, apps, lane, board, phone, port, ux, runner, notices, linesToBoard, key: serial.settings.key };
}

async function armAndCalibrate(r: Awaited<ReturnType<typeof rig>>) {
  r.board.arm();
  await until(() => r.manager.list()[0].armed);
  expect(await r.manager.calibrate(BOARD)).toEqual({ ok: true });
}

describe('Allternit Link: board hands, app-phone eyes', () => {
  it('paired and armed: a bot taps a native button on the app iPhone through the board, aimed with the shared screen', async () => {
    const r = await rig();
    // Before calibration the board's own answer comes back, never not_supported.
    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    expect(await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 90, y: 130 })).toMatchObject({ ok: false, error: 'not_calibrated' });
    expect(await r.manager.calibrate(BOARD)).toEqual({ ok: true });

    // The screenshot is the frame the pointer uses: pixel coordinates, no coordinate_space: 'none'.
    const shot = await r.runner.call('phone.screenshot', { serial: APP_SERIAL });
    expect(shot).toMatchObject({ ok: true, mimeType: 'image/jpeg', width: 120, height: 260 });
    expect(shot).not.toHaveProperty('coordinate_space');
    expect(typeof (shot as { ageMs?: unknown }).ageMs).toBe('number');

    const before = r.board.ref.log.length;
    const tap = await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 90, y: 130 });
    expect(tap).toMatchObject({ ok: true, screenChanged: true });
    const sent = r.board.ref.log.slice(before);
    expect(sent.some((l) => l.startsWith('move '))).toBe(true);
    expect(sent).toContain('click');
    expect(r.phone.checked).toBe(true);
    const [tx, ty] = r.phone.taps[r.phone.taps.length - 1];
    expect(Math.abs(tx - 90)).toBeLessThan(4);
    expect(Math.abs(ty - 130)).toBeLessThan(4);

    // A USBA/1 board never receives face or prog lines (its firmware would stay armed on them).
    expect(r.board.ref.log.filter((l) => /^(face|prog|label) /.test(l))).toEqual([]);
    // The status every surface reads.
    expect(r.manager.status(BOARD)).toMatchObject({ status: 'ready', eyes: 'app', phone: APP_SERIAL, screen: true, online: true, armed: true, protocol: 1, link: null, relay: 'none' });
  }, 30_000);

  it('not paired: taps on the app iPhone answer not_supported with the board message, as before', async () => {
    const r = await rig({ pair: false });
    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    const res = await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 10, y: 10 });
    expect(res).toMatchObject({ ok: false, error: 'not_supported' });
    expect((res as { message: string }).message.startsWith(APP_TAPS_MESSAGE)).toBe(true);
    expect(r.board.ref.log).toEqual([]);
    // The board is a phone of its own again.
    expect(r.manager.serials()).toEqual([serialOf(BOARD)]);
  });

  it('paired and disarmed answers disarmed; paired and offline answers device_offline', async () => {
    const disarmed = await rig();
    await until(() => disarmed.manager.list()[0].usb === 'mounted');
    expect(await disarmed.runner.call('phone.tap', { serial: APP_SERIAL, x: 10, y: 10 })).toMatchObject({ ok: false, error: 'disarmed' });
    expect(await disarmed.runner.call('phone.type', { serial: APP_SERIAL, text: 'hi' })).toMatchObject({ ok: false, error: 'disarmed' });

    const offline = await rig({ connect: false });
    expect(await offline.runner.call('phone.tap', { serial: APP_SERIAL, x: 10, y: 10 })).toMatchObject({ ok: false, error: 'device_offline' });
    // The board's link is not needed to look: the app's shared screen still answers, as a usb: phone's mirror does.
    expect(await offline.runner.call('phone.screenshot', { serial: APP_SERIAL })).toMatchObject({ ok: true, width: 120, height: 260 });
  });

  it('a disarm mid-swipe answers disarmed; later calls send nothing until the board is armed again, link loss included', async () => {
    const r = await rig();
    await armAndCalibrate(r);
    let pressed = false;
    r.board.onCommand = (cmd) => {
      setTimeout(() => r.app.push(), 5);
      if (cmd === 'press' && !pressed) {
        pressed = true;
        r.board.arm(false); // the button, in the middle of the drag
      }
    };
    expect(await r.runner.call('phone.swipe', { serial: APP_SERIAL, direction: 'up' })).toMatchObject({ ok: false, error: 'disarmed' });
    expect(pressed).toBe(true);
    await until(() => !r.manager.list()[0].armed);

    const sentBefore = r.linesToBoard.length;
    expect(await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 90, y: 130 })).toMatchObject({ ok: false, error: 'disarmed' });
    // Looking still works while disarmed, as on a usb: phone: a frame with its size, and nothing sent to the board.
    expect(await r.runner.call('phone.screenshot', { serial: APP_SERIAL })).toMatchObject({ ok: true, mimeType: 'image/jpeg', width: 120, height: 260 });
    expect(r.linesToBoard.length).toBe(sentBefore);

    // Armed again: calls work.
    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    expect(await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 90, y: 130 })).toMatchObject({ ok: true });
    expect(r.phone.checked).toBe(true);

    // The link drops while armed: the run stops as disarmed, not as a phone that went away.
    r.board.close();
    await until(() => r.manager.list()[0].status === 'board_offline');
    expect(await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 90, y: 130 })).toMatchObject({ ok: false, error: 'disarmed' });
    expect(await r.runner.call('phone.screenshot', { serial: APP_SERIAL })).toMatchObject({ ok: true, width: 120, height: 260 });
  }, 30_000);

  it('the guard still holds a sensitive tap on a paired phone: approval_required, nothing reaches the board, the guard saw the screen', async () => {
    const guard = fakeGuard((b) => (b.holdId === 'h1' ? { decision: 'allow', holdId: null } : { decision: 'hold', approvalId: 'h1', inboxId: 'approval:h1', effect: 'delete', expiresAt: '2026-11-10T10:05:00Z' }));
    const r = await rig({ guard: guard.client });
    await armAndCalibrate(r);
    const logBefore = r.board.ref.log.length;
    const sentBefore = r.linesToBoard.length;

    const res = await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 90, y: 130 });
    expect(res).toMatchObject({ ok: false, error: 'approval_required', approvalId: 'h1' });
    expect(r.board.ref.log.length).toBe(logBefore);
    expect(r.linesToBoard.length).toBe(sentBefore);
    expect(r.phone.checked).toBe(false);
    expect(guard.bodies[0]).toMatchObject({ serial: APP_SERIAL, tool: 'phone.tap', platform: 'ios' });
    expect(guard.bodies[0].screenshot).toMatch(/^data:image\/jpeg;base64,./);
    expect(r.notices[0]).toMatchObject({ approvalId: 'h1', serial: APP_SERIAL });

    // Approved: the retry taps.
    expect(await r.runner.call('phone.tap', { serial: APP_SERIAL, x: 90, y: 130, _hold: 'h1' })).toMatchObject({ ok: true });
    expect(r.phone.checked).toBe(true);
  }, 30_000);

  it('phone.devices lists a paired pair as one phone; phone.task on it goes to the node task route', async () => {
    const node = fakeNodeApi();
    const r = await rig({ api: node.api });
    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    // The bot looks first (the eyes join the shared screen), then lists phones.
    expect(await r.runner.call('phone.screenshot', { serial: APP_SERIAL })).toMatchObject({ ok: true });
    const devices = await r.runner.call('phone.devices', {});
    expect(devices).toMatchObject({ ok: true, serials: [APP_SERIAL] });
    const list = (devices as unknown as { devices: Array<{ serial: string; note: string }> }).devices;
    expect(list).toHaveLength(1);
    expect(list[0].note).toMatch(/Allternit Link board is paired.*taps work while it is armed.*not calibrated/);

    expect(await r.runner.call('phone.task', { serial: APP_SERIAL, task: 'set an alarm for 7am' })).toMatchObject({ ok: true, done: true });
    expect(node.tasks).toEqual([{ serial: APP_SERIAL, goal: 'set an alarm for 7am' }]);

    // Unpaired, the same phone still refuses tasks as today.
    r.manager.pair(BOARD, null);
    expect(await r.runner.call('phone.task', { serial: APP_SERIAL, task: 'set an alarm for 7am' })).toMatchObject({ ok: false, error: 'not_supported' });
    expect(node.tasks).toHaveLength(1);
  });

  it('pairing is one to one, clears calibration, hides the board as its own phone and stops its AirPlay receiver', async () => {
    const r = await rig({ pair: false });
    // Unpaired, the board sees through AirPlay.
    const airplayFrame = () => r.ux.spawned[r.ux.spawned.length - 1].write(fakeJpeg(r.phone.render()));
    r.board.onCommand = () => setTimeout(airplayFrame, 5);
    airplayFrame();
    await armAndCalibrate(r);
    expect(r.store.cal.get(BOARD)).toBeTruthy();
    const receiver = r.ux.spawned[0];
    expect(receiver.exitCode).toBeNull();

    const info = r.manager.pair(BOARD, APP_SERIAL);
    expect(info).toMatchObject({ eyes: 'app', phone: APP_SERIAL, calibrated: false });
    expect(r.store.cal.get(BOARD)).toBeUndefined();
    expect(r.store.boards[0].pairedPhone).toBe(APP_SERIAL);
    expect(receiver.exitCode).not.toBeNull();
    expect(r.ux.spawned).toHaveLength(1);
    expect(r.manager.serials()).toEqual([]);
    expect(r.manager.has(serialOf(BOARD))).toBe(false);
    expect(r.manager.boardForPhone(APP_SERIAL)).toBe(BOARD);
    expect(() => r.manager.pair(BOARD, 'usb:usba-111111111111')).toThrow();

    // Unpaired: AirPlay eyes again.
    expect(r.manager.pair(BOARD, null)).toMatchObject({ eyes: 'airplay', phone: null });
    expect(r.ux.spawned).toHaveLength(2);
    expect(r.manager.serials()).toEqual([serialOf(BOARD)]);
  }, 30_000);

  it('the eyes leave the stream when the person stops sharing', async () => {
    const r = await rig();
    r.board.arm();
    await until(() => r.manager.list()[0].armed);
    expect(await r.runner.call('phone.screenshot', { serial: APP_SERIAL })).toMatchObject({ ok: true });
    expect(r.app.openViewers()).toBe(1);
    r.app.stopSharing();
    await r.apps.refresh(true);
    await until(() => r.app.openViewers() === 0);
    const res = await r.runner.call('phone.screenshot', { serial: APP_SERIAL });
    expect(res).toMatchObject({ ok: false, error: 'no_screen' });
    expect((res as { message: string }).message).toMatch(/^Open the Allternit app on your iPhone and tap Share screen/);
  });
});
