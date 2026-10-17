/**
 * Phone Link: per-phone health. A linked phone is only useful to bots when it
 * actually answers, so `phone.devices` and Settings → Computers → Phones show
 * one of a small set of states, classified from evidence only — never guessed.
 *
 *   HEALTHY                              whoami matches + a sealed call works
 *   DEVICE_LOCKED                        answering, is_locked says locked
 *   PAIRING_INVALID                      whoami matches, the key was revoked
 *   TRANSPORT_OFFLINE                    nothing answers at the saved address
 *   HELPER_NOT_RESPONDING                whoami matches, the sealed call fails
 *   DEVELOPER_IMAGE_REMOUNT_REQUIRED     on the cable, no developer image
 *   SIGNATURE_RENEWAL_REQUIRED           helper signature expired/expiring
 *   UNKNOWN                              anything else
 *
 * Product-owner rule: DEVELOPER_IMAGE_REMOUNT_REQUIRED and
 * SIGNATURE_RENEWAL_REQUIRED are ALWAYS recovery 'manual' with a user action;
 * never mark them automatic.
 */

import type { PhoneLinkHealth, PhoneLinkHealthState } from './phone-link-types.js';

export type { PhoneLinkHealth, PhoneLinkHealthState };

/** A phone the Phone Link lane can serve (source injected by the backend). */
export interface LinkedPhone {
  deviceId: string;
  platform: 'ios' | 'android';
  name?: string;
  lanHost?: string;
  /** `<100.x>:8200` on the user's mesh, saved by setup once the phone's join is running. */
  meshHost?: string;
  /** Secret-store entry holding this phone's iCTRL key record. */
  storeKey: string;
  /** iOS only: when the free-Apple-ID signature lapses (ISO string). */
  expiresAt?: string;
  apps?: { vendor: string; signedIn: boolean | null }[];
}

export type LinkedPhoneSource = () => LinkedPhone[];

/** What a probe saw. `whoami` is null when nothing answered (or the answering device wasn't ours). */
export interface HealthEvidence {
  deviceId: string;
  platform: 'ios' | 'android';
  whoami: { deviceId: string } | null;
  call: 'ok' | 'locked' | 'needs_repair' | 'failed';
  /** Injected go-ios probe: phone on the cable, no developer image mounted. */
  remountRequired?: boolean;
  expiresAt?: string;
}

export const SIGNATURE_WARNING_MS = 24 * 3600_000;
const RESTART_DETAIL = 'The iPhone may have restarted; its developer image then needs remounting.';
const REMOUNT_DETAIL =
  'The phone is connected but has no developer image mounted. Or on the iPhone: open iCTRL › Mount without a Mac, then Start Server.';

function signatureAction(expiresAt: string): string {
  const date = new Date(expiresAt).toLocaleDateString('en-US', {
    timeZone: 'UTC',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });
  return `Your iPhone helper's signature runs out on ${date}. Plug in your iPhone to renew it.`;
}

function signatureExpired(expiresAt: string | undefined, now: number): boolean {
  const at = expiresAt ? Date.parse(expiresAt) : NaN;
  return Number.isFinite(at) && at <= now;
}

function signatureExpiring(expiresAt: string | undefined, now: number): boolean {
  const at = expiresAt ? Date.parse(expiresAt) : NaN;
  return Number.isFinite(at) && at - now <= SIGNATURE_WARNING_MS;
}

/** Classify one probe's evidence into a health object. Pure; unit-tested. */
export function classifyHealth(ev: HealthEvidence, now: number): PhoneLinkHealth {
  const base = { checkedAt: new Date(now).toISOString() };

  // Evidence-based only: without a probe that says the image is gone, a
  // silent phone stays TRANSPORT_OFFLINE.
  if (ev.remountRequired === true) {
    return {
      ...base,
      state: 'DEVELOPER_IMAGE_REMOUNT_REQUIRED',
      recovery: 'manual',
      action: 'Plug your iPhone into this computer and open Allternit',
      detail: REMOUNT_DETAIL,
    };
  }

  if (!ev.whoami) {
    if (ev.platform === 'ios' && signatureExpired(ev.expiresAt, now)) {
      return { ...base, state: 'SIGNATURE_RENEWAL_REQUIRED', recovery: 'manual', action: signatureAction(ev.expiresAt!) };
    }
    return {
      ...base,
      state: 'TRANSPORT_OFFLINE',
      recovery: 'automatic',
      detail: ev.platform === 'ios' ? RESTART_DETAIL : undefined,
    };
  }

  if (ev.whoami.deviceId !== ev.deviceId) {
    return { ...base, state: 'TRANSPORT_OFFLINE', recovery: 'automatic', detail: 'A different device answered at the saved address.' };
  }

  if (ev.call === 'needs_repair') {
    return {
      ...base,
      state: 'PAIRING_INVALID',
      recovery: 'manual',
      action: 'Pair this phone again in Settings › Computers › Phones.',
      detail: 'The phone no longer accepts this computer\'s key.',
    };
  }

  if (ev.call === 'failed') {
    return {
      ...base,
      state: 'HELPER_NOT_RESPONDING',
      recovery: 'automatic',
      detail: 'The phone answered but its helper did not complete a sealed call.',
    };
  }

  // The phone works; a lapsed (or about-to-lapse) helper signature still wins.
  if (ev.platform === 'ios' && signatureExpiring(ev.expiresAt, now)) {
    return { ...base, state: 'SIGNATURE_RENEWAL_REQUIRED', recovery: 'manual', action: signatureAction(ev.expiresAt!) };
  }

  if (ev.call === 'locked') {
    return { ...base, state: 'DEVICE_LOCKED', recovery: 'manual', action: 'Unlock your phone.' };
  }

  if (ev.call === 'ok') {
    return { ...base, state: 'HEALTHY', recovery: 'automatic' };
  }

  return { ...base, state: 'UNKNOWN', recovery: 'automatic', detail: 'Not enough evidence to tell what is wrong.' };
}

/** States that count as online for tool routing: the phone answers, even locked. */
export function healthIsOnline(health: PhoneLinkHealth | null | undefined): boolean {
  return !health || health.state === 'HEALTHY' || health.state === 'DEVICE_LOCKED';
}

export interface PhoneLinkHealthMonitorDeps {
  phones: LinkedPhoneSource;
  /** One phone → evidence. Throws are treated as "nothing answered". */
  probe: (phone: LinkedPhone) => Promise<HealthEvidence>;
  now?: () => number;
  /** How long a health entry stays fresh (default 30 s). */
  ttlMs?: number;
  /** Per-phone budget for a parallel refresh (default 10 s: room for one shared LAN scan when a phone moved). */
  budgetMs?: number;
  logger?: (message: string) => void;
}

/**
 * Caches health per phone so `phone.devices` and tool routing don't probe on
 * every call. Stale entries are refreshed in parallel, one probe per phone,
 * each with its own budget.
 */
export class PhoneLinkHealthMonitor {
  private readonly cache = new Map<string, { at: number; health: PhoneLinkHealth }>();
  private readonly now: () => number;
  private readonly ttlMs: number;
  private readonly budgetMs: number;

  constructor(private readonly deps: PhoneLinkHealthMonitorDeps) {
    this.now = deps.now ?? Date.now;
    this.ttlMs = deps.ttlMs ?? 30_000;
    this.budgetMs = deps.budgetMs ?? 10_000;
  }

  /** The cached entry whatever its age, or null when never checked. */
  health(deviceId: string): PhoneLinkHealth | null {
    return this.cache.get(deviceId)?.health ?? null;
  }

  /** The cached entry while it is fresh (younger than ttlMs). */
  fresh(deviceId: string): PhoneLinkHealth | null {
    const entry = this.cache.get(deviceId);
    return entry && this.now() - entry.at < this.ttlMs ? entry.health : null;
  }

  /** Probe one phone now and cache the result. */
  async check(deviceId: string): Promise<PhoneLinkHealth> {
    return this.checkBudgeted(deviceId, 0);
  }

  /** Refresh every stale (or missing) entry in parallel, each within the per-phone budget. */
  async refreshStale(deviceIds?: string[]): Promise<void> {
    const ids = deviceIds ?? this.deps.phones().map((p) => p.deviceId);
    const stale = ids.filter((id) => !this.fresh(id));
    await Promise.all(stale.map((id) => this.checkBudgeted(id, this.budgetMs)));
  }

  /** Probe within `budgetMs` (0 = no cap); a timeout counts as "nothing answered". */
  private async checkBudgeted(deviceId: string, budgetMs: number): Promise<PhoneLinkHealth> {
    const phone = this.deps.phones().find((p) => p.deviceId === deviceId);
    const evidence = phone
      ? budgetMs > 0
        ? await Promise.race([
            this.deps.probe(phone).catch(() => null),
            new Promise<null>((resolve) => setTimeout(() => resolve(null), budgetMs)),
          ])
        : await this.deps.probe(phone).catch(() => null)
      : null;
    const health = classifyHealth(
      evidence ?? { deviceId, platform: phone?.platform ?? 'ios', whoami: null, call: 'failed', expiresAt: phone?.expiresAt },
      this.now(),
    );
    this.cache.set(deviceId, { at: this.now(), health });
    return health;
  }
}
