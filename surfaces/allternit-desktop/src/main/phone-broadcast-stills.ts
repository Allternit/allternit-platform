/**
 * The iPhone's Allternit screen broadcast as the keyboard/mouse core
 * (phone-hid-core.ts `PointerScreen`) needs it: stills pulled on demand from
 * the app-phone backend (`phone.screenshot` on an `app:` iPhone) and kept as
 * grayscale. Shared by the Bluetooth lanes that see through the broadcast:
 * `bt-mac` (phone-bt-mac.ts), `bt-apple` (phone-bt-apple.ts) and `bt-android`
 * (phone-bt-android.ts).
 */

import type { PointerScreen } from './phone-hid-core.js';
import { changed, type Gray } from './phone-usb-agent-vision.js';

export class BroadcastStills implements PointerScreen {
  private gray: Gray | null = null;
  private at: number | null = null;
  private readonly now: () => number;
  private readonly sleep: (ms: number) => Promise<void>;

  constructor(
    private readonly grab: () => Promise<{ jpeg: Buffer; ageMs: number } | null>,
    private readonly isLive: () => boolean,
    private readonly decode: (jpeg: Buffer) => Gray | null,
    opts: { now?: () => number; sleep?: (ms: number) => Promise<void> } = {},
  ) {
    this.now = opts.now ?? Date.now;
    this.sleep = opts.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  }

  live(): boolean {
    return this.isLive();
  }

  latestAt(): number | null {
    return this.at;
  }

  latestGray(): Gray | null {
    return this.gray;
  }

  /** Pulls a still; true when it is newer than the one kept. */
  async pull(): Promise<boolean> {
    const shot = await this.grab();
    if (!shot) return false;
    const at = this.now() - Math.max(0, shot.ageMs);
    if (this.at !== null && at <= this.at) return false;
    this.gray = this.decode(shot.jpeg);
    this.at = at;
    return true;
  }

  async nextGray(since: number, timeoutMs: number): Promise<Gray | null> {
    const deadline = this.now() + timeoutMs;
    for (;;) {
      await this.pull();
      if (this.at !== null && this.at > since) return this.gray;
      if (this.now() >= deadline) return null;
      await this.sleep(100);
    }
  }

  /** Waits at least `leastMs`, then until two stills `quietMs` apart match, or `mostMs`. */
  async settled(quietMs: number, leastMs: number, mostMs: number): Promise<void> {
    const deadline = this.now() + mostMs;
    await this.sleep(leastMs);
    await this.pull();
    while (this.now() < deadline) {
      const before = this.gray;
      await this.sleep(quietMs);
      await this.pull();
      if (before && this.gray && !changed(before, this.gray)) return;
    }
  }
}
