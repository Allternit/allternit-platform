/**
 * Phone Link: the contract between Desktop main and the setup UI (onboarding
 * "Your AI" step and the standalone PhoneLinkSetup window in allternit-ai).
 *
 * Phone Link connects the AI apps already signed in on the user's own phone
 * so bots can use them (SPEC-signed-in-apps-routing-2026-10-16). iPhone: a
 * one-time cable setup installs our iCTRL-based runner, then it works over
 * Wi-Fi. Android: USB debugging, then wireless debugging.
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
  /** 'chatgpt' | 'claude' | 'kimi' | 'gemini' | 'copilot' | 'grok' | … */
  vendor: string;
  /** null = installed, sign-in not checked yet. */
  signedIn: boolean | null;
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

/** A linked phone as the UI's device list shows it (phone-link:devices). */
export interface PhoneLinkDevice {
  deviceId: string;
  platform: PhoneLinkPlatform;
  name?: string;
  route: 'iphone-app-ictrl' | 'android-app';
  health: PhoneLinkHealth;
  apps?: PhoneLinkApp[];
}

/** `window.allternit.phoneLink` */
export interface PhoneLinkAPI {
  status(): Promise<PhoneLinkStatus>;
  /** Linked phones with per-phone health, for Settings → Computers → Phones. */
  devices(): Promise<PhoneLinkDevice[]>;
  onStatusChanged(handler: (status: PhoneLinkStatus) => void): () => void;
  start(opts: { platform: PhoneLinkPlatform }): Promise<PhoneLinkStatus>;
  cancel(): Promise<PhoneLinkStatus>;
  /** The user did a step the phone can't report (see PHONE_LINK_MANUAL_STATES). */
  continue(): Promise<PhoneLinkStatus>;
  submitAppleId(creds: { appleId: string; password: string }): Promise<PhoneLinkStatus>;
  submitTwoFactor(code: string): Promise<PhoneLinkStatus>;
  retry(): Promise<PhoneLinkStatus>;
}
