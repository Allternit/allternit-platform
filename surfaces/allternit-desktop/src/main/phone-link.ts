/**
 * Phone Link: setup state machine for "use the AI apps on your phone".
 * Contract and states: phone-link-types.ts. UI: allternit-ai PhoneLinkSetup.
 *
 * One setup runs at a time. Each platform is a driver that walks the states
 * and reports through `set()`; the manager owns the current status and
 * broadcasts every change. Electron-free so it runs under plain Node tests.
 */

import { EventEmitter } from 'node:events';
import type { PhoneLinkPlatform, PhoneLinkStatus } from './phone-link-types.js';
import { IosDriver, type IosDriverDeps } from './phone-link-ios.js';
import { AndroidDriver, type AndroidDriverDeps, type AndroidSetupTarget } from './phone-link-android.js';

// Android pieces live in phone-link-android.ts (re-exported for importers).
export { ANDROID_VENDOR_PACKAGES, installedVendorApps, parseAdbDevices, parseWlanIp } from './phone-link-android.js';
export type { AdbDeviceLine, AndroidDriverDeps, AndroidSetupTarget } from './phone-link-android.js';

export interface PhoneLinkDriver {
  run(ctx: DriverContext): Promise<void>;
  continue?(): void;
  submitAppleId?(creds: { appleId: string; password: string }): void;
  submitTwoFactor?(code: string): void;
  /** Stop child processes / forwards the run started (cancel, retry, quit). */
  dispose?(): void;
}

export interface DriverContext {
  set(status: Omit<PhoneLinkStatus, 'platform'>): void;
  /** Resolves on cancel(); drivers race their waits against it. */
  cancelled: Promise<void>;
  isCancelled(): boolean;
  sleep(ms: number): Promise<void>;
}

export interface PhoneLinkDeps {
  platform?: NodeJS.Platform;
  /** Lazily resolved so adb and the helper APK are only fetched when Android setup starts. */
  android?: () => Promise<AndroidDriverDeps>;
  /** Lazily resolved so go-ios and the runner are only fetched when iPhone setup starts. */
  ios?: () => Promise<IosDriverDeps>;
  sleep?: (ms: number) => Promise<void>;
  logger?: (message: string) => void;
}

const IDLE: PhoneLinkStatus = { platform: null, state: 'idle', instruction: 'Choose your phone to start.' };

export class PhoneLinkManager extends EventEmitter {
  private current: PhoneLinkStatus = IDLE;
  private driver: PhoneLinkDriver | null = null;
  private lastPlatform: PhoneLinkPlatform | null = null;
  private lastTarget: AndroidSetupTarget | undefined;
  private cancelRun: (() => void) | null = null;
  private runId = 0;

  constructor(private readonly deps: PhoneLinkDeps = {}) {
    super();
  }

  status(): PhoneLinkStatus {
    return this.current;
  }

  /**
   * `target` (Android only) runs setup on a phone adb already reaches by
   * address, such as a cloud phone, instead of waiting for a USB cable.
   */
  start(platform: PhoneLinkPlatform, target?: AndroidSetupTarget): PhoneLinkStatus {
    this.stop();
    this.lastPlatform = platform;
    this.lastTarget = platform === 'android' ? target : undefined;
    const runId = ++this.runId;
    const host = this.deps.platform ?? process.platform;
    if (platform === 'ios' && host !== 'darwin') {
      return this.update(platform, runId, {
        state: 'not_supported',
        instruction: 'iPhone setup needs Allternit Desktop on a Mac.',
      });
    }
    const driver = platform === 'android' ? new LazyAndroidDriver(this.deps.android, this.lastTarget) : new LazyIosDriver(this.deps.ios);
    this.driver = driver;
    let cancel!: () => void;
    const cancelled = new Promise<void>((resolve) => (cancel = resolve));
    let isCancelled = false;
    this.cancelRun = () => {
      isCancelled = true;
      cancel();
    };
    const sleep = this.deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
    const ctx: DriverContext = {
      set: (status) => this.update(platform, runId, status),
      cancelled,
      isCancelled: () => isCancelled,
      sleep: (ms) => Promise.race([sleep(ms), cancelled]),
    };
    driver.run(ctx).catch((error: unknown) => {
      if (isCancelled) return;
      this.deps.logger?.(`[phone-link] ${platform} setup failed: ${String(error)}`);
      this.update(platform, runId, {
        state: 'error',
        instruction: 'Setup stopped. Try again.',
        error: { code: 'unexpected', message: error instanceof Error ? error.message : String(error), retryable: true },
      });
    });
    return this.current;
  }

  cancel(): PhoneLinkStatus {
    this.stop();
    this.runId++;
    this.current = IDLE;
    this.emit('status', this.current);
    return this.current;
  }

  retry(): PhoneLinkStatus {
    return this.lastPlatform ? this.start(this.lastPlatform, this.lastTarget) : this.current;
  }

  continue(): PhoneLinkStatus {
    this.driver?.continue?.();
    return this.current;
  }

  submitAppleId(creds: { appleId: string; password: string }): PhoneLinkStatus {
    this.driver?.submitAppleId?.(creds);
    return this.current;
  }

  submitTwoFactor(code: string): PhoneLinkStatus {
    this.driver?.submitTwoFactor?.(code);
    return this.current;
  }

  private stop(): void {
    this.driver?.dispose?.();
    this.cancelRun?.();
    this.cancelRun = null;
    this.driver = null;
  }

  /** Ignores updates from a run that was cancelled or replaced. */
  private update(platform: PhoneLinkPlatform, runId: number, status: Omit<PhoneLinkStatus, 'platform'>): PhoneLinkStatus {
    if (runId !== this.runId) return this.current;
    this.current = { platform, ...status };
    this.emit('status', this.current);
    return this.current;
  }
}

// ─── Android ──────────────────────────────────────────────────────────────────

/** Resolves adb + the helper APK on start, then hands over to AndroidDriver (phone-link-android.ts). */
class LazyAndroidDriver implements PhoneLinkDriver {
  private inner: AndroidDriver | null = null;
  private disposed = false;

  constructor(
    private readonly load?: () => Promise<AndroidDriverDeps>,
    private readonly target?: AndroidSetupTarget,
  ) {}

  async run(ctx: DriverContext): Promise<void> {
    if (!this.load) {
      ctx.set({
        state: 'error',
        instruction: 'Android setup isn\'t ready in this version yet.',
        error: { code: 'android_not_ready', message: 'Android Phone Link is not wired in this build.', retryable: false },
      });
      return;
    }
    ctx.set({ state: 'usb_debugging', instruction: 'Getting Android tools ready…' });
    let deps: AndroidDriverDeps;
    try {
      deps = await this.load();
    } catch (error) {
      if (ctx.isCancelled()) return;
      ctx.set({
        state: 'error',
        instruction: 'Couldn\'t get the Android tools. Check your connection and try again.',
        error: { code: 'tools_unavailable', message: error instanceof Error ? error.message : String(error), retryable: true },
      });
      return;
    }
    if (ctx.isCancelled() || this.disposed) return;
    this.inner = new AndroidDriver(deps, this.target);
    await this.inner.run(ctx);
  }

  dispose(): void {
    this.disposed = true;
    this.inner?.dispose();
  }
}

// ─── iPhone ───────────────────────────────────────────────────────────────────

/** Resolves go-ios, the runner and the signer on start, then hands over to IosDriver (phone-link-ios.ts). */
class LazyIosDriver implements PhoneLinkDriver {
  private inner: IosDriver | null = null;
  private disposed = false;

  constructor(private readonly load?: () => Promise<IosDriverDeps>) {}

  async run(ctx: DriverContext): Promise<void> {
    if (!this.load) {
      ctx.set({
        state: 'error',
        instruction: 'iPhone setup isn\'t ready in this version yet.',
        error: { code: 'ios_not_ready', message: 'iPhone Phone Link is not wired in this build.', retryable: false },
      });
      return;
    }
    ctx.set({ state: 'needs_cable', instruction: 'Getting iPhone tools ready…' });
    let deps: IosDriverDeps;
    try {
      deps = await this.load();
    } catch (error) {
      if (ctx.isCancelled()) return;
      ctx.set({
        state: 'error',
        instruction: 'Couldn\'t download the iPhone tools. Check your connection and try again.',
        error: { code: 'tools_unavailable', message: error instanceof Error ? error.message : String(error), retryable: true },
      });
      return;
    }
    if (ctx.isCancelled() || this.disposed) return;
    this.inner = new IosDriver(deps);
    await this.inner.run(ctx);
  }

  continue(): void {
    this.inner?.continue();
  }

  submitAppleId(creds: { appleId: string; password: string }): void {
    this.inner?.submitAppleId(creds);
  }

  submitTwoFactor(code: string): void {
    this.inner?.submitTwoFactor(code);
  }

  dispose(): void {
    this.disposed = true;
    this.inner?.dispose();
  }
}
