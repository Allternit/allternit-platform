// input.mjs — input injection bridge: line-JSON over stdio to
// input/input_helper.py (Quartz CGEvent HID tap). Client coordinates arrive
// in capture-image pixels; mapping to screen points happens HERE, server-side
// (multiply by display-points / image-pixels). Maps to the future Rust
// `input` module.

import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const HELPER_MAC = join(HERE, '..', '..', 'input', 'input_helper.py');
const HELPER_X11 = join(HERE, '..', '..', 'input', 'input_helper_x11.py');
const HELPER = process.platform === 'darwin' ? HELPER_MAC : HELPER_X11;

export class InputBridge extends EventEmitter {
  constructor({ python = 'python3', helperPath = HELPER, dryRun = false, log = console.error } = {}) {
    super();
    this.python = python;
    this.helperPath = helperPath;
    this.dryRun = dryRun;
    this.log = log;
    this.child = null;
    this.unavailable = null; // why the helper could not start (e.g. 'python3 not found')
    this.pending = [];       // FIFO: helper answers one line per command, in order
    this.ready = null;       // { accessibilityTrusted, dryRun }
    this.display = null;     // { width, height } in screen points
    this.imageSize = null;   // { width, height } in capture-image pixels
  }

  start() {
    const args = [this.helperPath];
    if (this.dryRun) args.push('--dry-run');
    const child = spawn(this.python, args);
    this.child = child;
    // Without this listener a missing python3 (minimal Linux installs) is an
    // uncaught 'error' that kills the whole server, and the desktop then
    // respawned it forever. Input turns off; screen viewing keeps working.
    child.on('error', (err) => {
      if (this.child === child) this.child = null;
      this.unavailable = err.code === 'ENOENT' ? `${this.python} not found` : err.message;
      this.log(`[input] helper could not start (${this.unavailable}): phone input is off, screen viewing still works`);
      const fail = { ok: false, error: `input helper unavailable: ${this.unavailable}` };
      while (this.pending.length) this.pending.shift()(fail);
      this.emit('unavailable', this.unavailable);
    });
    // A write to a helper that never started (or died) must not crash either.
    child.stdin.on('error', (err) => this.log(`[input] helper stdin: ${err.message}`));
    let buf = '';
    child.stdout.on('data', (d) => {
      buf += d;
      let idx;
      while ((idx = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, idx);
        buf = buf.slice(idx + 1);
        let msg;
        try { msg = JSON.parse(line); } catch { continue; }
        if (msg.type === 'ready') {
          this.ready = msg;
          this.log(`[input] helper ready (accessibilityTrusted=${msg.accessibilityTrusted}, dryRun=${msg.dryRun})`);
          this.command({ type: 'displaysize' }).then((r) => {
            if (r.ok) this.display = { width: r.width, height: r.height };
            this.emit('ready', { ...msg, display: this.display });
          });
        } else {
          const resolve = this.pending.shift();
          if (resolve) resolve(msg);
        }
      }
    });
    child.stderr.on('data', (d) => this.log(`[input] helper stderr: ${d}`));
    child.on('exit', (code) => {
      this.log(`[input] helper exited (${code})`);
      this.emit('exit', code);
      const err = { ok: false, error: 'input helper exited' };
      while (this.pending.length) this.pending.shift()(err);
    });
  }

  command(cmd) {
    return new Promise((resolve) => {
      if (!this.child) { resolve({ ok: false, error: 'input helper not running' }); return; }
      this.pending.push(resolve);
      this.child.stdin.write(JSON.stringify(cmd) + '\n');
    });
  }

  // Client events carry x/y in capture-image pixels. Convert to screen points
  // server-side. Non-pointer commands pass through untouched.
  async handleClientEvent(ev) {
    if (!this.imageSize || !this.display) {
      // Not fatal for ping/text; pointer commands need the mapping basis.
      if (['click', 'mousedown', 'mouseup', 'move'].includes(ev.type)) {
        return { ok: false, error: 'coordinate mapping not ready yet' };
      }
    }
    const out = { ...ev };
    if (typeof out.x === 'number' && typeof out.y === 'number' && this.imageSize && this.display
        && this.imageSize.width > 0 && this.imageSize.height > 0) {
      out.x = (out.x / this.imageSize.width) * this.display.width;
      out.y = (out.y / this.imageSize.height) * this.display.height;
    }
    return this.command(out);
  }

  setImageSize(width, height) {
    this.imageSize = { width, height };
  }

  stop() {
    if (this.child) this.child.stdin.end();
    if (this.child) this.child.kill('SIGTERM');
    this.child = null;
  }
}
