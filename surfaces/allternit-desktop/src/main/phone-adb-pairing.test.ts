import { describe, expect, it } from 'vitest';
import {
  buildQrPayload,
  connectSucceeded,
  escapeQrField,
  findConnectService,
  findPairingService,
  generateQrPairing,
  isSixDigitCode,
  pairSucceeded,
  parseAdbDevices,
  parseHostPort,
  parseMdnsServices,
} from './phone-adb-pairing.js';

describe('QR payload', () => {
  it('matches the Android "Pair device with QR code" grammar', () => {
    expect(buildQrPayload('allternit-ab12CD34', 'Zx9kQ2mP7wLs')).toBe('WIFI:T:ADB;S:allternit-ab12CD34;P:Zx9kQ2mP7wLs;;');
  });

  it('escapes reserved characters', () => {
    expect(escapeQrField('a;b,c:d"e\\f')).toBe('a\\;b\\,c\\:d\\"e\\\\f');
  });

  it('generates a fresh name and code each time, with the payload built from them', () => {
    const a = generateQrPairing();
    const b = generateQrPairing();
    expect(a.name).toMatch(/^allternit-[A-Za-z0-9]{8}$/);
    expect(a.code).toMatch(/^[A-Za-z0-9]{12}$/);
    expect(a.payload).toBe(buildQrPayload(a.name, a.code));
    expect(a.code).not.toBe(b.code);
    expect(a.name).not.toBe(b.name);
  });
});

describe('mDNS parsing', () => {
  const output = [
    'List of discovered mdns services',
    'adb-R5CT1234-abcdef\t_adb-tls-connect._tcp\t192.168.1.20:41234',
    'allternit-ab12CD34\t_adb-tls-pairing._tcp\t192.168.1.20:37123',
    'adb-OTHER\t_adb-tls-connect._tcp\t192.168.1.99:40000',
    'garbage line',
    '',
  ].join('\n');

  it('parses services and ignores noise', () => {
    const services = parseMdnsServices(output);
    expect(services).toHaveLength(3);
    expect(services[0]).toEqual({ name: 'adb-R5CT1234-abcdef', type: '_adb-tls-connect._tcp', host: '192.168.1.20', port: 41234 });
  });

  it('finds the pairing service by our QR name only', () => {
    const services = parseMdnsServices(output);
    expect(findPairingService(services, 'allternit-ab12CD34')?.port).toBe(37123);
    expect(findPairingService(services, 'allternit-other')).toBeUndefined();
  });

  it('finds the connect port by host (it changes per toggle)', () => {
    const services = parseMdnsServices(output);
    expect(findConnectService(services, '192.168.1.20')?.port).toBe(41234);
    expect(findConnectService(services, '10.0.0.1')).toBeUndefined();
  });

  it('parses host:port including bad input', () => {
    expect(parseHostPort('192.168.1.20:37123')).toEqual({ host: '192.168.1.20', port: 37123 });
    expect(parseHostPort('192.168.1.20')).toBeNull();
    expect(parseHostPort('192.168.1.20:99999')).toBeNull();
    expect(parseHostPort('; rm -rf /:1')).toBeNull();
  });
});

describe('adb output parsing', () => {
  it('parses adb devices -l', () => {
    const rows = parseAdbDevices(
      [
        'List of devices attached',
        '192.168.1.20:41234     device product:x model:Pixel_8 device:y transport_id:3',
        'R5CT1234               device usb:1-1 model:SM_G991B',
        '192.168.1.30:5555      unauthorized',
        '192.168.1.31:5555      offline',
        '* daemon started successfully',
      ].join('\n'),
    );
    expect(rows).toHaveLength(4);
    expect(rows[0]).toMatchObject({ serial: '192.168.1.20:41234', state: 'device', model: 'Pixel_8', wireless: true });
    expect(rows[1].wireless).toBe(false);
    expect(rows[2].state).toBe('unauthorized');
    expect(rows[3].state).toBe('offline');
  });

  it('recognises pair/connect outcomes', () => {
    expect(pairSucceeded('Successfully paired to 192.168.1.20:37123 [guid=adb-X]')).toBe(true);
    expect(pairSucceeded('Failed: Wrong password or connection was dropped.')).toBe(false);
    expect(connectSucceeded('connected to 192.168.1.20:41234')).toBe(true);
    expect(connectSucceeded('already connected to 192.168.1.20:41234')).toBe(true);
    expect(connectSucceeded('failed to connect to 192.168.1.20:41234')).toBe(false);
    expect(connectSucceeded('cannot connect to 192.168.1.20:41234: Connection refused')).toBe(false);
  });

  it('validates six-digit codes', () => {
    expect(isSixDigitCode('123456')).toBe(true);
    expect(isSixDigitCode('12345')).toBe(false);
    expect(isSixDigitCode('12345a')).toBe(false);
  });
});
