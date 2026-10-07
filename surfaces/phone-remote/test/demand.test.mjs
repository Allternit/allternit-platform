// node test/demand.test.mjs
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { CaptureDemand } from '../server/lib/demand.mjs';

class FakeCapture extends EventEmitter {
  constructor() { super(); this.running = false; this.starts = 0; this.lastFrame = null; }
  async start() { this.running = true; this.starts += 1; }
  stop() { this.running = false; }
}

let t = 0;
const now = () => t;
const quiet = () => {};

{ // Nothing captures until someone asks.
  const cap = new FakeCapture();
  const d = new CaptureDemand({ capture: cap, idleMs: 15000, checkMs: 0, now, log: quiet });
  assert.equal(cap.running, false);
  d.demand();
  assert.equal(cap.running, true);
  assert.equal(cap.starts, 1);
  d.demand();
  assert.equal(cap.starts, 1, 'a second demand does not restart');
}

{ // Polls keep it on; idle stops it and drops the stale frame.
  t = 0;
  const cap = new FakeCapture();
  const d = new CaptureDemand({ capture: cap, idleMs: 15000, checkMs: 0, now, log: quiet });
  d.demand();
  cap.lastFrame = Buffer.from('jpeg');
  t = 10000; assert.equal(d.check(), false);
  d.demand();
  t = 24000; assert.equal(d.check(), false, 'last poll 14 s ago');
  t = 26000; assert.equal(d.check(), true, 'idle 16 s: stopped');
  assert.equal(cap.running, false);
  assert.equal(cap.lastFrame, null, 'no stale frame after stopping');
}

{ // A connected viewer keeps capture on however long; it stops after the viewer leaves and idles.
  t = 0;
  const cap = new FakeCapture();
  const d = new CaptureDemand({ capture: cap, idleMs: 15000, checkMs: 0, now, log: quiet });
  d.viewerAttached();
  t = 600000; assert.equal(d.check(), false);
  d.viewerDetached();
  t = 610000; assert.equal(d.check(), false);
  t = 616000; assert.equal(d.check(), true);
}

{ // frame() starts capture and resolves with the first frame, or null on timeout.
  t = 0;
  const cap = new FakeCapture();
  const d = new CaptureDemand({ capture: cap, idleMs: 15000, checkMs: 0, now, log: quiet });
  const p = d.frame(1000);
  assert.equal(cap.running, true);
  cap.emit('frame', Buffer.from('first'));
  assert.equal((await p).toString(), 'first');
  const cap2 = new FakeCapture();
  const d2 = new CaptureDemand({ capture: cap2, idleMs: 15000, checkMs: 0, now, log: quiet });
  assert.equal(await d2.frame(20), null);
}

console.log('demand: all passed');
