/**
 * Test kit for the USB Agent lane: a simulated board that dials the real
 * link over TCP, and a simulated iPhone whose pointer moves the way iOS
 * AssistiveTouch does (relative moves with a gain the calibration has to
 * discover). No hardware, no UxPlay.
 */

import { EventEmitter } from 'node:events';
import * as net from 'node:net';
import { PassThrough } from 'node:stream';
import type { ChildProcess } from 'node:child_process';
import { LineReader } from './phone-usb-agent-link.js';
import { ReferenceBoard } from './phone-usb-agent-protocol.js';
import { FRAME_FD } from './phone-airplay.js';
import type { Gray } from './phone-usb-agent-vision.js';

// ── fake frames: a JPEG-shaped envelope around an id the fake decoder looks up ──

const frames = new Map<number, Gray>();
let nextId = 1;

export function fakeJpeg(img: Gray): Buffer {
  const id = nextId++;
  frames.set(id, img);
  const b = Buffer.alloc(2 + 2 + 17 + 2);
  b.set([0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08], 0);
  b.writeUInt16BE(img.height, 7);
  b.writeUInt16BE(img.width, 9);
  b.set([(id >> 21) & 0x7f, (id >> 14) & 0x7f, (id >> 7) & 0x7f, id & 0x7f], 11);
  b.set([0xff, 0xd9], b.length - 2);
  return b;
}

export function fakeDecode(jpeg: Buffer): Gray | null {
  const id = (jpeg[11] << 21) | (jpeg[12] << 14) | (jpeg[13] << 7) | jpeg[14];
  return frames.get(id) ?? null;
}

// ── simulated iPhone ────────────────────────────────────────────────────────

export class SimPhone {
  readonly width = 120;
  readonly height = 260;
  /** Pointer in pixels. */
  px = 60;
  py = 130;
  pressed = false;
  taps: Array<[number, number]> = [];
  typed = '';
  checked = false;
  /** Mouse units to cross the screen, as iOS would map them (unknown to Desktop). */
  constructor(readonly spanW = 900, readonly spanH = 1950, readonly gain = 1.0) {}

  button = { x0: 70, y0: 120, x1: 110, y1: 140 };

  apply(command: string): void {
    const [op, ...rest] = command.split(' ');
    if (op === 'home') {
      this.px = 0;
      this.py = 0;
    } else if (op === 'move') {
      const [dx, dy] = rest.map(Number);
      this.px = clamp(this.px + (dx * this.width * this.gain) / this.spanW, 0, this.width - 1);
      this.py = clamp(this.py + (dy * this.height * this.gain) / this.spanH, 0, this.height - 1);
    } else if (op === 'click') {
      this.taps.push([this.px, this.py]);
      const b = this.button;
      if (this.px >= b.x0 && this.px <= b.x1 && this.py >= b.y0 && this.py <= b.y1) this.checked = !this.checked;
    } else if (op === 'press') this.pressed = true;
    else if (op === 'release') this.pressed = false;
    else if (op === 'type') this.typed += Buffer.from(rest[0], 'hex').toString('ascii');
  }

  render(): Gray {
    const data = new Uint8Array(this.width * this.height).fill(40);
    const fill = (x0: number, y0: number, x1: number, y1: number, v: number) => {
      for (let y = Math.max(0, y0); y < Math.min(this.height, y1); y += 1) for (let x = Math.max(0, x0); x < Math.min(this.width, x1); x += 1) data[y * this.width + x] = v;
    };
    const b = this.button;
    fill(b.x0, b.y0, b.x1, b.y1, 120);
    if (this.checked) fill(10, 200, 30, 220, 220);
    const x = Math.round(this.px);
    const y = Math.round(this.py);
    fill(x - 2, y - 2, x + 3, y + 3, 255);
    return { width: this.width, height: this.height, data };
  }
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

// ── fake UxPlay ─────────────────────────────────────────────────────────────

export class FakeUxplay extends EventEmitter {
  exitCode: number | null = null;
  readonly frames = new PassThrough();
  readonly stdio: Array<unknown> = [];
  readonly args: string[];
  constructor(args: string[]) {
    super();
    this.args = args;
    this.stdio[FRAME_FD] = this.frames;
  }
  write(jpeg: Buffer): void {
    this.frames.write(jpeg);
  }
  kill(): boolean {
    if (this.exitCode === null) {
      this.exitCode = 0;
      setImmediate(() => this.emit('exit', 0));
    }
    return true;
  }
}

export function fakeUxplaySpawner() {
  const spawned: FakeUxplay[] = [];
  const spawn = (args: string[]) => {
    const p = new FakeUxplay(args);
    spawned.push(p);
    return p as unknown as ChildProcess;
  };
  return { spawn, spawned };
}

// ── simulated board over TCP ────────────────────────────────────────────────

export class SimBoard {
  readonly ref: ReferenceBoard;
  socket: net.Socket | null = null;
  /** Called after each executed command, e.g. to push a new frame. */
  onCommand: (command: string) => void = () => undefined;
  /** When true, the board swallows commands without answering (timeout path). */
  mute = false;

  constructor(boardId: string, keyHex: string, readonly phone: SimPhone | null = null) {
    this.ref = new ReferenceBoard(boardId, keyHex);
  }

  connect(port: number): Promise<void> {
    return new Promise((resolve, reject) => {
      const socket = net.connect(port, '127.0.0.1', () => {
        socket.write(`${this.ref.hello()}\n`);
      });
      this.socket = socket;
      const reader = new LineReader(
        (line) => {
          if (line === 'ready') resolve();
          const before = this.ref.log.length;
          if (this.mute && line.startsWith('c ')) return;
          const out = this.ref.receive(line);
          if (out === 'drop') {
            socket.destroy();
            return;
          }
          for (const l of out) socket.write(`${l}\n`);
          if (this.ref.log.length > before) {
            const cmd = this.ref.log[this.ref.log.length - 1];
            this.phone?.apply(cmd);
            this.onCommand(cmd);
          }
        },
        () => socket.destroy(),
      );
      socket.on('data', (c) => reader.push(c));
      socket.on('error', reject);
      socket.on('close', () => reject(new Error('closed before ready')));
    });
  }

  /** Simulates pressing the arm button. */
  arm(armed = true): void {
    this.ref.armed = armed;
    this.socket?.write(`s armed=${armed ? 1 : 0} usb=${this.ref.usb}\n`);
  }

  setUsb(usb: 'mounted' | 'asleep' | 'none'): void {
    this.ref.usb = usb;
    this.socket?.write(`s armed=${this.ref.armed ? 1 : 0} usb=${usb}\n`);
  }

  close(): void {
    this.socket?.destroy();
  }
}

export const until = async (cond: () => boolean, ms = 2_000) => {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error('timed out waiting');
    await new Promise((r) => setTimeout(r, 5));
  }
};
