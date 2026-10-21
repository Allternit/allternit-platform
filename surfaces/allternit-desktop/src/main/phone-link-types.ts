/**
 * Phone Link: the contract between Desktop main and the setup UI (onboarding
 * "Your AI" step and the standalone PhoneLinkSetup window in allternit-ai).
 *
 * Phone Link connects the AI apps already signed in on the user's own phone
 * so bots can use them (SPEC-signed-in-apps-routing-2026-10-16). iPhone: a
 * one-time cable setup installs our iCTRL-based runner, then it works over
 * Wi-Fi. Android: USB debugging, then the Phone Link app installs over the
 * same cable and takes over Wi-Fi itself.
 *
 * Credentials typed into the UI (Apple ID, 2FA code) cross IPC once, go to
 * the signing step in main, and are never stored in plain text or logged.
 */

export type PhoneLinkPlatform = 'ios' | 'android';

export type PhoneLinkState =
  | 'idle'
  | 'not_supported'
  // iPhone
  | 'needs_cable'
  | 'trust_prompt'
  | 'apple_id'
  | 'two_factor'
  | 'developer_mode'
  | 'waiting_for_reboot'
  | 'installing'
  // After installing: the certificate only shows in Settings once the helper is on the phone.
  | 'trust_certificate'
  // Android
  | 'usb_debugging'
  | 'pairing'
  // Both
  | 'ready_wifi'
  | 'error';

/**
 * States the phone can't report back; the UI shows "I've done it" → continue().
 * iPhone Developer Mode is polled (go-ios devmode get), so it isn't one.
 */
export const PHONE_LINK_MANUAL_STATES: readonly PhoneLinkState[] = [
  'trust_certificate',
  'usb_debugging',
];

export interface PhoneLinkApp {
  /** Phone app catalog id: 'chatgpt', 'claude', 'notion', 'telegram', … (phone-app-catalog.json). */
  vendor: string;
  /** null = installed, sign-in not checked yet. */
  signedIn: boolean | null;
  /** Display name from the catalog. */
  name?: string;
  /** Catalog category (the connector catalog's category when the app has a connector). */
  category?: string;
  /** Subscription gateway provider id when the app is a gateway vendor ('google' for Gemini). */
  provider?: string;
  /** Account email or @handle last read from the app's own account screen (phone-account-reader.ts), else null. */
  account?: string | null;
  /** Plan name the app showed (Free, Plus, Pro, Max, SuperGrok, …), else null. */
  plan?: string | null;
  /** Usage windows the app showed (5-hour, Weekly, …), else null. */
  usage?: PhoneLinkUsage | null;
  /** ISO time of the last account read that saw anything. */
  accountCheckedAt?: string;
  /** 'verified' when the account reader has a path confirmed on this phone's platform (phone-account-specs.ts); listed, not saved. */
  reader?: 'verified';
}

/** One usage window as an app shows it ("Current session … 34% used … Resets in 2 hr"). */
export interface PhoneLinkUsageWindow {
  /** 'five_hour', 'weekly', 'weekly_opus', 'daily', 'monthly', 'messages'. */
  id: string;
  /** '5-hour', 'Weekly', … */
  label: string;
  /** 0–100, or null when the screen didn't show it. */
  remaining_pct: number | null;
  /** ISO reset time, or null when the screen didn't show it. */
  resets_at: string | null;
}

export interface PhoneLinkUsage {
  windows: PhoneLinkUsageWindow[];
  /** ISO time the screen was read. */
  observedAt: string;
}

/** phone-link:read-account answer (allternit-ai src/lib/phone-link-bridge.ts). Fields the app didn't show are null; never guessed. */
export interface PhoneLinkAccountResult {
  ok: boolean;
  /** Whether the app's first screen was a sign-in screen (false), its signed-in UI (true), or neither (null). Absent on failures. */
  signedIn?: boolean | null;
  account: string | null;
  plan: string | null;
  usage: { windows: Array<{ id: string; label: string; remaining_pct: number | null; resets_at: string | null }>; observedAt: string } | null;
  error?: PhoneLinkError;
}

export interface PhoneLinkError {
  code: string;
  message: string;
  retryable: boolean;
}

export interface PhoneLinkStatus {
  platform: PhoneLinkPlatform | null;
  state: PhoneLinkState;
  /** One plain-English line telling the user what to do now. */
  instruction: string;
  detail?: string;
  /**
   * two_factor only: Apple sent the code to trusted devices, and this masked
   * number ("•••• 12") can get it by text instead (submitTwoFactor('sms')).
   */
  textTo?: string;
  /** App-relative asset path or data URL. */
  hintImage?: string;
  /** 0–1 while installing. */
  progress?: number;
  deviceName?: string;
  osVersion?: string;
  /** Filled once ready, for the result card and the connected graph. */
  apps?: PhoneLinkApp[];
  error?: PhoneLinkError;
}

/**
 * Health of a linked phone, from evidence only (phone-link-health.ts).
 * `recovery` says whether Desktop can fix it on its own ('automatic') or the
 * user must act ('manual'); `action` is the one-line instruction when manual.
 */
export type PhoneLinkHealthState =
  | 'HEALTHY'
  | 'TRANSPORT_OFFLINE'
  | 'PAIRING_INVALID'
  | 'DEVICE_LOCKED'
  | 'HELPER_NOT_RESPONDING'
  // Android: the Phone Link app answers but its accessibility service is off, so no screen read or tap works.
  | 'ACCESSIBILITY_OFF'
  | 'DEVELOPER_IMAGE_REMOUNT_REQUIRED'
  | 'SIGNATURE_RENEWAL_REQUIRED'
  | 'UNKNOWN';

export interface PhoneLinkHealth {
  state: PhoneLinkHealthState;
  /** ISO timestamp of the check. */
  checkedAt: string;
  /** What we saw, for the UI tooltip. */
  detail?: string;
  recovery: 'automatic' | 'manual';
  /** One short sentence telling the user what to do (manual states). */
  action?: string;
}

/** On a computer that received a phone by pairing handover: who gave it (phone-link-handover.ts). */
export interface HandedOverFrom {
  /** The giving computer's name, e.g. "Eoj's MacBook Pro". */
  name: string;
  at: string;
}

/** A computer this Desktop can hand a linked phone to: the person's bound Sessions computer. */
export interface PhoneLinkHandoverTarget {
  computerId: string;
  name: string;
  /** False when it can't take a phone now; `reason` says why in one sentence. */
  ready: boolean;
  reason?: string;
}

export type PhoneLinkHandoverResult =
  | { ok: true; deviceId: string; computerName: string }
  | { ok: false; cancelled?: boolean; error: string; message: string };

/** A linked phone as the UI's device list shows it (phone-link:devices). */
export interface PhoneLinkDevice {
  deviceId: string;
  platform: PhoneLinkPlatform;
  name?: string;
  route: 'iphone-app-ictrl' | 'android-app';
  health: PhoneLinkHealth;
  apps?: PhoneLinkApp[];
  /** Set on a Sessions computer for a phone the Mac handed over. */
  handedOverFrom?: HandedOverFrom;
}

/** `window.allternit.phoneLink` */
export interface PhoneLinkAPI {
  status(): Promise<PhoneLinkStatus>;
  /** Linked phones with per-phone health, for Settings → Connections › Setup. */
  devices(): Promise<PhoneLinkDevice[]>;
  onStatusChanged(handler: (status: PhoneLinkStatus) => void): () => void;
  /**
   * Android only: `serial` (`host:port`) runs setup on a phone adb reaches by
   * address, e.g. a cloud phone, instead of over USB; `linkHost` is where its
   * Phone Link app port is published (cloud phones' `linkAddress`).
   */
  start(opts: { platform: PhoneLinkPlatform; serial?: string; linkHost?: string }): Promise<PhoneLinkStatus>;
  cancel(): Promise<PhoneLinkStatus>;
  /** The user did a step the phone can't report (see PHONE_LINK_MANUAL_STATES). */
  continue(): Promise<PhoneLinkStatus>;
  submitAppleId(creds: { appleId: string; password: string }): Promise<PhoneLinkStatus>;
  submitTwoFactor(code: string): Promise<PhoneLinkStatus>;
  retry(): Promise<PhoneLinkStatus>;
  /** The person's Sessions computer, when one is bound (empty list otherwise). */
  handoverTargets(): Promise<PhoneLinkHandoverTarget[]>;
  /** Give a linked phone's pairing to the Sessions computer. Asks the person to confirm on this computer first. */
  handover(opts: { deviceId: string; computerId: string }): Promise<PhoneLinkHandoverResult>;
  /** Connections wizard: open a catalog app (phone-app-catalog.json id) on a linked phone so the person can sign in there. */
  openApp(opts: { deviceId: string; app: string }): Promise<PhoneLinkAppResult>;
  /** Connections wizard: open the app, read its screen, report whether it is signed in (and as whom, when shown). */
  checkApp(opts: { deviceId: string; app: string }): Promise<PhoneLinkAppResult>;
  /** Read-only: open the app's own account/usage screen and read account, plan and usage windows. Never types or toggles. */
  readAccount(opts: { deviceId: string; app: string }): Promise<PhoneLinkAccountResult>;
}

/** phone-link:open-app / phone-link:check-app answer (allternit-ai src/lib/phone-link-bridge.ts). */
export interface PhoneLinkAppResult {
  ok: boolean;
  /** null = the screen didn't show either way (always null from open-app). */
  signedIn: boolean | null;
  /** An email or @handle the app plainly shows, else null. */
  account?: string | null;
  error?: PhoneLinkError;
}
