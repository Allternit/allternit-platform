/**
 * Phone Link, iPhone: the cable setup that installs our iCTRL runner and
 * leaves it reachable over Wi-Fi (SPEC-signed-in-apps-routing §6).
 *
 *   needs_cable → trust_prompt → apple_id (→ two_factor) → developer_mode
 *   → waiting_for_reboot → installing → trust_certificate → ready_wifi
 *
 * Everything the phone reports is polled; only trust_certificate needs the
 * user's "I've done it" (continue()), because the only probe is launching the
 * app, and each untrusted launch pops an alert on the phone.
 *
 * Wi-Fi. A runner started by `ios runtest` lives inside an XCTest session
 * carried by the USB tunnel; iOS tears it down when the cable comes out
 * (seen live 2026-10-16), and go-ios can't carry that tunnel over Wi-Fi
 * (network-only devices are skipped by its tunnel manager, go-ios #596/#532).
 * So the cable session is setup only: runtest (with the one-shot
 * auto-approval) → pair → turn on Remote Access (persisted by the runner) →
 * stop the session and relaunch the runner as a plain app (`ios launch`, the
 * same as tapping its icon, documented by iCTRL as the no-Mac way to start
 * it). Then Desktop drops the tunnel and forward itself and checks the runner
 * still answers on the LAN before saying the cable can come out.
 *
 * After a phone restart the Developer Disk Image is gone and the runner can't
 * start until it's remounted: Desktop does that when the phone is plugged in
 * (or visible to usbmuxd over Wi-Fi sync), and the iCTRL host app can do it
 * on the phone (Settings › Mount without a Mac), for which setup leaves the
 * Mac's pairing record in that app's container.
 */

import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import type { DriverContext, PhoneLinkDriver } from './phone-link.js';
import type { PhoneLinkApp, PhoneLinkStatus } from './phone-link-types.js';
import { ICTRL_PORT, IctrlError, type IctrlWhoami } from './ictrl-client.js';
import type { GoIos, LongRunning } from './phone-goios.js';
import {
  RunnerUnavailableError,
  SigningError,
  type AppleIdCredentials,
  type RunnerArtifacts,
  type RunnerSigner,
  type SignedRunner,
} from './phone-ios-runner.js';

/**
 * Bundle ids of the vendor apps Phone Link looks for. Verified 2026-10-16
 * against the App Store lookup API (itunes.apple.com/search → bundleId)
 * unless marked otherwise.
 */
export const IOS_VENDOR_BUNDLES: Record<string, string[]> = {
  chatgpt: ['com.openai.chat'], // ChatGPT, OpenAI OpCo (id 6448311069)
  claude: ['com.anthropic.claude'], // Claude by Anthropic (id 6473753684)
  gemini: ['com.google.gemini'], // Google Gemini (id 6477489729)
  grok: ['ai.x.GrokApp'], // Grok AI, X Corp. (id 6670324846)
  kimi: ['com.moonshot.kimichat'], // Kimi, Beijing Moonshot (id 6474233312)
  // "Microsoft Copilot" in the US store is com.microsoft.officemobile (id 541164041, the M365 app).
  // com.microsoft.copilot (the consumer Copilot app) is UNVERIFIED: it no longer appears in US search.
  copilot: ['com.microsoft.copilot', 'com.microsoft.officemobile'],
  perplexity: ['ai.perplexity.app'], // Perplexity (id 1668000334)
};

export function iosVendorApps(installed: Iterable<string>): PhoneLinkApp[] {
  const set = new Set(installed);
  return Object.entries(IOS_VENDOR_BUNDLES)
    .filter(([, ids]) => ids.some((id) => set.has(id)))
    .map(([vendor]) => ({ vendor, signedIn: null }));
}

/** Non-secret per-phone state, kept so a later start can reconnect instead of re-installing. */
export interface IosLinkRecord {
  udid: string;
  deviceName?: string;
  osVersion?: string;
  deviceId?: string;
  runnerBundleId: string;
  hostBundleId: string;
  xctestConfig: string;
  testToRun: string;
  expiresAt?: string;
  lanHost?: string;
  /** Vendor apps found last time, so a Wi-Fi-only reconnect doesn't open every app to check. */
  apps?: PhoneLinkApp[];
  linkedAt: string;
}

export interface IosLinkStore {
  load(udid: string): IosLinkRecord | null;
  all(): IosLinkRecord[];
  save(record: IosLinkRecord): void;
}

export function fileLinkStore(file: string): IosLinkStore {
  const read = (): Record<string, IosLinkRecord> => {
    try {
      return JSON.parse(fs.readFileSync(file, 'utf8'));
    } catch {
      return {};
    }
  };
  return {
    load: (udid) => read()[udid] ?? null,
    all: () => Object.values(read()),
    save: (record) => {
      const all = read();
      all[record.udid] = record;
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, JSON.stringify(all, null, 2), { mode: 0o600 });
    },
  };
}

/** The parts of IctrlClient the driver uses (faked in tests). */
export interface IctrlLike {
  readonly host: string;
  hasKey(): boolean;
  forget(): void;
  whoami(): Promise<IctrlWhoami | null>;
  pair(clientName: string, meta?: { deviceId?: string; deviceName?: string }): Promise<unknown>;
  call<T = Record<string, unknown>>(method: string, params?: Record<string, unknown>, timeoutMs?: number): Promise<T>;
  withHost(host: string): IctrlLike;
}

export type GoIosLike = Pick<
  GoIos,
  'list' | 'info' | 'pair' | 'devmodeEnabled' | 'devmodeReveal' | 'tunnelStart' | 'tunnels' | 'imageAuto' | 'install' | 'apps' | 'launch' | 'kill' | 'pushToApp' | 'runtest'
>;

export interface IosDriverDeps {
  goios: GoIosLike;
  artifacts: () => Promise<RunnerArtifacts>;
  signer: RunnerSigner;
  /** Client for this phone's key (secret store entry per UDID) at `host`. */
  ictrl: (udid: string, host: string) => IctrlLike;
  /** 127.0.0.1-only forward to the phone's port (phone-usbmux.ts). */
  forward: (udid: string, devicePort: number) => Promise<{ port: number; close(): Promise<void> }>;
  /** Directory holding the pinned DDI (ensureDdi), resolved lazily. */
  ddiDir: () => Promise<string>;
  readPairRecord?: (udid: string) => Promise<Buffer | null>;
  store: IosLinkStore;
  workDir: string;
  clientName?: string;
  now?: () => number;
  logger?: (message: string) => void;
}

const POLL_MS = 1500;
const RESIGN_MARGIN_MS = 24 * 3600_000;
const PAIRING_FILE_REMOTE = 'Library/Application Support/Pairing/pairingFile.plist';

type Base = Omit<PhoneLinkStatus, 'platform' | 'state' | 'instruction'>;

class SetupStop extends Error {}

export class IosDriver implements PhoneLinkDriver {
  private continueWaiter: (() => void) | null = null;
  private appleIdWaiter: ((creds: AppleIdCredentials) => void) | null = null;
  private codeWaiter: ((code: string) => void) | null = null;
  private tunnel: LongRunning | null = null;
  private session: LongRunning | null = null;
  private fwd: { port: number; close(): Promise<void> } | null = null;
  private base: Base = {};

  constructor(private readonly deps: IosDriverDeps) {}

  continue(): void {
    this.continueWaiter?.();
  }

  submitAppleId(creds: AppleIdCredentials): void {
    this.appleIdWaiter?.(creds);
  }

  submitTwoFactor(code: string): void {
    this.codeWaiter?.(code);
  }

  dispose(): void {
    this.session?.stop();
    this.tunnel?.stop();
    void this.fwd?.close();
    this.session = this.tunnel = this.fwd = null;
  }

  private log(message: string): void {
    this.deps.logger?.(`[phone-link ios] ${message}`);
  }

  private set(ctx: DriverContext, state: PhoneLinkStatus['state'], instruction: string, extra: Partial<Base> = {}): void {
    ctx.set({ ...this.base, detail: undefined, progress: undefined, ...extra, state, instruction });
  }

  private fail(ctx: DriverContext, code: string, instruction: string, message: string, retryable: boolean): never {
    this.dispose();
    ctx.set({ ...this.base, state: 'error', instruction, error: { code, message, retryable } });
    throw new SetupStop();
  }

  /** Wait for a UI answer (or cancel). */
  private ask<T>(ctx: DriverContext, register: (resolve: (value: T) => void) => void): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      register(resolve);
      void ctx.cancelled.then(() => reject(new SetupStop()));
    });
  }

  async run(ctx: DriverContext): Promise<void> {
    try {
      await this.walk(ctx);
    } catch (error) {
      if (error instanceof SetupStop || ctx.isCancelled()) return;
      this.dispose();
      throw error;
    }
  }

  private check(ctx: DriverContext): void {
    if (ctx.isCancelled()) throw new SetupStop();
  }

  private async walk(ctx: DriverContext): Promise<void> {
    const { goios } = this.deps;

    // 0. A phone linked before and still answering on Wi-Fi needs no cable at all.
    for (const known of this.deps.store.all()) {
      this.check(ctx);
      if (!known.lanHost || !this.deps.ictrl(known.udid, known.lanHost).hasKey()) continue;
      const lan = await this.probeLan(known.udid, known.lanHost);
      if (lan) {
        this.base = { deviceName: known.deviceName, osVersion: known.osVersion };
        return this.finish(ctx, known.udid, known, lan);
      }
    }

    // 1. A phone on the cable.
    let udid: string | undefined;
    for (;;) {
      this.check(ctx);
      udid = (await goios.list())[0];
      if (udid) break;
      this.set(ctx, 'needs_cable', 'Plug your iPhone into this Mac with a cable.', {
        detail: 'Only needed once. After setup it works over Wi-Fi.',
      });
      await ctx.sleep(POLL_MS);
    }

    // 2. The phone trusts this Mac.
    let info = await goios.info(udid);
    let asked = false;
    while (!info.ok) {
      this.check(ctx);
      this.set(ctx, 'trust_prompt', 'Unlock your iPhone and tap Trust, then enter its passcode.');
      if (!asked) {
        asked = true;
        await goios.pair(udid);
      }
      await ctx.sleep(POLL_MS);
      if (!(await goios.list()).includes(udid)) return this.walk(ctx);
      info = await goios.info(udid);
      // A refused or timed-out prompt never re-appears on its own; ask again.
      if (!info.ok && /denied|reject/i.test(info.error)) asked = false;
    }
    if (!info.ok) throw new SetupStop();
    this.base = { deviceName: info.name, osVersion: info.version ? `iOS ${info.version}` : undefined };
    const major = Number((info.version ?? '0').split('.')[0]);
    if (major && major < 17) {
      this.fail(ctx, 'ios_too_old', 'Phone Link needs iOS 17 or later. Update your iPhone, then try again.', `iOS ${info.version}`, false);
    }

    // Already linked: reconnect instead of reinstalling, unless the signature is about to lapse.
    const known = this.deps.store.load(udid);
    const fresh = known?.expiresAt ? Date.parse(known.expiresAt) - this.now() > RESIGN_MARGIN_MS : !!known;
    if (known && fresh && this.deps.ictrl(udid, '127.0.0.1:0').hasKey()) {
      this.set(ctx, 'installing', 'Reconnecting to your iPhone…', { progress: 0.5 });
      const lan = await reconnectIos(this.deps, udid, ctx.sleep);
      this.check(ctx);
      if (lan) return this.finish(ctx, udid, known, lan);
    }

    // 3. Unsigned runner + signing (Apple ID for the product signer).
    let artifacts: RunnerArtifacts;
    try {
      artifacts = await this.deps.artifacts();
    } catch (error) {
      const unavailable = error instanceof RunnerUnavailableError;
      this.fail(
        ctx,
        unavailable ? 'runner_unavailable' : 'runner_download_failed',
        unavailable ? 'iPhone setup isn\'t available in this version yet.' : 'Couldn\'t download the iPhone helper. Check your connection and try again.',
        error instanceof Error ? error.message : String(error),
        !unavailable,
      );
    }
    let signed = known && fresh && known.runnerBundleId ? null : await this.signRunner(ctx, udid, artifacts!);
    this.check(ctx);

    // 4. Developer Mode (Settings switch, restart, then "Turn On" after the restart).
    await this.developerMode(ctx, udid);

    // 5. Tunnel, Developer Disk Image, install.
    const record: IosLinkRecord = {
      udid,
      deviceName: this.base.deviceName,
      osVersion: this.base.osVersion,
      runnerBundleId: signed?.runnerBundleId ?? known!.runnerBundleId,
      hostBundleId: signed?.hostBundleId ?? known!.hostBundleId,
      xctestConfig: artifacts!.manifest.runner.xctestConfig,
      testToRun: artifacts!.manifest.runner.testToRun,
      expiresAt: signed?.expiresAt ?? known?.expiresAt,
      linkedAt: new Date(this.now()).toISOString(),
    };
    await this.prepareDevice(ctx, udid, 0.05);
    const install = async (pkg: SignedRunner) => {
      for (const [file, progress] of [[pkg.hostIpa, 0.35], [pkg.runnerIpa, 0.55]] as const) {
        this.set(ctx, 'installing', 'Installing the Allternit helper on your iPhone…', { progress });
        await this.attempt(ctx, 'install_failed', 'Couldn\'t install the helper. Keep the cable in and try again.', () => goios.install(udid!, file));
      }
    };
    if (signed) await install(signed);

    // 6. Trust the developer certificate (only visible now that the app is on the phone).
    for (;;) {
      this.check(ctx);
      const launched = await goios.launch(udid, record.hostBundleId, { killExisting: true });
      if (launched.ok) break;
      if (launched.notInstalled) {
        // Linked before, but the helper was deleted: sign and install again.
        if (signed) this.fail(ctx, 'install_failed', 'The helper isn\'t on your iPhone. Try again.', launched.error, true);
        signed = await this.signRunner(ctx, udid, artifacts!);
        Object.assign(record, { runnerBundleId: signed.runnerBundleId, hostBundleId: signed.hostBundleId, expiresAt: signed.expiresAt });
        await install(signed);
        continue;
      }
      if (!launched.untrusted) this.fail(ctx, 'launch_failed', 'The helper wouldn\'t start. Try again.', launched.error, true);
      this.set(ctx, 'trust_certificate', 'On your iPhone, trust the developer certificate, then tap Continue.', {
        detail: `Settings › General › VPN & Device Management › your Apple ID › Trust.`,
      });
      await this.ask<void>(ctx, (resolve) => (this.continueWaiter = () => resolve()));
      this.continueWaiter = null;
    }

    // 7. Start the runner over the cable once, pair with the one-shot auto-approval.
    this.set(ctx, 'installing', 'Starting the helper. If your iPhone asks for its passcode, enter it.', { progress: 0.75 });
    const usb = await this.startCableSession(ctx, udid, record, true);
    const whoami = await usb.whoami();
    record.deviceId = whoami?.device_id;
    if (usb.hasKey()) {
      try {
        await usb.call('device_info');
      } catch (error) {
        if (!(error instanceof IctrlError && error.needsRepair)) throw error;
        usb.forget();
      }
    }
    if (!usb.hasKey()) {
      try {
        await usb.pair(this.deps.clientName ?? `Allternit Desktop (${os.hostname()})`, { deviceId: whoami?.device_id, deviceName: this.base.deviceName });
      } catch (error) {
        this.fail(ctx, 'pair_failed', 'Couldn\'t connect to the helper. Try again.', error instanceof Error ? error.message : String(error), true);
      }
    }

    // 8. Leave the no-Mac remount option on the phone, then move to Wi-Fi.
    await this.leavePairingFile(udid, record.hostBundleId);
    this.set(ctx, 'installing', 'Moving the connection to Wi-Fi…', { progress: 0.9 });
    const lanHost = await this.moveToWifi(ctx, udid, usb, record);
    record.lanHost = lanHost;
    await this.finish(ctx, udid, record, lanHost);
  }

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  private async attempt(ctx: DriverContext, code: string, instruction: string, fn: () => Promise<void>): Promise<void> {
    try {
      await fn();
    } catch (error) {
      this.check(ctx);
      this.fail(ctx, code, instruction, error instanceof Error ? error.message : String(error), true);
    }
  }

  private async signRunner(ctx: DriverContext, udid: string, artifacts: RunnerArtifacts): Promise<SignedRunner> {
    const { signer } = this.deps;
    let problem: string | undefined;
    for (;;) {
      this.check(ctx);
      let credentials: AppleIdCredentials | undefined;
      if (signer.needsAppleId) {
        this.set(ctx, 'apple_id', 'Sign in with your Apple ID to install the helper.', {
          detail: problem ?? 'A free Apple ID works. Your password goes to Apple only and isn\'t saved.',
        });
        credentials = await this.ask<AppleIdCredentials>(ctx, (resolve) => (this.appleIdWaiter = resolve));
        this.appleIdWaiter = null;
        this.set(ctx, 'apple_id', 'Signing in to Apple…');
      } else {
        this.set(ctx, 'installing', 'Preparing the helper…', { progress: 0.02 });
      }
      try {
        return await signer.sign({
          udid,
          deviceName: this.base.deviceName ?? 'iPhone',
          artifacts,
          workDir: path.join(this.deps.workDir, 'signed', udid),
          credentials,
          twoFactor: (hint) => {
            this.set(ctx, 'two_factor', hint.sms ? `Enter the code Apple texted to ${hint.phone ?? 'your phone'}.` : 'Enter the 6-digit code shown on your Apple devices.');
            return this.ask<string>(ctx, (resolve) => (this.codeWaiter = resolve)).finally(() => {
              this.codeWaiter = null;
              this.set(ctx, 'apple_id', 'Setting up your developer certificate…');
            });
          },
        });
      } catch (error) {
        this.check(ctx);
        if (error instanceof SetupStop) throw error;
        if (error instanceof SigningError && (error.code === 'bad_credentials' || error.code === 'two_factor_failed') && signer.needsAppleId) {
          problem = error.code === 'bad_credentials' ? 'That Apple ID or password didn\'t work. Try again.' : 'That code didn\'t work. Sign in again to get a new one.';
          continue;
        }
        const code = error instanceof SigningError ? error.code : 'signing_failed';
        const limits: Record<string, string> = {
          app_id_limit: 'Your Apple ID has hit Apple\'s weekly limit for new apps. Try again in a few days.',
          certificate_limit: 'Your Apple ID has too many developer certificates. Remove one at developer.apple.com, then try again.',
          device_limit: 'Your Apple ID has too many registered devices. Remove one at developer.apple.com, then try again.',
          no_identity: 'No developer certificate on this Mac covers this iPhone.',
          signer_unavailable: 'iPhone setup isn\'t available in this version yet.',
        };
        this.fail(ctx, code, limits[code] ?? 'Couldn\'t sign the helper. Try again.', error instanceof Error ? error.message : String(error), code !== 'signer_unavailable');
      }
    }
  }

  private async developerMode(ctx: DriverContext, udid: string): Promise<void> {
    const { goios } = this.deps;
    const enabled = async () => {
      try {
        return await goios.devmodeEnabled(udid);
      } catch {
        return false;
      }
    };
    if (await enabled()) return;
    await goios.devmodeReveal(udid).catch(() => {});
    let rebooted = false;
    for (;;) {
      this.check(ctx);
      const present = (await goios.list()).includes(udid);
      if (!present) rebooted = true;
      if (present && (await goios.info(udid)).ok && (await enabled())) return;
      if (rebooted) {
        this.set(ctx, 'waiting_for_reboot', present ? 'Unlock your iPhone and tap Turn On, then enter its passcode.' : 'Your iPhone is restarting. Keep it plugged in.');
      } else {
        this.set(ctx, 'developer_mode', 'Turn on Developer Mode, then tap Restart.', {
          detail: 'Settings › Privacy & Security › Developer Mode (at the bottom).',
        });
      }
      await ctx.sleep(POLL_MS);
    }
  }

  /** Tunnel (iOS 17+ services) and the Developer Disk Image. */
  private async prepareDevice(ctx: DriverContext, udid: string, progress: number): Promise<void> {
    const { goios } = this.deps;
    this.set(ctx, 'installing', 'Getting your iPhone ready…', { progress });
    if (!this.tunnel?.isRunning()) {
      this.tunnel = goios.tunnelStart(udid);
      let up = false;
      for (let i = 0; i < 40 && !up; i++) {
        this.check(ctx);
        up = (await goios.tunnels()).some((t) => t.udid === udid);
        if (!up) {
          if (!this.tunnel.isRunning()) break;
          await ctx.sleep(500);
        }
      }
      if (!up) this.fail(ctx, 'tunnel_failed', 'Couldn\'t open a connection to your iPhone. Unplug it, plug it back in and try again.', 'go-ios tunnel did not come up', true);
    }
    this.set(ctx, 'installing', 'Getting your iPhone ready…', { progress: progress + 0.15 });
    const dir = await this.deps.ddiDir().catch((error: unknown) =>
      this.fail(ctx, 'ddi_unavailable', 'Couldn\'t download the developer image. Check your connection and try again.', String(error), true),
    );
    await this.attempt(ctx, 'ddi_mount_failed', 'Couldn\'t prepare your iPhone for the helper. Try again.', () => goios.imageAuto(udid, dir));
  }

  /** runtest + 127.0.0.1 forward; resolves once the runner answers through the cable. */
  private async startCableSession(ctx: DriverContext, udid: string, record: IosLinkRecord, armAutoApprove: boolean): Promise<IctrlLike> {
    this.session?.stop();
    this.session = this.deps.goios.runtest({
      udid,
      runnerBundleId: record.runnerBundleId,
      xctestConfig: record.xctestConfig,
      testToRun: record.testToRun,
      // One auto-approved pairing, for this launch only. Never set anywhere else.
      env: armAutoApprove ? { ICTRL_ALLOW_AUTH_ONCE: '1' } : undefined,
    });
    if (!this.fwd) this.fwd = await this.deps.forward(udid, ICTRL_PORT);
    const usb = this.deps.ictrl(udid, `127.0.0.1:${this.fwd.port}`);
    for (let i = 0; i < 60; i++) {
      this.check(ctx);
      if (await usb.whoami().catch(() => null)) return usb;
      if (!this.session.isRunning() && i > 4) break;
      await ctx.sleep(1000);
    }
    this.fail(ctx, 'runner_start_failed', 'The helper didn\'t start. Unlock your iPhone and try again.', 'runner did not answer on :8200', true);
  }

  private async leavePairingFile(udid: string, hostBundleId: string): Promise<void> {
    if (!this.deps.readPairRecord) return;
    try {
      const record = await this.deps.readPairRecord(udid);
      if (!record) return;
      const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'allternit-pair-'));
      const file = path.join(tmp, 'pairingFile.plist');
      try {
        fs.writeFileSync(file, record, { mode: 0o600 });
        await this.deps.goios.pushToApp(udid, hostBundleId, file, PAIRING_FILE_REMOTE);
      } finally {
        fs.rmSync(tmp, { recursive: true, force: true });
      }
    } catch (error) {
      // Optional: only the no-Mac remount needs it.
      this.log(`pairing file not left on the phone: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /**
   * Remote Access on, relaunch the runner outside the cable's test session,
   * then drop our tunnel and forward and make sure it still answers.
   */
  private async moveToWifi(ctx: DriverContext, udid: string, usb: IctrlLike, record: IosLinkRecord): Promise<string> {
    const config = await usb.call<{ address?: string; ipv4_interfaces?: { name: string; address: string }[] }>('config', { allow_remote_access: true }).catch((error: unknown) =>
      this.fail(ctx, 'wifi_setup_failed', 'Couldn\'t turn on Wi-Fi access for the helper. Try again.', String(error), true),
    );
    const lanHost = pickLanHost(config);
    if (!lanHost) {
      this.fail(ctx, 'no_wifi', 'Turn on Wi-Fi on your iPhone and join the same network as this Mac, then try again.', 'the runner reported no LAN address', true);
    }
    this.session?.stop();
    this.session = null;
    await ctx.sleep(1500);
    const relaunched = await this.deps.goios.launch(udid, record.runnerBundleId, { killExisting: true });
    if (!relaunched.ok) this.fail(ctx, 'runner_start_failed', 'The helper didn\'t restart. Try again.', relaunched.error, true);

    let reachable = await this.waitForLan(ctx, udid, lanHost, 30);
    if (reachable) {
      // The real test: no tunnel, no forward. The runner must not depend on them.
      await this.fwd?.close();
      this.fwd = null;
      this.tunnel?.stop();
      this.tunnel = null;
      await ctx.sleep(3000);
      reachable = await this.waitForLan(ctx, udid, lanHost, 10);
    }
    if (!reachable) {
      this.fail(
        ctx,
        'wifi_unreachable',
        'Your iPhone isn\'t reachable over Wi-Fi. Check both are on the same network (not a guest network), then try again.',
        `no answer from ${lanHost} once the cable session ended`,
        true,
      );
    }
    return lanHost;
  }

  private async waitForLan(ctx: DriverContext, udid: string, host: string, tries: number): Promise<boolean> {
    for (let i = 0; i < tries; i++) {
      this.check(ctx);
      if (await this.probeLan(udid, host)) return true;
      await ctx.sleep(1000);
    }
    return false;
  }

  private async probeLan(udid: string, host?: string): Promise<string | null> {
    if (!host) return null;
    try {
      await this.deps.ictrl(udid, host).call('device_info', {}, 5000);
      return host;
    } catch {
      return null;
    }
  }

  private async finish(ctx: DriverContext, udid: string, record: IosLinkRecord, lanHost: string): Promise<void> {
    const client = this.deps.ictrl(udid, lanHost);
    let apps: PhoneLinkApp[];
    try {
      apps = iosVendorApps(await this.deps.goios.apps(udid));
    } catch {
      if (record.apps) {
        apps = record.apps;
      } else {
        // No cable and no earlier list: ask the runner. Each launch opens the app, so go Home after.
        const installed: string[] = [];
        for (const ids of Object.values(IOS_VENDOR_BUNDLES)) {
          for (const id of ids) {
            if (await client.call('launch_app', { bundle_id: id }).then(() => true, () => false)) installed.push(id);
          }
        }
        await client.call('home').catch(() => {});
        apps = iosVendorApps(installed);
      }
    }
    this.deps.store.save({ ...record, lanHost, apps });
    this.base = { ...this.base, apps };
    this.set(ctx, 'ready_wifi', 'Connected over Wi-Fi. You can unplug the cable.', { detail: lanHost, apps });
  }
}

/** First non-loopback IPv4 the runner reports for itself, as host:port. */
export function pickLanHost(config: { address?: string; ipv4_interfaces?: { name: string; address: string }[]; listen_port?: number }): string | null {
  const port = (config as { listen_port?: number }).listen_port ?? ICTRL_PORT;
  if (config.address && !config.address.startsWith('127.') && /^\d+\.\d+\.\d+\.\d+:\d+$/.test(config.address)) return config.address;
  const wifi = (config.ipv4_interfaces ?? []).find((i) => i.name.startsWith('en') && !i.address.startsWith('127.') && !i.address.startsWith('169.254.'));
  return wifi ? `${wifi.address}:${port}` : null;
}

/**
 * Bring a linked phone back after it restarted (DDI gone, runner stopped).
 * Needs the phone visible to usbmuxd: on the cable, or over Wi-Fi sync.
 * The phone's address may have changed, so it's read back from the runner
 * through the 127.0.0.1 forward. Returns the LAN host once it answers.
 */
export async function reconnectIos(
  deps: IosDriverDeps,
  udid: string,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
): Promise<string | null> {
  const record = deps.store.load(udid);
  if (!record) return null;
  const probe = async (host?: string) => (host ? deps.ictrl(udid, host).call('device_info', {}, 5000).then(() => host, () => null) : null);
  const direct = await probe(record.lanHost);
  if (direct) return direct;
  if (!(await deps.goios.list()).includes(udid)) return null;
  const tunnel = deps.goios.tunnelStart(udid);
  let fwd: { port: number; close(): Promise<void> } | null = null;
  try {
    for (let i = 0; i < 40 && !(await deps.goios.tunnels()).some((t) => t.udid === udid); i++) await sleep(500);
    await deps.goios.imageAuto(udid, await deps.ddiDir());
    const launched = await deps.goios.launch(udid, record.runnerBundleId, { killExisting: true });
    if (!launched.ok) return null;
    fwd = await deps.forward(udid, ICTRL_PORT);
    const usb = deps.ictrl(udid, `127.0.0.1:${fwd.port}`);
    for (let i = 0; i < 20; i++) {
      const config = await usb.call<Parameters<typeof pickLanHost>[0]>('config').catch(() => null);
      const host = config ? pickLanHost(config) : null;
      if (host && (await probe(host))) {
        deps.store.save({ ...record, lanHost: host });
        return host;
      }
      await sleep(1000);
    }
    return null;
  } catch (error) {
    deps.logger?.(`[phone-link ios] reconnect failed: ${error instanceof Error ? error.message : String(error)}`);
    return null;
  } finally {
    await fwd?.close();
    tunnel.stop();
  }
}
