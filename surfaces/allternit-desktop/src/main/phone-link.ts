/**
 * Phone Link: setup state machine for "use the AI apps on your phone".
 * Contract and states: phone-link-types.ts. UI: allternit-ai PhoneLinkSetup.
 *
 * One setup runs at a time. Each platform is a driver that walks the states
 * and reports through `set()`; the manager owns the current status and
 * broadcasts every change. Electron-free so it runs under plain Node tests.
 */

import { EventEmitter } from 'node:events';
import type { AdbCall } from './phone-tools.js';
import type { PhoneLinkApp, PhoneLinkPlatform, PhoneLinkStatus } from './phone-link-types.js';

export interface PhoneLinkDriver {
  run(ctx: DriverContext): Promise<void>;
  continue?(): void;
  submitAppleId?(creds: { appleId: string; password: string }): void;
  submitTwoFactor?(code: string): void;
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
  /** Lazily resolved so adb is only downloaded when Android setup starts. */
  adb?: () => Promise<AdbCall>;
  sleep?: (ms: number) => Promise<void>;
  logger?: (message: string) => void;
}

const IDLE: PhoneLinkStatus = { platform: null, state: 'idle', instruction: 'Choose your phone to start.' };

export class PhoneLinkManager extends EventEmitter {
  private current: PhoneLinkStatus = IDLE;
  private driver: PhoneLinkDriver | null = null;
  private lastPlatform: PhoneLinkPlatform | null = null;
  private cancelRun: (() => void) | null = null;
  private runId = 0;

  constructor(private readonly deps: PhoneLinkDeps = {}) {
    super();
  }

  status(): PhoneLinkStatus {
    return this.current;
  }

  start(platform: PhoneLinkPlatform): PhoneLinkStatus {
    this.stop();
    this.lastPlatform = platform;
    const runId = ++this.runId;
    const host = this.deps.platform ?? process.platform;
    if (platform === 'ios' && host !== 'darwin') {
      return this.update(platform, runId, {
        state: 'not_supported',
        instruction: 'iPhone setup needs Allternit Desktop on a Mac.',
      });
    }
    const driver = platform === 'android' ? new AndroidDriver(this.deps) : new IosDriver();
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
    return this.lastPlatform ? this.start(this.lastPlatform) : this.current;
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

/** Package ids of the vendor apps Phone Link looks for. */
export const ANDROID_VENDOR_PACKAGES: Record<string, string> = {
  chatgpt: 'com.openai.chatgpt',
  claude: 'com.anthropic.claude',
  gemini: 'com.google.android.apps.bard',
  kimi: 'com.moonshot.kimichat',
  copilot: 'com.microsoft.copilot',
  grok: 'ai.x.grok',
  perplexity: 'ai.perplexity.app.android',
};

const POLL_MS = 1500;
const WIFI_PORT = 5555;

export interface AdbDeviceLine {
  serial: string;
  state: string;
  model?: string;
}

export function parseAdbDevices(stdout: string): AdbDeviceLine[] {
  return stdout
    .split('\n')
    .slice(1)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [serial, state, ...rest] = line.split(/\s+/);
      const model = rest.find((part) => part.startsWith('model:'))?.slice('model:'.length).replace(/_/g, ' ');
      return { serial, state, model };
    });
}

export function installedVendorApps(packagesStdout: string): PhoneLinkApp[] {
  const installed = new Set(
    packagesStdout.split('\n').map((line) => line.trim().replace(/^package:/, '')).filter(Boolean),
  );
  return Object.entries(ANDROID_VENDOR_PACKAGES)
    .filter(([, pkg]) => installed.has(pkg))
    .map(([vendor]) => ({ vendor, signedIn: null }));
}

/** First IPv4 on wlan0 from `ip -f inet addr show wlan0`. */
export function parseWlanIp(stdout: string): string | null {
  return /inet (\d+\.\d+\.\d+\.\d+)\//.exec(stdout)?.[1] ?? null;
}

class AndroidDriver implements PhoneLinkDriver {
  constructor(private readonly deps: PhoneLinkDeps) {}

  async run(ctx: DriverContext): Promise<void> {
    if (!this.deps.adb) throw new Error('adb is not available in this build');
    ctx.set({ state: 'usb_debugging', instruction: 'Getting Android tools ready…' });
    const adb = await this.deps.adb();

    // 1. A USB-connected phone that allowed this computer.
    let usb: AdbDeviceLine | undefined;
    for (;;) {
      if (ctx.isCancelled()) return;
      const devices = parseAdbDevices((await adb(['devices', '-l'])).stdout).filter((d) => !d.serial.includes(':'));
      usb = devices.find((d) => d.state === 'device');
      if (usb) break;
      const unauthorized = devices.some((d) => d.state === 'unauthorized');
      ctx.set({
        state: 'usb_debugging',
        instruction: unauthorized
          ? 'On your phone, tap Allow on "Allow USB debugging?".'
          : 'Plug your phone in with a cable and turn on USB debugging in Developer options.',
        detail: unauthorized
          ? undefined
          : 'Settings › About phone › tap Build number 7 times, then Settings › Developer options › USB debugging.',
      });
      await ctx.sleep(POLL_MS);
    }

    const serial = usb.serial;
    const prop = async (name: string) => (await adb(['shell', 'getprop', name], { serial })).stdout.trim();
    const deviceName = usb.model ?? (await prop('ro.product.model'));
    const osVersion = `Android ${await prop('ro.build.version.release')}`;
    const apps = installedVendorApps((await adb(['shell', 'pm', 'list', 'packages'], { serial })).stdout);

    // 2. Move to Wi-Fi so the cable can come out.
    ctx.set({ state: 'pairing', instruction: 'Connecting over Wi-Fi. Keep the phone on the same Wi-Fi as this computer.', deviceName, osVersion, apps });
    const ip = parseWlanIp((await adb(['shell', 'ip', '-f', 'inet', 'addr', 'show', 'wlan0'], { serial })).stdout);
    if (!ip) {
      ctx.set({
        state: 'error',
        instruction: 'Turn on Wi-Fi on your phone, then try again.',
        deviceName,
        osVersion,
        apps,
        error: { code: 'no_wifi', message: 'The phone has no Wi-Fi address.', retryable: true },
      });
      return;
    }
    await adb(['tcpip', String(WIFI_PORT)], { serial });
    await ctx.sleep(1500);
    const target = `${ip}:${WIFI_PORT}`;
    const connected = await adb(['connect', target]);
    if (ctx.isCancelled()) return;
    if (!/connected to/.test(connected.stdout)) {
      ctx.set({
        state: 'error',
        instruction: 'Couldn\'t reach the phone over Wi-Fi. Check both are on the same network, then try again.',
        deviceName,
        osVersion,
        apps,
        error: { code: 'wifi_connect_failed', message: connected.stdout.trim() || connected.stderr.trim(), retryable: true },
      });
      return;
    }
    ctx.set({
      state: 'ready_wifi',
      instruction: 'Connected. You can unplug the cable.',
      detail: target,
      deviceName,
      osVersion,
      apps,
    });
  }
}

// ─── iPhone ───────────────────────────────────────────────────────────────────

/** The cable setup (signing + runner install over idevice) is being built; see SPEC §6. */
class IosDriver implements PhoneLinkDriver {
  async run(ctx: DriverContext): Promise<void> {
    ctx.set({
      state: 'error',
      instruction: 'iPhone setup isn\'t ready in this version yet.',
      error: { code: 'ios_not_ready', message: 'iPhone Phone Link is still being built.', retryable: false },
    });
  }
}
