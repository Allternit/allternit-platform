/**
 * Read-only navigation specs for the account reader (phone-account-reader.ts):
 * how to get from an app's main screen to the screen that shows the
 * signed-in account and, where the app has them, the plan and the usage
 * windows. Every app on a linked phone gets a check (signed in or not); an
 * app without a spec here gets only that sign-in check.
 *
 * A path marked `verifiedOn` was walked on a real phone, screen by screen,
 * with every Settings page scrolled (2026-10-19, iPhone). Once a platform has
 * verified paths for an app, only those run there. Unverified paths list
 * several candidate labels per tap; the reader tries them in order and
 * returns whatever it could read.
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
 * a whole label / content description, compared case-insensitively,
 * `id:<resource-id>` (the iPhone accessibility id, or Android's full id or
 * the part after ":id/"), or `re:<pattern>`, a case-insensitive pattern the
 * whole label must match (for labels that carry the person's name, like
 * Discord's "<name>, Online"). A label only in the visible text (no element,
 * like Telegram's tab bar) is tapped where the text is.
 */
export interface AccountTapStep {
  tap: readonly string[];
  /** When none of the candidates is on screen, skip this step instead of abandoning the path. */
  optional?: boolean;
  /** Swipe up at most this many times looking for a candidate (a row further down a Settings list). */
  scroll?: number;
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
  /** The phone platform this path was confirmed on. A phone runs only its platform's verified paths when it has any. */
  verifiedOn?: 'ios' | 'android';
  steps: readonly AccountStep[];
}

/**
 * Where the app's own account is on a read screen; the first rule that finds
 * one wins. `email` = the first email, `handle` = the first label that is a
 * whole "@handle", `match` = group 1 of a pattern tried on each label,
 * `atId` = the label of the element with that id,
 * `after` / `before` = the nearest non-empty label after / before an anchor
 * (a whole label, or `id:<id>`).
 */
export type AccountRule =
  | { kind: 'email' }
  | { kind: 'handle' }
  | { match: RegExp }
  | { after: string }
  | { before: string }
  /** The label of the element with this id (Android: the part after ":id/"). */
  | { atId: string };

export interface PhoneAccountSpec {
  /** Phone app catalog id (phone-app-catalog.json). */
  app: string;
  /** False until a path is confirmed on a real phone. */
  verified: boolean;
  /** Plan names this vendor uses, longest first is not required (the parser sorts). Empty: the app shows no plan. */
  plans: readonly string[];
  /** Words a plan label may be prefixed with ("ChatGPT Plus", "Claude Max"). */
  vendorWords: readonly string[];
  /**
   * How to find the account. Omitted: an email, else a whole-label @handle
   * (fine for AI chat apps; a feed app needs its own rule, since its screens
   * show other people's handles and emails).
   */
  account?: readonly AccountRule[];
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
/** Verified paths read what's on screen without swiping: a swipe can push the next tap's row off screen. */
const readHere: AccountReadStep = { read: true };

/**
 * Optional first taps that leave a screen the path itself visits: an app
 * reopens where it was left (the iPhone helper resumes it, it doesn't
 * restart it), so a read cut short last time must not strand this one. Only
 * Back / Close controls the path's own screens were seen to have.
 */
/** Google's Android apps: the account button reads "Signed in as <name> <email>\nAccount and settings." */
const GOOGLE_SIGNED_IN: AccountRule = { match: /signed in as[\s\S]*?([A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,})/i };

/** Google apps on Android: the home screen's account button names the account (checked 2026-10-19, moto g play). */
const googleAccountApp = (app: string): PhoneAccountSpec => ({
  app,
  verified: true,
  plans: [],
  vendorWords: [],
  account: [GOOGLE_SIGNED_IN],
  paths: [{ name: 'home screen account button (Android, verified)', verifiedOn: 'android', steps: [readHere] }],
});

/** Discord's profile back button carries the unread mention count. */
const DISCORD_PROFILE_BACK = 're:^back( [\\d,]+ mentions?)?$';

const leave = (...labels: string[]): AccountTapStep[] => labels.map((label) => ({ tap: [label], optional: true }));

/** Opening the app's menu / sidebar / profile, shared by most vendors. */
const MENU = ['Open sidebar', 'Open menu', 'Menu', 'Show sidebar', 'Open navigation drawer', 'Navigation drawer'] as const;
const PROFILE = ['Profile', 'Account', 'Your account', 'Open profile', 'Profile picture', 'Account and settings', 'Me', 'id:profile', 'id:avatar'] as const;
const SETTINGS = ['Settings', 'Open settings', 'Settings and help', 'id:settings'] as const;

export const ACCOUNT_SPECS: Readonly<Record<string, PhoneAccountSpec>> = {
  chatgpt: {
    app: 'chatgpt',
    // Checked 2026-10-19 on iPhone (ChatGPT iOS): Menu → "Open Settings" shows
    // "Email, <email>" and "Subscription, Plus" on page 1 of 3; "Usage and
    // limits" is on page 2 and shows "Weekly usage limit", "95% remaining",
    // "Resets Sat 11:15 PM" (plus a Credits balance).
    verified: true,
    plans: ['Free', 'Go', 'Plus', 'Pro', 'Team', 'Business', 'Enterprise', 'Edu'],
    vendorWords: ['ChatGPT'],
    paths: [
      {
        name: 'menu → settings → usage and limits (iOS, verified)',
        verifiedOn: 'ios',
        steps: [
          ...leave('id:BackButton', 'Close'),
          { tap: ['Menu'], optional: true },
          { tap: ['Open Settings'] },
          readHere,
          { tap: ['Usage and limits'], scroll: 2, optional: true },
          readHere,
          { tap: ['id:BackButton'], optional: true },
          { tap: ['Close'], optional: true },
        ],
      },
      { name: 'sidebar → settings → account', steps: [{ tap: MENU, optional: true }, { tap: [...SETTINGS, ...PROFILE] }, read, { tap: ['Account', 'Subscription', 'Plan'], optional: true }, read] },
      { name: 'profile → settings', steps: [{ tap: PROFILE }, { tap: SETTINGS, optional: true }, read] },
    ],
  },
  claude: {
    app: 'claude',
    // Checked 2026-10-19 on iPhone (Claude iOS): sidebar → account button
    // ("<name>, Settings") shows "Personal, <email>" and "Billing, Max plan";
    // Settings → Usage shows "Current session, N% used, …", "Weekly limits",
    // "All models, N% used, Resets Thu 8:59 AM", "<Model> only, …".
    verified: true,
    plans: ['Free', 'Pro', 'Max', 'Team', 'Enterprise'],
    vendorWords: ['Claude'],
    paths: [
      { name: 'sidebar → account → usage (iOS, verified)', verifiedOn: 'ios', steps: [...leave('id:BackButton', 'Close'), { tap: ['id:drawer-toggle', 'Open sidebar'], optional: true }, { tap: ['id:sidebar-account-button'] }, readHere, { tap: ['Usage'] }, readHere, { tap: ['id:BackButton'], optional: true }, { tap: ['Close'], optional: true }] },
      { name: 'menu → settings → usage', steps: [{ tap: MENU, optional: true }, { tap: [...SETTINGS, ...PROFILE] }, read, { tap: ['Usage', 'Plan usage', 'Usage limits', 'Plan usage limits'], optional: true }, read] },
      { name: 'profile → usage', steps: [{ tap: PROFILE }, { tap: ['Usage', 'Plan usage', 'Usage limits'], optional: true }, read] },
    ],
  },
  kimi: {
    app: 'kimi',
    // 2026-10-19 on iPhone: signed out ("Session expired. Please log in
    // again." / "Log in"), so its Settings → Usage path isn't walked yet.
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
    // Checked 2026-10-19 on iPhone (Gemini iOS): the home screen's account
    // button is labelled "Account and settings. Signed in as <name>, <email>".
    // It opens the account sheet → "Settings " (trailing space) → "Usage
    // limits": "Current usage, 0% used, Resets at 8:54 PM" and "Weekly limit,
    // 0% used, Resets at 6:54 PM". "Your plan" names no plan on the free tier
    // (only an "Upgrade to Google AI Plus" offer), so the plan stays unread.
    verified: true,
    plans: ['Free', 'Advanced', 'Google AI Plus', 'Google AI Pro', 'Google AI Ultra', 'AI Pro', 'AI Ultra'],
    vendorWords: ['Gemini', 'Google'],
    paths: [
      {
        name: 'account → settings → usage limits (iOS, verified)',
        verifiedOn: 'ios',
        steps: [
          ...leave('Close', 'Close', 'Done'),
          readHere,
          { tap: ['id:kOGLAccessibilityIdentifierAccountParticleDiscButton'] },
          { tap: ['Settings'] },
          { tap: ['Usage limits'], scroll: 1 },
          readHere,
          { tap: ['Close'], optional: true },
          { tap: ['Close'], optional: true },
          { tap: ['Done'], optional: true },
        ],
      },
      { name: 'account → settings', steps: [{ tap: PROFILE }, read, { tap: ['Settings', 'Subscriptions', 'Your plan'], optional: true }, read] },
      { name: 'menu → settings', steps: [{ tap: MENU, optional: true }, { tap: SETTINGS }, read] },
    ],
  },
  perplexity: {
    app: 'perplexity',
    // 2026-10-19 on iPhone: signed out (a "Sign in" button), not walked yet.
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

  // ── Apps that aren't AI chat: the account (none of them shows a plan or usage) ──
  // All checked 2026-10-19 on iPhone and on Android (moto g play), signed in, every screen read.
  // Signed out on the moto then: ChatGPT, Facebook, Telegram, Stripe, Square (the sign-in check covers them).

  discord: {
    app: 'discord',
    // Home → the "<name>, Online" button (bottom left) → Settings → Account:
    // "Username, <name>", "Email, <email>". The profile's back button reads
    // "Back 19,457 mentions".
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ kind: 'email' }, { match: /^username, (.+)$/i }],
    paths: [
      {
        name: 'profile → settings → account (iOS, verified)',
        verifiedOn: 'ios',
        steps: [
          ...leave('Back', 'Back', DISCORD_PROFILE_BACK),
          { tap: ['re:^.+, (online|idle|do not disturb|invisible|offline)$'] },
          { tap: ['Settings'] },
          { tap: ['Account'] },
          readHere,
          ...leave('Back', 'Back', DISCORD_PROFILE_BACK),
        ],
      },
      {
        // Same rows on Android ("<name>, Online" → Settings → Account); Back steps out at the end.
        name: 'profile → settings → account (Android, verified)',
        verifiedOn: 'android',
        steps: [...leave('Back', 'Back', 'Back'), { tap: ['re:^.+, (online|idle|do not disturb|invisible|offline)$'] }, { tap: ['Settings'] }, { tap: ['Account'] }, readHere],
      },
    ],
  },
  telegram: {
    app: 'telegram',
    // The tab bar (Contacts · Calls · Chats · Settings) is only visible text,
    // not elements. Settings shows "<name>" and "<phone>•@<handle>".
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ match: /(?:^|[\s•·|])(@[a-z0-9_]{4,32})$/i }],
    paths: [{ name: 'settings tab (iOS, verified)', verifiedOn: 'ios', steps: [{ tap: ['Settings'] }, readHere] }],
  },
  instagram: {
    app: 'instagram',
    // Profile tab (id profile-tab): the username is the button just before
    // "Tap to open creation menu" (id profile-add-button).
    verified: true,
    plans: [],
    vendorWords: [],
    // Android: Profile tab (id profile_tab), the username is the title bar (id action_bar_title).
    account: [{ before: 'id:profile-add-button' }, { atId: 'action_bar_title' }],
    paths: [
      { name: 'profile tab (iOS, verified)', verifiedOn: 'ios', steps: [{ tap: ['id:profile-tab'] }, readHere] },
      // Optional tap: Instagram can reopen on the profile already (the title is then read where it is).
      { name: 'profile tab (Android, verified)', verifiedOn: 'android', steps: [{ tap: ['id:profile_tab'], optional: true }, readHere] },
    ],
  },
  facebook: {
    app: 'facebook',
    // "Your profile" tab: the name follows "Edit profile picture". The bio
    // can hold someone else's email, so emails are never taken here.
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ after: 'Edit profile picture' }],
    paths: [{ name: 'your profile tab (iOS, verified)', verifiedOn: 'ios', steps: [{ tap: ['Your profile'] }, readHere] }],
  },
  tiktok: {
    app: 'tiktok',
    // Profile tab (id a11y_vo_profile; while a video plays the element tree
    // can stop answering, then the visible "Profile" text is tapped): the
    // person's "@handle". The feed shows other people's handles, so nothing
    // is read before the tap.
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ kind: 'handle' }],
    paths: [
      { name: 'profile tab (iOS, verified)', verifiedOn: 'ios', steps: [{ tap: ['id:a11y_vo_profile', 'Profile'] }, readHere] },
      { name: 'profile tab (Android, verified)', verifiedOn: 'android', steps: [{ tap: ['Profile'] }, readHere] },
    ],
  },
  twitter: {
    app: 'twitter',
    // "Account Menu" (id NavigationBarDashButton): name and "@handle" (wrapped
    // in bidi marks). Its "Subscription" row is an upgrade page, so it isn't opened.
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ kind: 'handle' }],
    paths: [
      { name: 'account menu (iOS, verified)', verifiedOn: 'ios', steps: [{ tap: ['id:NavigationBarDashButton', 'Account Menu'] }, readHere] },
      // Android: "Show navigation drawer" shows the name and "@handle".
      { name: 'navigation drawer (Android, verified)', verifiedOn: 'android', steps: [{ tap: ['Show navigation drawer'] }, readHere] },
    ],
  },
  youtube: {
    app: 'youtube',
    // You tab → Accounts: the Google account's email, then its channels'
    // "@handle"s. Without Accounts, the You tab's own "@handle".
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ kind: 'email' }, { kind: 'handle' }],
    paths: [
      {
        name: 'you → accounts (iOS, verified)',
        verifiedOn: 'ios',
        steps: [
          ...leave('id:id.ui.browse.close.button'),
          { tap: ['id:id.ui.pivotbar.FElibrary.button', 'You'] },
          { tap: ['Accounts'], optional: true },
          readHere,
          { tap: ['id:id.ui.browse.close.button'], optional: true },
        ],
      },
      // Android: You → Accounts lists the Google account's name and email.
      { name: 'you → accounts (Android, verified)', verifiedOn: 'android', steps: [{ tap: ['You'] }, { tap: ['Accounts'], optional: true }, readHere] },
    ],
  },
  github: {
    app: 'github',
    // Home: the navigation bar's first button is the login ("<login>", then
    // "Add", "Search"). The profile screen's "@handle" is the linked X account.
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ before: 'Add' }],
    // Left on the profile by a cut-short read: its "Back" (id BackButton) returns Home.
    paths: [{ name: 'home navigation bar (iOS, verified)', verifiedOn: 'ios', steps: [...leave('id:BackButton'), readHere] }],
  },
  linkedin: {
    app: 'linkedin',
    // Android: the profile image (id me_launcher; its container ignores an accessibility click) opens your profile; the name follows "Edit profile".
    verified: true,
    plans: [],
    vendorWords: [],
    account: [{ after: 'Edit profile' }],
    // Left on the profile by a cut-short read: its "Back button" returns to the feed.
    paths: [{ name: 'me → profile (Android, verified)', verifiedOn: 'android', steps: [...leave('Back button'), { tap: ['id:me_launcher'] }, readHere] }],
  },
  gmail: googleAccountApp('gmail'),
  google_drive: googleAccountApp('google_drive'),
  googlecalendar: googleAccountApp('googlecalendar'),
  googlemeet: googleAccountApp('googlemeet'),
  googlephotos: googleAccountApp('googlephotos'),
};

export function accountSpec(appId: string): PhoneAccountSpec | null {
  return ACCOUNT_SPECS[appId] ?? null;
}

/** True when the app has a path confirmed on this platform (Settings → Connections shows "Verified reader"). */
export function verifiedReader(appId: string, platform: 'ios' | 'android'): boolean {
  return ACCOUNT_SPECS[appId]?.paths.some((p) => p.verifiedOn === platform) ?? false;
}
