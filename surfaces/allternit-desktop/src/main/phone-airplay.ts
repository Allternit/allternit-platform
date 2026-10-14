/**
 * The iPhone's screen, mirrored to this computer over AirPlay by UxPlay.
 *
 * UxPlay (GPL-3.0, github.com/FDH2/UxPlay) runs as its own process; we never
 * link or ship its code. Its GStreamer video sink writes JPEG frames into a
 * pipe on fd 4 (fd 3 is spawnSidecar's lifeline), and we keep the newest one. The approach follows
 * esp32-iphone-agent (MIT, Henry Yuan), see hardware/usb-agent/NOTICE.
 *
 * UxPlay is started on its own ports so it doesn't collide with macOS's
 * built-in AirPlay Receiver, and it is restarted whenever it exits (it does
 * on some disconnects).
 *
 * Only a phone that has entered this computer's AirPlay PIN can mirror to it
 * (`-pin`, remembered in our own `-reg` file). Without that, anyone on the
 * Wi-Fi could mirror their own screen to "Allternit" and feed the bot fake
 * frames.
 */

import type { ChildProcess } from 'node:child_process';
import { EventEmitter } from 'node:events';
import * as fs from 'node:fs';
import * as path from 'node:path';
import type { Readable } from 'node:stream';

export const AIRPLAY_NAME = 'Allternit';
/** UxPlay uses this port and the next two (TCP and UDP). Clear of macOS's AirPlay Receiver on 7000. */
export const AIRPLAY_BASE_PORT = 47_100;
const MAX_FRAME_BYTES = 8 * 1024 * 1024;
/** The frame pipe's fd in UxPlay. spawnSidecar owns fd 3. */
export const FRAME_FD = 4;

/** Pulls whole JPEGs (FFD8 … FFD9) out of a byte stream. */
export class JpegSplitter {
  private buf: Buffer = Buffer.alloc(0);
  constructor(private readonly onFrame: (jpeg: Buffer) => void) {}

  push(chunk: Buffer): void {
    this.buf = this.buf.length ? Buffer.concat([this.buf, chunk]) : chunk;
    for (;;) {
      const start = this.buf.indexOf(SOI);
      if (start < 0) {
        this.buf = this.buf.subarray(Math.max(0, this.buf.length - 1));
        return;
      }
      const end = this.buf.indexOf(EOI, start + 2);
      if (end < 0) {
        this.buf = this.buf.subarray(start);
        if (this.buf.length > MAX_FRAME_BYTES) this.buf = Buffer.alloc(0); // garbage, not a frame
        return;
      }
      this.onFrame(Buffer.from(this.buf.subarray(start, end + 2)));
      this.buf = this.buf.subarray(end + 2);
    }
  }
}
const SOI = Buffer.from([0xff, 0xd8]);
const EOI = Buffer.from([0xff, 0xd9]);

/** Width and height from a baseline or progressive JPEG header, or null. */
export function jpegSize(jpeg: Buffer): { width: number; height: number } | null {
  let i = 2;
  while (i + 9 < jpeg.length) {
    if (jpeg[i] !== 0xff) return null;
    const marker = jpeg[i + 1];
    const len = jpeg.readUInt16BE(i + 2);
    if (marker >= 0xc0 && marker <= 0xc3) {
      return { height: jpeg.readUInt16BE(i + 5), width: jpeg.readUInt16BE(i + 7) };
    }
    i += 2 + len;
  }
  return null;
}

export interface UxplayOptions {
  name: string;
  basePort: number;
  fps: number;
  quality: number;
  /** Four digits the phone enters once. */
  pin: string;
  /** Register of phones that entered the PIN. */
  registerFile: string;
}

export function isAirplayPin(pin: string): boolean {
  return /^[0-9]{4}$/.test(pin);
}

export function uxplayArgs(o: UxplayOptions): string[] {
  if (!isAirplayPin(o.pin)) throw new Error('AirPlay PIN must be four digits.');
  const sink = `videorate drop-only=true ! video/x-raw,framerate=${o.fps}/1 ! jpegenc quality=${o.quality} ! fdsink fd=${FRAME_FD}`;
  // -nh: no "@hostname" suffix on the name; -a: no audio; -reset 0: never drop a quiet client.
  return ['-n', o.name, '-nh', '-a', '-reset', '0', '-p', String(o.basePort), '-pin', o.pin, '-reg', o.registerFile, '-vs', sink];
}

export function findUxplay(env: NodeJS.ProcessEnv = process.env): string | null {
  const candidates = [
    env.ALLTERNIT_UXPLAY_PATH,
    ...(env.PATH ?? '').split(path.delimiter).filter(Boolean).map((d) => path.join(d, 'uxplay')),
    '/opt/homebrew/bin/uxplay',
    '/usr/local/bin/uxplay',
    '/usr/bin/uxplay',
  ];
  return candidates.find((c) => c && fs.existsSync(c)) ?? null;
}

export interface Frame {
  jpeg: Buffer;
  width: number;
  height: number;
  at: number;
}

export type AirplayState = 'unavailable' | 'stopped' | 'waiting' | 'mirroring';

export interface AirplayDeps {
  /** Spawns uxplay with FRAME_FD as a readable pipe; null when uxplay isn't installed. */
  spawn: ((args: string[]) => ChildProcess) | null;
  now?: () => number;
  restartDelayMs?: number;
}

/** No frame for this long (AirPlay only sends on change) and the mirror is treated as gone for status. */
export const MIRROR_IDLE_MS = 90_000;

export class AirplayScreen extends EventEmitter {
  private proc: ChildProcess | null = null;
  private frame: Frame | null = null;
  private stopping = true;
  private restartTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly now: () => number;

  constructor(private readonly deps: AirplayDeps, private readonly opts: UxplayOptions) {
    super();
    this.now = deps.now ?? Date.now;
  }

  available(): boolean {
    return this.deps.spawn !== null;
  }

  receiverName(): string {
    return this.opts.name;
  }

  pin(): string {
    return this.opts.pin;
  }

  start(): void {
    if (!this.deps.spawn || !this.stopping) return;
    this.stopping = false;
    this.launch();
  }

  stop(): void {
    this.stopping = true;
    if (this.restartTimer) clearTimeout(this.restartTimer);
    this.restartTimer = null;
    if (this.proc && this.proc.exitCode === null) this.proc.kill('SIGTERM');
    this.proc = null;
    this.frame = null;
  }

  state(): AirplayState {
    if (!this.deps.spawn) return 'unavailable';
    if (this.stopping || !this.proc) return 'stopped';
    return this.frame && this.now() - this.frame.at < MIRROR_IDLE_MS ? 'mirroring' : 'waiting';
  }

  latest(): Frame | null {
    return this.frame;
  }

  /** The first frame newer than `since`, or null after `timeoutMs`. */
  nextFrame(since: number, timeoutMs: number): Promise<Frame | null> {
    if (this.frame && this.frame.at > since) return Promise.resolve(this.frame);
    return new Promise((resolve) => {
      const onFrame = (f: Frame) => {
        if (f.at > since) {
          clearTimeout(timer);
          this.off('frame', onFrame);
          resolve(f);
        }
      };
      const timer = setTimeout(() => {
        this.off('frame', onFrame);
        resolve(null);
      }, timeoutMs);
      this.on('frame', onFrame);
    });
  }

  /** Waits until no new frame has arrived for `quietMs` (AirPlay sends frames only when the screen changes). */
  async settled(quietMs = 350, leastMs = 250, mostMs = 4_000): Promise<void> {
    const started = this.now();
    await sleep(leastMs);
    while (this.frame && this.now() - this.frame.at < quietMs && this.now() - started < mostMs) await sleep(50);
  }

  private launch(): void {
    if (!this.deps.spawn || this.stopping) return;
    const child = this.deps.spawn(uxplayArgs(this.opts));
    this.proc = child;
    const pipe = child.stdio[FRAME_FD] as Readable | null | undefined;
    const splitter = new JpegSplitter((jpeg) => {
      const size = jpegSize(jpeg);
      if (!size) return;
      this.frame = { jpeg, ...size, at: this.now() };
      this.emit('frame', this.frame);
    });
    pipe?.on('data', (chunk: Buffer) => splitter.push(chunk));
    pipe?.on('error', () => undefined);
    child.once('error', (error) => this.emit('error', error));
    child.once('exit', (code) => {
      if (this.proc !== child) return;
      this.proc = null;
      this.frame = null;
      this.emit('exit', code);
      if (!this.stopping) {
        this.restartTimer = setTimeout(() => this.launch(), this.deps.restartDelayMs ?? 1_000);
        this.restartTimer.unref?.();
      }
    });
    this.emit('started');
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
