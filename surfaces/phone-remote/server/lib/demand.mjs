// demand.mjs — runs the screen capture only while someone wants frames.
// A viewer connection or a /frame poll is demand; with no viewer and no poll
// for `idleMs`, capture stops (and its last frame is dropped, so nobody gets a
// stale screen). Capturing all the time cost a cloud computer ~10 screenshot
// processes a second with nobody watching.

export class CaptureDemand {
  constructor({ capture, idleMs = 15000, checkMs = 5000, now = () => Date.now(), log = console.error } = {}) {
    this.capture = capture;
    this.idleMs = idleMs;
    this.checkMs = checkMs;
    this.now = now;
    this.log = log;
    this.viewers = 0;
    this.lastDemand = 0;
    this.timer = null;
  }

  /** Someone wants frames now: start capture if it is off. */
  demand() {
    if (!this.capture) return;
    this.lastDemand = this.now();
    if (!this.capture.running) {
      this.log('[capture] starting (demand)');
      this.capture.start().catch((err) => this.log(`[capture] start: ${err.message}`));
    }
    if (!this.timer && this.checkMs > 0) {
      this.timer = setInterval(() => this.check(), this.checkMs);
      this.timer.unref?.();
    }
  }

  viewerAttached() {
    this.viewers += 1;
    this.demand();
  }

  viewerDetached() {
    this.viewers = Math.max(0, this.viewers - 1);
    this.lastDemand = this.now();
  }

  /** Stop capture once idle. Returns true when it stopped. */
  check() {
    if (!this.capture?.running) return this.#clear(false);
    if (this.viewers > 0 || this.now() - this.lastDemand < this.idleMs) return false;
    this.log('[capture] stopping (idle)');
    this.capture.stop();
    this.capture.lastFrame = null;
    return this.#clear(true);
  }

  /** Resolve with a frame: the current one, or the first after starting (null on timeout). */
  async frame(timeoutMs = 3000) {
    this.demand();
    if (this.capture?.lastFrame) return this.capture.lastFrame;
    return new Promise((resolve) => {
      const done = (jpeg) => { clearTimeout(t); this.capture.off('frame', done); resolve(jpeg ?? null); };
      const t = setTimeout(() => done(null), timeoutMs);
      this.capture.on('frame', done);
    });
  }

  #clear(result) {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    return result;
  }
}
