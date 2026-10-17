/**
 * The phone app catalog: which apps Phone Link looks for on a linked phone,
 * as data (`phone-app-catalog.json`), not as lists in code or the UI.
 *
 * It carries the AI vendors the subscription gateway supports (`provider`)
 * plus the most useful apps that also have an MCP connector (`connector`,
 * the id in cmd/allternit-api/assets/open-design/connectors.json, with that
 * connector's category), and messaging channels without one (`channel`).
 * Every iPhone bundle id and Android package was checked against the App
 * Store lookup API and Google Play when it was added
 * (scripts/verify-phone-app-catalog.mjs re-checks them).
 *
 * `phone.devices` and the Phone Link status report installed catalog apps
 * with their name and category, so Settings → Subscriptions shows one row
 * per app without a list of its own.
 */
import catalog from './phone-app-catalog.json' with { type: 'json' };
import type { PhoneLinkApp, PhoneLinkPlatform } from './phone-link-types.js';

export interface PhoneCatalogApp {
  id: string;
  name: string;
  category: string;
  /** Subscription gateway provider id (KNOWN_PROVIDERS: Gemini is 'google', Copilot 'microsoft'). */
  provider?: string;
  /** MCP connector id in the connector catalog. */
  connector?: string;
  /** Messaging channel id (src/lib/gateway/channel.ts in allternit-ai) when there is no connector. */
  channel?: string;
  ios: string[];
  android: string[];
}

export const PHONE_APP_CATALOG: readonly PhoneCatalogApp[] = (catalog as { apps: PhoneCatalogApp[] }).apps;

const BY_ID = new Map(PHONE_APP_CATALOG.map((app) => [app.id, app]));

export function catalogApp(id: string): PhoneCatalogApp | undefined {
  return BY_ID.get(id);
}

const BY_NAME = new Map(PHONE_APP_CATALOG.map((app) => [app.name.toLowerCase(), app]));

/** "Google Drive" → the google_drive entry (case-insensitive). */
export function catalogAppByName(name: string): PhoneCatalogApp | undefined {
  return BY_NAME.get(name.trim().toLowerCase());
}

function idsFor(platform: PhoneLinkPlatform): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const app of PHONE_APP_CATALOG) {
    const ids = platform === 'ios' ? app.ios : app.android;
    if (ids.length > 0) out[app.id] = ids;
  }
  return out;
}

/** Catalog id → iPhone bundle ids. */
export const IOS_APP_BUNDLES: Readonly<Record<string, string[]>> = idsFor('ios');
/** Catalog id → Android packages. */
export const ANDROID_APP_PACKAGES: Readonly<Record<string, string[]>> = idsFor('android');

/**
 * Apps Desktop may find by launching them when the phone can't list its
 * apps (iPhone with no cable and no earlier list): only the AI chat apps.
 * Opening a hundred apps on someone's phone to see which exist is never
 * acceptable.
 */
export const PROBE_CATEGORY = 'AI chat';
export const IOS_PROBE_BUNDLES: Readonly<Record<string, string[]>> = Object.fromEntries(
  PHONE_APP_CATALOG.filter((app) => app.category === PROBE_CATEGORY && app.ios.length > 0).map((app) => [app.id, app.ios]),
);

/** The catalog apps among installed bundle ids / packages, named, in catalog order. */
export function catalogPhoneApps(installed: Iterable<string>, platform: PhoneLinkPlatform): PhoneLinkApp[] {
  const set = new Set(installed);
  return PHONE_APP_CATALOG.filter((app) => (platform === 'ios' ? app.ios : app.android).some((id) => set.has(id))).map((app) => ({
    vendor: app.id,
    signedIn: null,
    name: app.name,
    category: app.category,
    ...(app.provider ? { provider: app.provider } : {}),
  }));
}

/** Fill name and category on an app list saved by an older build (vendor id only). */
export function nameCatalogApps(apps: PhoneLinkApp[]): PhoneLinkApp[] {
  return apps.map((app) => {
    const entry = BY_ID.get(app.vendor);
    return entry && !app.name
      ? { ...app, name: entry.name, category: entry.category, ...(entry.provider ? { provider: entry.provider } : {}) }
      : app;
  });
}
