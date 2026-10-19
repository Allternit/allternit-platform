/**
 * Read-only navigation specs for the account reader (phone-account-reader.ts):
 * how to get from an AI chat app's main screen to the screen that shows the
 * signed-in account, the plan and the usage windows.
 *
 * Every spec is UNVERIFIED (`verified: false`): the phones weren't readable
 * when these were written, so each vendor lists several candidate paths and
 * several candidate labels per tap; the reader tries them in order and
 * returns whatever it could read. Flip `verified` to true only after the
 * path has been confirmed on a real phone, and drop the candidates that
 * never matched.
 *
 * Safety rules the reader enforces on top of these specs:
 *   - taps go only to an element whose whole label (or resource id) equals a
 *     candidate below, via phone.find_and_tap with exact matching;
 *   - DENY_LABEL is checked against every element before it is tapped, so a
 *     candidate never reaches "Sign out", "Delete", "Upgrade", "Buy",
 *     "Subscribe", "Cancel", …;
 *   - switches, toggles, check boxes and sliders are never tapped;
 *   - nothing is ever typed;
 *   - the app is always left with Home at the end, even after a failure.
 */

/**
 * Tap one element: the first candidate found on screen wins. A candidate is
 * a whole label / content description, compared case-insensitively, or
 * `id:<resource-id>` (Android; matches the full id or the part after ":id/").
 */
export interface AccountTapStep {
  tap: readonly string[];
  /** When none of the candidates is on screen, skip this step instead of abandoning the path. */
  optional?: boolean;
}

/** Read the screen and parse it (parseAccountScreen). `scroll` also reads once more after one swipe up. */
export interface AccountReadStep {
  read: true;
  scroll?: boolean;
}

export type AccountStep = AccountTapStep | AccountReadStep;

export interface AccountPath {
  /** For logs: which path got the answer. */
  name: string;
  steps: readonly AccountStep[];
}

export interface PhoneAccountSpec {
  /** Phone app catalog id (phone-app-catalog.json). */
  app: string;
  /** False until the path is confirmed on a real phone. */
  verified: boolean;
  /** Plan names this vendor uses, longest first is not required (the parser sorts). */
  plans: readonly string[];
  /** Words a plan label may be prefixed with ("ChatGPT Plus", "Claude Max"). */
  vendorWords: readonly string[];
  paths: readonly AccountPath[];
}

/**
 * Never tapped, whatever a spec says: anything that signs out, deletes,
 * pays, changes a plan, cancels, resets or signs in. Checked against the
 * element's normalized label (lower case, single spaces).
 */
export const DENY_LABEL =
  /\b(sign|log)\s?(out|off|in|up)\b|\bdelete\b|\bremove\b|\berase\b|\bdeactivate\b|\bclose (your )?account\b|\bupgrade\b|\bdowngrade\b|\bbuy\b|\bpurchase|\bsubscribe\b|\bunsubscribe\b|\bcancel|\bpay\b|\bpayment|\bbilling\b|\bmanage (your )?(subscription|plan|membership)\b|\brestore purchases\b|\btry\b|\bget (plus|pro|max|premium|supergrok|go|advanced|ultra)\b|\bstart (a |your )?(free )?trial\b|\bswitch (account|plan|to)\b|\breset\b|\bclear\b|\badd account\b|\breport\b|\bblock\b|\bshare\b|\bexport\b|\barchive\b/;

/** Element classes that change state when tapped: never tapped. */
export const DENY_CLASS = /switch|toggle|checkbox|check_box|radio|seekbar|slider|picker/i;

/** True when an element must not be tapped (label or class on the deny lists). */
export function deniedElement(label: string, className = ''): boolean {
  const normed = label.toLowerCase().replace(/[’]/g, "'").replace(/\s+/g, ' ').trim();
  return DENY_LABEL.test(normed) || DENY_CLASS.test(className);
}

const read: AccountReadStep = { read: true, scroll: true };

/** Opening the app's menu / sidebar / profile, shared by most vendors. */
const MENU = ['Open sidebar', 'Open menu', 'Menu', 'Show sidebar', 'Open navigation drawer', 'Navigation drawer'] as const;
const PROFILE = ['Profile', 'Account', 'Your account', 'Open profile', 'Profile picture', 'Account and settings', 'Me', 'id:profile', 'id:avatar'] as const;
const SETTINGS = ['Settings', 'Open settings', 'Settings and help', 'id:settings'] as const;

export const ACCOUNT_SPECS: Readonly<Record<string, PhoneAccountSpec>> = {
  chatgpt: {
    app: 'chatgpt',
    verified: false,
    plans: ['Free', 'Go', 'Plus', 'Pro', 'Team', 'Business', 'Enterprise', 'Edu'],
    vendorWords: ['ChatGPT'],
    paths: [
      { name: 'sidebar → settings → account', steps: [{ tap: MENU, optional: true }, { tap: [...SETTINGS, ...PROFILE] }, read, { tap: ['Account', 'Subscription', 'Plan'], optional: true }, read] },
      { name: 'profile → settings', steps: [{ tap: PROFILE }, { tap: SETTINGS, optional: true }, read] },
    ],
  },
  claude: {
    app: 'claude',
    verified: false,
    plans: ['Free', 'Pro', 'Max', 'Team', 'Enterprise'],
    vendorWords: ['Claude'],
    paths: [
      { name: 'menu → settings → usage', steps: [{ tap: MENU, optional: true }, { tap: [...SETTINGS, ...PROFILE] }, read, { tap: ['Usage', 'Plan usage', 'Usage limits', 'Plan usage limits'], optional: true }, read] },
      { name: 'profile → usage', steps: [{ tap: PROFILE }, { tap: ['Usage', 'Plan usage', 'Usage limits'], optional: true }, read] },
    ],
  },
  kimi: {
    app: 'kimi',
    verified: false,
    plans: ['Free', 'Adagio', 'Andante', 'Moderato', 'Allegretto', 'Allegro', 'Vivace'],
    vendorWords: ['Kimi'],
    paths: [
      { name: 'me → membership', steps: [{ tap: MENU, optional: true }, { tap: [...PROFILE, ...SETTINGS] }, read, { tap: ['Membership', 'Subscription', 'Plan', 'My plan', 'Usage'], optional: true }, read] },
      { name: 'settings', steps: [{ tap: SETTINGS }, read] },
    ],
  },
  gemini: {
    app: 'gemini',
    verified: false,
    plans: ['Free', 'Advanced', 'Google AI Plus', 'Google AI Pro', 'Google AI Ultra', 'AI Pro', 'AI Ultra'],
    vendorWords: ['Gemini', 'Google'],
    paths: [
      { name: 'account → settings', steps: [{ tap: PROFILE }, read, { tap: ['Settings', 'Subscriptions', 'Your plan'], optional: true }, read] },
      { name: 'menu → settings', steps: [{ tap: MENU, optional: true }, { tap: SETTINGS }, read] },
    ],
  },
  perplexity: {
    app: 'perplexity',
    verified: false,
    plans: ['Free', 'Pro', 'Max', 'Enterprise Pro', 'Enterprise Max', 'Education Pro'],
    vendorWords: ['Perplexity'],
    paths: [
      { name: 'profile → account', steps: [{ tap: MENU, optional: true }, { tap: [...PROFILE, ...SETTINGS] }, read, { tap: ['Account', 'Subscription', 'Plan'], optional: true }, read] },
      { name: 'settings', steps: [{ tap: SETTINGS }, read] },
    ],
  },
  grok: {
    app: 'grok',
    verified: false,
    plans: ['Free', 'SuperGrok', 'SuperGrok Heavy', 'X Premium', 'X Premium+', 'Premium', 'Premium+'],
    vendorWords: ['Grok'],
    paths: [
      { name: 'sidebar → settings → account', steps: [{ tap: MENU, optional: true }, { tap: [...SETTINGS, ...PROFILE] }, read, { tap: ['Account', 'Subscription', 'Usage'], optional: true }, read] },
      { name: 'profile', steps: [{ tap: PROFILE }, read] },
    ],
  },
  copilot: {
    app: 'copilot',
    verified: false,
    plans: ['Free', 'Pro', 'Copilot Pro', 'Microsoft 365 Personal', 'Microsoft 365 Family', 'Microsoft 365 Premium'],
    vendorWords: ['Copilot', 'Microsoft'],
    paths: [
      { name: 'profile → account', steps: [{ tap: MENU, optional: true }, { tap: [...PROFILE, ...SETTINGS] }, read, { tap: ['Account', 'Subscription', 'Plan'], optional: true }, read] },
      { name: 'settings', steps: [{ tap: SETTINGS }, read] },
    ],
  },
};

export function accountSpec(appId: string): PhoneAccountSpec | null {
  return ACCOUNT_SPECS[appId] ?? null;
}
