/**
 * iPhone recovery without a person, where iOS lets Desktop do it:
 *
 *   1. Renewal. A free Apple ID signature lasts 7 days (IosLinkRecord.expiresAt
 *      holds it). Once less than RESIGN_WINDOW_MS (48 h) remain and the phone
 *      is installable, re-sign with the Keychain-saved Apple session (empty
 *      password — no prompt, no 2FA), reinstall the runner + host app, relaunch
 *      the helper over the cable and confirm it answers, then persist the new
 *      expiry. When the session needs the password/2FA, or the phone can't be
 *      installed on, nothing is forced: health keeps reporting
 *      SIGNATURE_RENEWAL_REQUIRED ('manual') and the setup flow handles it.
 *
 *   2. Remount. After an iPhone restart the Developer Disk Image is gone and
 *      the helper can't start until it's remounted. Each pass watches usbmuxd
 *      (go-ios `list`, cheap); when a linked iPhone appears and its helper
 *      doesn't answer, run the existing reconnectIos path (image auto → launch
 *      → confirm) with zero clicks — the "plug it in once after a restart"
 *      promise from the docs.
 *
 * What "installable" means, measured against the pinned go-ios v1.3.2
 * (github.com/danielpaulus/go-ios, tag v1.3.2):
 *   - `ios tunnel start`'s tunnel manager never tunnels a Wi-Fi device:
 *     shouldSkipDevice (ios/tunnel/tunnel_api.go) returns true for
 *     ConnectionType == "Network" (issues #596/#532). On iOS 17+ both the
 *     developer-image mount (`ios image auto`) and `ios launch` (instruments
 *     over the RSD tunnel) need that tunnel, so neither works over Wi-Fi.
 *   - `ios install` itself goes over plain lockdown (installation_proxy) and
 *     can reach a network (Wi-Fi sync) device — but without a working launch
 *     it can't bring the helper back, so renewal as a whole needs the cable.
 * So: a phone is installable exactly when it shows up in `ios list` on USB.
 *
 * The product-owner rule holds: the two health states stay recovery 'manual'
 * unless a pass here actually succeeded for that phone — success is reported
 * through onRecovered so the health monitor re-probes on evidence.
 */

import * as path from 'node:path';
import { reconnectIos, type IosDriverDeps, type IosLinkRecord } from './phone-link-ios.js';
import { SigningError, type RunnerArtifacts, type SignedRunner } from './phone-ios-runner.js';

/** The free-Apple-ID signature lasts 7 days; start renewing this long before it lapses. */
export const RESIGN_WINDOW_MS = 48 * 3600_000;

/** How often the scheduler watches usbmuxd and the renewal window (default 60 s). */
export const IOS_RECOVERY_INTERVAL_MS = 60_000;

/**
 * Minimum gap between renewal attempts for one phone. Signing talks to
 * Apple's sign-in; retrying every pass would hammer it (and can get an Apple
 * ID rate-limited or locked) when a renewal keeps needing a person.
 */
export const RENEWAL_RETRY_MS = 6 * 3600_000;

const brief = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/** True when the record's signature expiry is known and inside the renewal window. */
export function renewalDue(record: IosLinkRecord, at: number): boolean {
  const expires = record.expiresAt ? Date.parse(record.expiresAt) : NaN;
  return Number.isFinite(expires) && expires - at <= RESIGN_WINDOW_MS;
}

export interface IosRecoveryDeps extends IosDriverDeps {
  /** Test hook; reconnectIos's default is a real setTimeout. */
  sleep?: (ms: number) => Promise<void>;
  /** Called after a pass actually recovered a phone, so health re-probes it. */
  onRecovered?: (udid: string, kind: 'renewal' | 'remount') => void;
}

/**
 * Re-sign and reinstall the helper before its signature lapses. Returns
 * 'renewed' when signing + install succeeded (the helper is relaunched and
 * confirmed best-effort), 'manual' when anything needed a person — the record
 * is only written on success, so a failed attempt never corrupts state.
 */
export async function renewIosSignature(deps: IosRecoveryDeps, record: IosLinkRecord, connected: string[]): Promise<'renewed' | 'manual'> {
  const log = (message: string) => deps.logger?.(`[phone-link ios] ${message}`);
  const name = record.deviceName ?? record.udid;
  if (!connected.includes(record.udid)) {
    log(`the helper's signature for ${name} is inside the renewal window, but the phone isn't on the cable; renewal needs it plugged in`);
    return 'manual';
  }
  if (deps.signer.needsAppleId && !record.appleId) {
    log(`the helper's signature for ${name} is inside the renewal window, but this link has no Apple ID on record; run Phone Link setup once to enable automatic renewal`);
    return 'manual';
  }
  let artifacts: RunnerArtifacts;
  try {
    artifacts = await deps.artifacts();
  } catch (error) {
    log(`couldn't fetch the helper to renew it: ${brief(error)}`);
    return 'manual';
  }
  let signed: SignedRunner;
  try {
    signed = await deps.signer.sign({
      udid: record.udid,
      deviceName: record.deviceName ?? 'iPhone',
      artifacts,
      workDir: path.join(deps.workDir, 'signed', record.udid),
      // Empty password: the signer reuses the Apple session saved in the
      // Keychain. No twoFactor callback on purpose — a renewal never prompts;
      // if Apple wants a code anyway, the answer is 'manual'.
      credentials: deps.signer.needsAppleId ? { appleId: record.appleId!, password: '' } : undefined,
    });
  } catch (error) {
    if (error instanceof SigningError && error.code === 'password_required') {
      log(`the saved Apple sign-in for ${name} expired; plug the phone in and run Phone Link setup to sign in again`);
    } else {
      const code = error instanceof SigningError ? error.code : 'signing_failed';
      log(`re-signing the helper for ${name} failed (${code}); leaving renewal to Phone Link setup`);
    }
    return 'manual';
  }
  try {
    await deps.goios.install(record.udid, signed.hostIpa);
    await deps.goios.install(record.udid, signed.runnerIpa);
  } catch (error) {
    log(`re-signed the helper for ${name} but couldn't install it: ${brief(error)}`);
    return 'manual';
  }
  // Only now is the record updated — install is what renews the signature.
  deps.store.save({ ...record, runnerBundleId: signed.runnerBundleId, hostBundleId: signed.hostBundleId, expiresAt: signed.expiresAt ?? record.expiresAt });
  log(`helper for ${name} re-signed (valid until ${signed.expiresAt ?? 'unknown'}); relaunching it`);
  // Installing kills the running helper, so always relaunch: tunnel → image
  // auto → launch → confirm (which also refreshes the pairing file the
  // phone-side Mount-without-a-Mac flow reads).
  const lan = await reconnectIos(deps, record.udid, deps.sleep, { forceCable: true });
  if (lan) {
    log(`helper for ${name} renewed and answering at ${lan}`);
    deps.onRecovered?.(record.udid, 'renewal');
  } else {
    log(`helper for ${name} renewed but isn't answering yet; a later pass retries the relaunch`);
  }
  return 'renewed';
}

/**
 * A linked iPhone is on the cable and its helper doesn't answer: run the
 * reconnect path (image auto → launch → confirm through the paired key) and
 * report success through onRecovered so health re-probes on evidence.
 */
export async function remountIosHelper(deps: IosRecoveryDeps, record: IosLinkRecord): Promise<boolean> {
  const log = (message: string) => deps.logger?.(`[phone-link ios] ${message}`);
  const name = record.deviceName ?? record.udid;
  log(`${name} is on the cable and its helper doesn't answer; remounting the developer image and relaunching the helper`);
  const lan = await reconnectIos(deps, record.udid, deps.sleep);
  if (!lan) {
    log(`couldn't bring the helper back on the cable this time; will retry on a later pass`);
    return false;
  }
  log(`helper for ${name} is back, answering at ${lan}`);
  deps.onRecovered?.(record.udid, 'remount');
  return true;
}

/** Cheap answer check at the runner's saved LAN address (sealed call, 5 s). */
async function helperAnswers(deps: IosRecoveryDeps, record: IosLinkRecord): Promise<boolean> {
  if (!record.lanHost) return false;
  try {
    await deps.ictrl(record.udid, record.lanHost).call('device_info', {}, 5000);
    return true;
  } catch {
    return false;
  }
}

export interface IosRecoverySchedulerOpts {
  /**
   * Lazy deps: return null to skip a pass (no linked iPhone — the common
   * case, one small JSON read — or go-ios/signing tooling unavailable).
   */
  deps: () => Promise<IosRecoveryDeps | null>;
  intervalMs?: number;
  logger?: (message: string) => void;
}

/**
 * The Desktop-main scheduler: one pass per interval (plus one at start),
 * serialized so a slow pass never overlaps itself and a phone never gets two
 * remounts/renewals racing (the per-UDID in-flight set).
 */
export class IosRecoveryScheduler {
  private timer: ReturnType<typeof setInterval> | null = null;
  private readonly inFlight = new Set<string>();
  /** udid → when renewal was last attempted (success or not). */
  private readonly lastRenewal = new Map<string, number>();
  private chain: Promise<void> = Promise.resolve();

  constructor(private readonly opts: IosRecoverySchedulerOpts) {}

  start(): void {
    if (this.timer) return;
    void this.tick();
    this.timer = setInterval(() => void this.tick(), this.opts.intervalMs ?? IOS_RECOVERY_INTERVAL_MS);
    this.timer.unref?.();
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  /** Run one pass now; passes are chained, so concurrent calls queue up. */
  tick(): Promise<void> {
    const next = this.chain.then(() => this.run(), () => this.run());
    this.chain = next.catch(() => {});
    return next;
  }

  private async run(): Promise<void> {
    const log = (message: string) => this.opts.logger?.(`[phone-link ios] ${message}`);
    let deps: IosRecoveryDeps | null = null;
    try {
      deps = await this.opts.deps();
    } catch (error) {
      log(`recovery pass skipped: ${brief(error)}`);
      return;
    }
    if (!deps) return;
    const records = deps.store.all();
    if (records.length === 0) return;
    const connected = await deps.goios.list().catch(() => [] as string[]);
    const now = deps.now?.() ?? Date.now();
    for (const record of records) {
      if (this.inFlight.has(record.udid)) continue; // still being worked on by an earlier pass
      const last = this.lastRenewal.get(record.udid);
      const renew = renewalDue(record, now) && (last === undefined || now - last >= RENEWAL_RETRY_MS);
      if (!renew && !connected.includes(record.udid)) continue;
      this.inFlight.add(record.udid);
      try {
        if (renew) {
          // Only a pass that could actually try (phone on the cable) counts as an attempt.
          if (connected.includes(record.udid)) this.lastRenewal.set(record.udid, now);
          await renewIosSignature(deps, record, connected);
        } else if (!(await helperAnswers(deps, record))) {
          await remountIosHelper(deps, record);
        }
      } catch (error) {
        // One phone must never take down the whole pass.
        log(`recovery failed for ${record.deviceName ?? record.udid}: ${brief(error)}`);
      } finally {
        this.inFlight.delete(record.udid);
      }
    }
  }
}
