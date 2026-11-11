/**
 * Test kit for the Allternit Link composite lane: an iPhone with the Allternit
 * app that shares its screen through a fake node (the network edge), with a
 * USB Agent board in its port (SimBoard over real TCP). Frames are real iCTRL
 * packets (CONFIG, then H.264 access units: an IDR with SPS/PPS, then P
 * slices) through the real AppPhoneViewerSession, PhoneStreamManager and
 * AppFrameKeeper. Only Chromium's H.264 decoder (decodeStill) is faked: it
 * returns the simulated phone's current picture.
 */

import type { SerialPort } from './phone-usb-agent-provision.js';
import type { Calibration } from './phone-usb-agent-pointer.js';
import type { StoredUsbAgent, UsbAgentConfig, UsbAgentStore } from './phone-usb-agent.js';
import { AppPhoneViewerSession, type AppPhoneNode, type AppRpcResult, type AppStreams, type AppViewerSocket, type StillDecoder } from './phone-app-store.js';
import { encodeIosVideoPacket, IOS_PACKET_CONFIG, IOS_PACKET_ENDED, IOS_PACKET_FRAME } from './phone-ios-video.js';
import { PhoneStreamManager } from './phone-stream.js';
import { fakeJpeg, type SimPhone } from './phone-usb-agent.testkit.js';

export const APP_SERIAL = 'app:5b1f0c3e-8d2a-4c11-9a7e-0f6b2d4c9e21';

// SPS (profile 0x42, compat 0xe0, level 0x1f) + PPS + IDR slice; P slices after it.
const idr = (n: number) => Buffer.from([0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0xaa, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88, 0x84, n & 0xff]);
const pSlice = (n: number) => Buffer.from([0, 0, 0, 1, 0x41, 0x9a, n & 0xff]);

interface OpenSocket {
  message?: (d: Buffer | string, bin: boolean) => void;
  close?: (c: number, r: string) => void;
  closed: boolean;
}

/** The app iPhone as the node shows it: its row, its control socket (rpc), and its screen stream. */
export class FakeAppPhone {
  online = true;
  rpcResult: AppRpcResult = { ok: true, result: {} };
  readonly sockets: OpenSocket[] = [];
  decodes = 0;
  private n = 0;
  readonly node: AppPhoneNode;
  readonly streams: AppStreams;
  readonly manager: PhoneStreamManager;

  constructor(readonly phone: SimPhone, readonly serial = APP_SERIAL) {
    this.node = {
      listPhones: async () => [{ serial: this.serial, name: 'Eoj iPhone', model: 'iPhone', online: this.online }],
      rpc: async () => this.rpcResult,
      openViewer: async () => this.openViewer(),
      decidePair: async () => ({ ok: true }),
    };
    this.manager = new PhoneStreamManager({
      createSession: () => new AppPhoneViewerSession({ open: () => this.node.openViewer(this.serial), deviceName: 'Eoj iPhone', startTimeoutMs: 1_000 }),
    });
    const m = this.manager;
    this.streams = {
      start: (s) => m.start(s),
      stop: (s) => m.stop(s),
      status: (s) => m.status(s),
      subscribe: (s, c) => m.subscribe(s, c),
      consumerCount: (s) => m.consumerCount(s),
    };
  }

  /** Chromium's decoder stand-in: the simulated phone's picture right now. */
  readonly decodeStill: StillDecoder = async () => {
    this.decodes += 1;
    return { jpeg: fakeJpeg(this.phone.render()), width: this.phone.width, height: this.phone.height };
  };

  private openViewer(): AppViewerSocket {
    const s: OpenSocket = { closed: false };
    this.sockets.push(s);
    // As the node does on join: CONFIG, then it asks the phone for a keyframe.
    setTimeout(() => {
      this.send(s, IOS_PACKET_CONFIG, { width: 1179, height: 2556, scale: 3, rotation: 0 });
      this.send(s, IOS_PACKET_FRAME, idr(this.n++));
    }, 1);
    return {
      send: () => undefined,
      close: () => {
        if (s.closed) return;
        s.closed = true;
        s.close?.(1000, '');
      },
      onMessage: (cb) => (s.message = cb),
      onClose: (cb) => (s.close = cb),
      onError: () => undefined,
    };
  }

  private send(s: OpenSocket, type: number, payload: Buffer | object): void {
    if (!s.closed) s.message?.(encodeIosVideoPacket(type, payload), true);
  }

  /** The broadcast extension sends the next frame (a keyframe every 30). */
  push(): void {
    const n = this.n++;
    for (const s of this.sockets) this.send(s, IOS_PACKET_FRAME, n % 30 === 0 ? idr(n) : pSlice(n));
  }

  /** The person stops sharing. */
  stopSharing(): void {
    this.online = false;
    for (const s of this.sockets) this.send(s, IOS_PACKET_ENDED, { reason: 'You stopped sharing.' });
  }

  openViewers(): number {
    return this.sockets.filter((s) => !s.closed).length;
  }
}

export function memoryStore(): UsbAgentStore & { boards: StoredUsbAgent[]; cal: Map<string, Calibration> } {
  let config: UsbAgentConfig | null = null;
  const store = {
    boards: [] as StoredUsbAgent[],
    cal: new Map<string, Calibration>(),
    load: () => store.boards,
    save: (b: StoredUsbAgent[]) => {
      store.boards = b;
    },
    loadConfig: () => config,
    saveConfig: (c: UsbAgentConfig) => {
      config = c;
    },
    loadCalibration: (id: string) => store.cal.get(id) ?? null,
    saveCalibration: (id: string, c: Calibration | null) => {
      if (c) store.cal.set(id, structuredClone(c));
      else store.cal.delete(id);
    },
  };
  return store;
}

/** A board on the serial port that speaks the setup protocol. */
export function fakeSerial(boardId: string) {
  const settings: Record<string, string> = {};
  let committed = false;
  const out: string[] = [];
  const port: SerialPort = {
    async writeLine(line) {
      const [, cmd, key, value] = line.split(' ');
      if (cmd === 'info') out.push(`prov info ${boardId} 1.0.0 provisioned=${committed ? 1 : 0}`);
      else if (cmd === 'set') {
        settings[key] = value;
        out.push('prov ok');
      } else if (cmd === 'commit') {
        committed = true;
        out.push('prov ok');
      } else out.push('prov err unknown');
    },
    async readLine() {
      return out.shift() ?? null;
    },
    close() {},
  };
  return { port, settings };
}
