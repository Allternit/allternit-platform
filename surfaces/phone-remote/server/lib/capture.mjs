// capture.mjs — screen capture sources producing JPEG frames.
// Default: ScreenCaptureKit helper (sc_capture binary, length-prefixed JPEGs
// on stdout). Fallback: `screencapture -x` in a loop (+ resize_jpeg downscale).
// Emits 'frame' (Buffer) and 'info' ({width, height} = capture image pixels).
// This module boundary maps to the future Rust `capture` module — same
// contract: start() → frames + info, stop().

import { spawn, execFile } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { closeSync, existsSync, mkdtempSync, openSync, readFileSync, readSync, readdirSync, rmSync, statSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { FrameWatchdog } from './watchdog.mjs';

const execFileP = promisify(execFile);
const HERE = dirname(fileURLToPath(import.meta.url));
const SWIFT_SRC = join(HERE, '..', 'capture', 'sc_capture.swift');
const SWIFT_BIN = join(HERE, '..', 'capture', 'sc_capture');
const RESIZE_SRC = join(HERE, '..', 'capture', 'resize_jpeg.swift');
const RESIZE_BIN = join(HERE, '..', 'capture', 'resize_jpeg');

export async function checkScreenRecordingPermission() {
  if (!existsSync(SWIFT_BIN)) await buildSwiftHelper();
  try {
    const { stdout } = await execFileP(SWIFT_BIN, ['--check-permission']);
    return stdout.trim() === 'granted';
  } catch {
    return false; // exit 3 = denied; other errors treated as denied
  }
}

export async function buildSwiftHelper() {
  await execFileP('swiftc', ['-O', '-o', SWIFT_BIN, SWIFT_SRC], { timeout: 180000 });
}

async function ensureResizer() {
  if (!existsSync(RESIZE_BIN)) {
    await execFileP('swiftc', ['-O', '-o', RESIZE_BIN, RESIZE_SRC], { timeout: 180000 });
  }
}

export class Capture extends EventEmitter {
  constructor({ mode = 'sckit', fps = 10, scale = 0.5, quality = 0.6, log = console.error, watchdogMs = 5000 } = {}) {
    super();
    this.mode = mode;
    this.fps = fps;
    this.scale = scale;
    this.quality = quality;
    this.log = log;
    this.child = null;
    this.running = false;
    this.actualMode = null;
    this.lastFrame = null;
    this.lastInfo = { width: 0, height: 0 };
    // In-process sckit restart attempts used (watchdog trip or unexpected
    // child exit). Exactly one is tried before the capture is declared dead
    // — a supervisor restarts the whole server after that.
    this.sckitRestarts = 0;
    // null while alive; string reason once the capture is unrecoverable
    // in-process. Surfaced by /hello (stale/error) and 'fatal' (server exit).
    this.dead = null;
    this.watchdog = new FrameWatchdog({
      staleMs: watchdogMs,
      log,
      onStale: () => this.#onWatchdogTrip(),
    });
  }

  async start() {
    if (this.running) return;
    this.running = true;
    if (this.mode === 'x11' || (this.mode !== 'sckit' && this.mode !== 'screencapture' && process.platform !== 'darwin')) {
      this.#startX11Loop();
      return;
    }
    if (this.mode === 'sckit' && process.platform === 'darwin') {
      try {
        await this.#startSckit();
        return;
      } catch (err) {
        this.log(`[capture] ScreenCaptureKit unavailable: ${err.message} — falling back to screencapture loop`);
      }
    }
    if (process.platform === 'darwin') {
      this.#startScreencaptureLoop();
      return;
    }
    this.#startX11Loop();
  }

  async #startSckit() {
    if (!existsSync(SWIFT_BIN)) {
      this.log('[capture] building sc_capture.swift (first run)…');
      await buildSwiftHelper();
    }
    const child = spawn(SWIFT_BIN, ['--fps', String(this.fps), '--scale', String(this.scale), '--quality', String(this.quality)]);
    this.child = child;
    this.actualMode = 'sckit';

    let buf = Buffer.alloc(0);
    child.stdout.on('data', (d) => {
      buf = Buffer.concat([buf, d]);
      for (;;) {
        if (buf.length < 4) break;
        const n = buf.readUInt32BE(0);
        if (buf.length < 4 + n) break;
        const jpeg = buf.subarray(4, 4 + n);
        this.lastFrame = jpeg;
        this.emit('frame', jpeg);
        this.watchdog.reset();
        buf = buf.subarray(4 + n);
      }
    });
    let errBuf = '';
    child.stderr.on('data', (d) => {
      errBuf += d;
      let idx;
      while ((idx = errBuf.indexOf('\n')) >= 0) {
        const line = errBuf.slice(0, idx);
        errBuf = errBuf.slice(idx + 1);
        try {
          const msg = JSON.parse(line);
          if (msg.type === 'display') {
            this.lastInfo = { width: msg.width, height: msg.height };
            this.emit('info', this.lastInfo);
          }
          else if (msg.type === 'started') {
            this.log(`[capture] sckit started ${msg.captureWidth}x${msg.captureHeight} @${msg.fps}fps`);
            // Frames follow within one frame period of 'started'; from here a
            // silent gap means the helper hung.
            this.watchdog.arm();
          }
          else if (msg.type === 'error') this.emit('captureError', msg);
        } catch {
          this.log(`[capture] helper: ${line}`);
        }
      }
    });
    child.on('exit', (code) => {
      // An in-process restart (#restartSckit) supersedes this child before its
      // exit event lands — the new child owns the watchdog and the running
      // flag. A stale exit must not disarm the new child's watchdog or count
      // as a second death.
      if (this.child !== child) return;
      this.watchdog.disarm();
      if (!this.running) return; // stop() already tore us down
      this.running = false;
      // 3 = Screen Recording TCC denied — surfaced, never faked.
      this.emit('exit', code);
      if (this.actualMode === 'sckit' && code === 3) {
        this.log('[capture] Screen Recording permission denied for this process tree — see README TCC runbook; falling back to screencapture loop');
        this.running = true;
        this.#startScreencaptureLoop();
        return;
      }
      if (this.actualMode === 'sckit') {
        this.#restartSckit(`sc_capture exited unexpectedly (code ${code})`);
      }
    });
  }

  /** One in-process sckit restart after a watchdog trip or unexpected exit. */
  #restartSckit(reason) {
    if (this.dead) return;
    if (this.sckitRestarts >= 1) {
      this.#die(`${reason}; in-process restart already attempted`);
      return;
    }
    this.sckitRestarts += 1;
    this.log(`[capture] ${reason} — attempting in-process sckit restart (${this.sckitRestarts}/1)`);
    // The frozen-helper case needs a hard kill; SIGTERM can hang on a wedged
    // ScreenCaptureKit stream.
    try { this.child?.kill('SIGKILL'); } catch { /* already gone */ }
    this.child = null;
    this.running = true;
    this.#startSckit().catch((err) => this.#die(`sckit restart failed: ${err.message}`));
  }

  #onWatchdogTrip() {
    if (!this.running || this.dead) return;
    this.#restartSckit(`frozen: no frame for >${this.watchdog.staleMs}ms (sckit)`);
  }

  #die(reason) {
    if (this.dead) return;
    this.dead = reason;
    this.watchdog.disarm();
    this.log(`[capture] FATAL: ${reason}`);
    // The server exits non-zero on 'fatal' so a supervisor (the desktop app)
    // restarts the whole process — in-process recovery is exhausted.
    this.emit('fatal', reason);
  }

  #startX11Loop() {
    this.actualMode = 'x11';
    const display = process.env.DISPLAY || ':0';
    const tmp = join(tmpdir(), `phone-remote-${process.pid}.jpg`);
    this.log(`[capture] X11 loop on DISPLAY=${display} (target ~${this.fps}fps)`);
    this.emit('info', { width: 0, height: 0 });
    // Look for ffmpeg once: trying it on every frame doubled the processes per frame where it is missing.
    let hasFfmpeg = null;
    const grab = async () => {
      if (hasFfmpeg === null) hasFfmpeg = await execFileP('ffmpeg', ['-version'], { timeout: 4000 }).then(() => true, () => false);
      try {
        if (!hasFfmpeg) throw new Error('no ffmpeg');
        await execFileP('ffmpeg', [
          '-y', '-loglevel', 'error',
          '-f', 'x11grab', '-video_size', '1280x720', '-i', `${display}.0`,
          '-frames', '1', '-q:v', String(Math.max(2, Math.round((1 - this.quality) * 30))),
          tmp,
        ], { timeout: 4000 });
        return true;
      } catch {
        try {
          await execFileP('scrot', ['-o', tmp], { timeout: 4000, env: { ...process.env, DISPLAY: display } });
          return true;
        } catch {
          return false;
        }
      }
    };
    const tick = async () => {
      if (!this.running) return;
      const t0 = performance.now();
      try {
        if (await grab()) {
          const frame = readFileSync(tmp);
          this.lastFrame = frame;
          this.emit('frame', frame);
        }
      } catch (err) {
        this.emit('captureError', { type: 'error', error: `x11 capture: ${err.message}` });
      }
      const wait = Math.max(0, 1000 / this.fps - (performance.now() - t0));
      this.timer = setTimeout(tick, wait);
    };
    tick();
  }

  async #startScreencaptureLoop() {
    this.actualMode = 'screencapture';
    const dir = mkdtempSync(join(tmpdir(), `phone-remote-${process.pid}-`));
    const tmp = join(dir, 'capture.jpg');
    const small = join(dir, 'small.jpg');
    // Downscale with our own helper, not sips: sips orphans one UUID-named
    // JPEG intermediate per invocation in the per-user temp root (confstr
    // _CS_DARWIN_USER_TEMP_DIR — a private TMPDIR does not redirect it),
    // which at ~8fps is ~2.4 MB/s and filled a 512 GB disk in days.
    try {
      await ensureResizer();
    } catch (err) {
      this.log(`[capture] resize_jpeg build failed: ${err.message} — sending full-res frames`);
    }
    const canResize = existsSync(RESIZE_BIN);
    // Belt-and-braces: if any sips (an old build, another tool) still drops
    // UUID orphans in the temp root, sweep them. JPEG magic + age floor keep
    // other apps' files and in-flight writes out of the blast radius.
    const tempRoot = tmpdir();
    const isOrphan = (name) => /^[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}$/i.test(name);
    const sweepOrphans = () => {
      const now = Date.now();
      for (const name of readdirSync(tempRoot)) {
        if (!isOrphan(name)) continue;
        const p = join(tempRoot, name);
        try {
          if (now - statSync(p).mtimeMs <= 10000) continue;
          const fd = openSync(p, 'r');
          const head = Buffer.alloc(3);
          readSync(fd, head, 0, 3, 0);
          closeSync(fd);
          if (head[0] === 0xff && head[1] === 0xd8 && head[2] === 0xff) unlinkSync(p);
        } catch { /* raced with an in-flight write or not a file */ }
      }
    };
    this.log(`[capture] screencapture loop started (target ~${this.fps}fps)`);
    this.emit('info', { width: 0, height: 0 }); // unknown until first frame; client learns from JPEG itself
    let frames = 0;
    const tick = async () => {
      if (!this.running) return;
      const t0 = performance.now();
      try {
        await execFileP('screencapture', ['-x', '-t', 'jpg', tmp], { timeout: 5000 });
        let frame;
        if (canResize) {
          await execFileP(RESIZE_BIN, [tmp, small, '1280', String(Math.round(this.quality * 100))], { timeout: 5000 });
          frame = readFileSync(small);
        } else {
          frame = readFileSync(tmp);
        }
        this.lastFrame = frame;
        this.emit('frame', frame);
        if (++frames % 50 === 0) sweepOrphans();
      } catch (err) {
        this.emit('captureError', { type: 'error', error: `screencapture: ${err.message}` });
      }
      const elapsed = performance.now() - t0;
      const wait = Math.max(0, 1000 / this.fps - elapsed);
      this.timer = setTimeout(tick, wait);
    };
    tick();
    this.once('stop', () => { rmSync(dir, { recursive: true, force: true }); });
  }

  stop() {
    this.running = false;
    this.watchdog.disarm();
    if (this.timer) clearTimeout(this.timer);
    if (this.child) this.child.kill('SIGTERM');
    this.child = null;
    this.emit('stop');
  }
}
