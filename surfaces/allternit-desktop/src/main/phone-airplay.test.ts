import { describe, expect, it } from 'vitest';
import { AirplayScreen, FRAME_FD, JpegSplitter, isAirplayPin, jpegSize, uxplayArgs } from './phone-airplay.js';
import { candidatePorts, lanAddresses, readIdentity, validateProvision, type SerialPort } from './phone-usb-agent-provision.js';
import { changed, findPointer, grayFromBgra, resize, type Gray } from './phone-usb-agent-vision.js';
import { fakeJpeg, fakeUxplaySpawner, until } from './phone-usb-agent.testkit.js';

const OPTS = { name: 'Allternit', basePort: 47100, fps: 8, quality: 75, pin: '4821', registerFile: '/tmp/reg' };
const img = (w: number, h: number, v = 40): Gray => ({ width: w, height: h, data: new Uint8Array(w * h).fill(v) });

describe('AirPlay frames', () => {
  it('splits JPEGs out of an arbitrarily chunked stream', () => {
    const a = fakeJpeg(img(10, 20));
    const b = fakeJpeg(img(30, 40));
    const out: Buffer[] = [];
    const s = new JpegSplitter((f) => out.push(f));
    const all = Buffer.concat([Buffer.from([1, 2, 3]), a, b]);
    for (let i = 0; i < all.length; i += 3) s.push(all.subarray(i, i + 3));
    expect(out.map((f) => jpegSize(f))).toEqual([{ width: 10, height: 20 }, { width: 30, height: 40 }]);
  });

  it('launches UxPlay PIN-protected on its own ports, writing frames to fd 4', () => {
    const args = uxplayArgs(OPTS);
    expect(args).toEqual(expect.arrayContaining(['-n', 'Allternit', '-nh', '-p', '47100', '-pin', '4821', '-reg', '/tmp/reg']));
    expect(args[args.indexOf('-vs') + 1]).toContain(`fdsink fd=${FRAME_FD}`);
    expect(() => uxplayArgs({ ...OPTS, pin: '12' })).toThrow();
    expect(isAirplayPin('0042')).toBe(true);
  });

  it('keeps the newest frame, reports mirroring, and restarts UxPlay when it exits', async () => {
    const ux = fakeUxplaySpawner();
    const screen = new AirplayScreen({ spawn: ux.spawn, restartDelayMs: 10 }, OPTS);
    expect(screen.state()).toBe('stopped');
    screen.start();
    expect(screen.state()).toBe('waiting');
    ux.spawned[0].write(fakeJpeg(img(12, 24)));
    await until(() => screen.state() === 'mirroring');
    expect(screen.latest()).toMatchObject({ width: 12, height: 24 });
    ux.spawned[0].kill();
    await until(() => ux.spawned.length === 2);
    expect(screen.latest()).toBeNull();
    screen.stop();
    expect(screen.state()).toBe('stopped');
  });

  it('is unavailable without UxPlay', () => {
    expect(new AirplayScreen({ spawn: null }, OPTS).state()).toBe('unavailable');
  });
});

describe('vision', () => {
  it('converts BGRA to gray and downsizes by area', () => {
    const g = grayFromBgra(1, 1, new Uint8Array([0, 0, 255, 255]));
    expect(g.data[0]).toBe(76);
    expect(resize(img(4, 4, 100), 2, 2).data).toEqual(new Uint8Array([100, 100, 100, 100]));
  });

  it('sees small changes, ignores the status bar and masked pointer spots', () => {
    const a = img(100, 200);
    const b = { ...a, data: new Uint8Array(a.data) };
    for (let y = 100; y < 110; y += 1) for (let x = 40; x < 50; x += 1) b.data[y * 100 + x] = 220;
    expect(changed(a, b)).toBe(true);
    // A pointer-sized blob where the pointer was is masked out.
    const p = { ...a, data: new Uint8Array(a.data) };
    for (let y = 100; y < 105; y += 1) for (let x = 40; x < 45; x += 1) p.data[y * 100 + x] = 255;
    expect(changed(a, p)).toBe(true);
    expect(changed(a, p, [[0.425, 0.51]])).toBe(false);
    const c = { ...a, data: new Uint8Array(a.data) };
    for (let x = 70; x < 90; x += 1) c.data[4 * 100 + x] = 255; // the clock ticking
    expect(changed(a, c)).toBe(false);
  });

  it('finds a pointer that moved right', () => {
    const blob = (x0: number) => {
      const g = img(100, 200);
      for (let y = 98; y < 105; y += 1) for (let x = x0; x < x0 + 7; x += 1) g.data[y * 100 + x] = 255;
      return g;
    };
    const p = findPointer(blob(50), blob(53), [0.5, 0.5], 0.03);
    expect(p![0]).toBeCloseTo(0.53, 1);
    expect(p![1]).toBeCloseTo(0.505, 2);
    expect(findPointer(blob(50), blob(50), [0.5, 0.5], 0.03)).toBeNull();
  });
});

describe('USB setup', () => {
  const ok = { ssid: 'Home', password: 'password1', host: '192.168.1.5', port: 7300, deskId: 'desk-abc123', keyHex: 'a'.repeat(64) };
  it('validates what goes onto the board', () => {
    expect(validateProvision(ok)).toBeNull();
    expect(validateProvision({ ...ok, ssid: '' })).toMatch(/Wi-Fi name/);
    expect(validateProvision({ ...ok, password: 'short' })).toMatch(/8 characters/);
    expect(validateProvision({ ...ok, password: '' })).toBeNull(); // open network
    expect(validateProvision({ ...ok, host: '' })).toMatch(/IPv4/);
    expect(validateProvision({ ...ok, host: '300.1.1.1' })).toMatch(/IPv4/);
    expect(validateProvision({ ...ok, keyHex: 'zz' })).toMatch(/key/);
  });

  it('finds USB serial ports per platform', () => {
    const list = () => ['cu.usbmodem1101', 'cu.Bluetooth-Incoming-Port', 'ttyACM0', 'tty.usbmodem1101'];
    expect(candidatePorts('darwin', list)).toEqual(['/dev/cu.usbmodem1101']);
    expect(candidatePorts('linux', list)).toEqual(['/dev/ttyACM0']);
    expect(candidatePorts('win32', list)).toEqual([]);
  });

  it('picks LAN IPv4 addresses, skipping loopback, VPN and link-local', () => {
    const v4 = (address: string, internal = false) => ({ address, family: 'IPv4', internal, netmask: '', mac: '', cidr: null }) as never;
    expect(lanAddresses({ lo0: [v4('127.0.0.1', true)], utun3: [v4('10.0.0.2')], en0: [v4('192.168.1.20'), v4('169.254.3.3')], tailscale0: [v4('100.64.0.1')] })).toEqual(['192.168.1.20']);
  });

  it('reports a port that is not a USB Agent', async () => {
    const silent: SerialPort = { writeLine: async () => undefined, readLine: async () => null, close: () => undefined };
    await expect(readIdentity(silent)).rejects.toThrow(/USB Agent firmware/);
  });
});
