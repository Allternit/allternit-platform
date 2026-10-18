import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { callBridgeMissing, DEFAULT_CALL_BRIDGE, pickAudioDevice, sanitizeCallBridge, type CallBridgeConfig } from './phone-call-bridge-config.js';
import { PhoneDeviceManager, type PhoneStore, type StoredPhone } from './phone-device-manager.js';
import { fileLinkStore, findIosLinkRecord, setIosCallBridge, type IosLinkRecord } from './phone-link-ios.js';
import { fileStore } from './phone-store-file.js';
import type { AdbCall } from './phone-tools.js';

const CFG: CallBridgeConfig = {
  enabled: true,
  botId: 'bot_1',
  inputDeviceId: 'in-1',
  inputLabel: 'USB Audio Device',
  outputDeviceId: 'out-1',
  outputLabel: 'USB Audio Device',
};

const dirs: string[] = [];
function tmp(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'call-bridge-cfg-'));
  dirs.push(dir);
  return dir;
}
afterEach(() => {
  for (const d of dirs.splice(0)) fs.rmSync(d, { recursive: true, force: true });
});

const noAdb = (async () => ({ stdout: '', stderr: '', code: 0 })) as unknown as AdbCall;
noAdb.binary = async () => Buffer.alloc(0);

const MOTO: StoredPhone = { id: 'ZY22ABC', name: 'Moto', host: '192.168.1.20', port: 37000, pairedAt: 1 };

describe('sanitizeCallBridge', () => {
  it('keeps the known fields and drops junk', () => {
    expect(sanitizeCallBridge({ ...CFG, extra: 'x', enabled: 'yes' })).toEqual({ ...CFG, enabled: false });
    expect(sanitizeCallBridge(null)).toEqual(DEFAULT_CALL_BRIDGE);
    expect(sanitizeCallBridge({ botId: '   ', inputLabel: 7 })).toEqual(DEFAULT_CALL_BRIDGE);
  });
});

describe('callBridgeMissing', () => {
  it('names what is still needed', () => {
    expect(callBridgeMissing(null)).toMatch(/off/);
    expect(callBridgeMissing({ ...CFG, botId: null })).toMatch(/bot/);
    expect(callBridgeMissing({ ...CFG, inputDeviceId: null, inputLabel: null })).toMatch(/input/);
    expect(callBridgeMissing({ ...CFG, outputDeviceId: null, outputLabel: null })).toMatch(/output/);
    expect(callBridgeMissing(CFG)).toBeNull();
  });
});

describe('pickAudioDevice', () => {
  const devices = [
    { kind: 'audioinput', deviceId: 'default', label: 'Default - MacBook Pro Microphone' },
    { kind: 'audioinput', deviceId: 'a1', label: 'MacBook Pro Microphone' },
    { kind: 'audioinput', deviceId: 'a2', label: 'USB Audio Device' },
    { kind: 'audiooutput', deviceId: 'b1', label: 'MacBook Pro Speakers' },
    { kind: 'audiooutput', deviceId: 'b2', label: 'USB Audio Device' },
  ];

  it('an exact id wins', () => {
    expect(pickAudioDevice(devices, 'audioinput', 'a2', 'something else')?.deviceId).toBe('a2');
  });

  it('falls back to the label (ids are salted per origin)', () => {
    expect(pickAudioDevice(devices, 'audiooutput', 'app-origin-id', 'USB Audio Device')?.deviceId).toBe('b2');
    expect(pickAudioDevice(devices, 'audioinput', null, 'Default - USB Audio Device')?.deviceId).toBe('a2');
  });

  it('never picks some other device when the cable is missing', () => {
    expect(pickAudioDevice(devices, 'audioinput', 'gone', 'Scarlett 2i2')).toBeNull();
    expect(pickAudioDevice(devices, 'audiooutput', null, null)).toBeNull();
    // "default" is a moving alias, never the chosen cable side.
    expect(pickAudioDevice(devices, 'audioinput', 'default', null)).toBeNull();
  });
});

describe('Android: callBridge on StoredPhone', () => {
  it('persists through phones.json and survives a restart', () => {
    const file = path.join(tmp(), 'phones.json');
    const store: PhoneStore = fileStore(file);
    store.save([MOTO]);
    const manager = new PhoneDeviceManager({ adb: noAdb, store, spawnScrcpy: null });
    expect(manager.getCallBridge('ZY22ABC')).toBeNull();
    // By adb serial (what the UI holds) or stored id.
    expect(manager.setCallBridge('192.168.1.20:37000', CFG)).toBe(true);
    expect(JSON.parse(fs.readFileSync(file, 'utf8'))[0].callBridge).toEqual(CFG);

    const again = new PhoneDeviceManager({ adb: noAdb, store: fileStore(file), spawnScrcpy: null });
    expect(again.getCallBridge('ZY22ABC')).toEqual(CFG);
    expect(again.list()[0].name).toBe('Moto');
  });

  it('refuses an unknown phone', () => {
    const manager = new PhoneDeviceManager({ adb: noAdb, store: fileStore(path.join(tmp(), 'p.json')), spawnScrcpy: null });
    expect(manager.setCallBridge('nope', CFG)).toBe(false);
  });
});

describe('iPhone: callBridge on IosLinkRecord', () => {
  const RECORD: IosLinkRecord = {
    udid: '00008110-AAAA',
    deviceId: 'ictrl-dev-1',
    deviceName: 'Eoj iPhone',
    runnerBundleId: 'r',
    hostBundleId: 'h',
    xctestConfig: 'x',
    testToRun: 't',
    linkedAt: '2026-10-18T00:00:00.000Z',
  };

  it('finds the record by udid, device id or link: serial and persists', () => {
    const file = path.join(tmp(), 'phone-link-ios.json');
    const store = fileLinkStore(file);
    store.save(RECORD);
    expect(findIosLinkRecord(store, 'link:ictrl-dev-1')?.udid).toBe(RECORD.udid);
    expect(findIosLinkRecord(store, RECORD.udid)?.udid).toBe(RECORD.udid);
    expect(setIosCallBridge(store, 'link:ictrl-dev-1', CFG)).toBe(true);
    expect(fileLinkStore(file).load(RECORD.udid)?.callBridge).toEqual(CFG);
    expect(setIosCallBridge(store, 'link:other', CFG)).toBe(false);
  });

  it('a writer that rebuilds the record (relink, renewal) keeps Call audio', () => {
    const store = fileLinkStore(path.join(tmp(), 'phone-link-ios.json'));
    store.save({ ...RECORD, callBridge: CFG });
    store.save({ ...RECORD, lanHost: '192.168.1.40:8200' });
    expect(store.load(RECORD.udid)).toMatchObject({ lanHost: '192.168.1.40:8200', callBridge: CFG });
    // An explicit new config replaces it.
    store.save({ ...RECORD, callBridge: { ...CFG, enabled: false } });
    expect(store.load(RECORD.udid)?.callBridge?.enabled).toBe(false);
  });
});
