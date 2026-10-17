import { describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  ANDROID_APP_PACKAGES,
  IOS_APP_BUNDLES,
  IOS_PROBE_BUNDLES,
  PHONE_APP_CATALOG,
  catalogApp,
  catalogAppByName,
  catalogPhoneApps,
  nameCatalogApps,
} from './phone-app-catalog.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../../../..');
const connectors = (
  JSON.parse(fs.readFileSync(path.join(repo, 'cmd/allternit-api/assets/open-design/connectors.json'), 'utf8')) as {
    connectors: { id: string; category: string }[];
  }
).connectors;
const knownProviders = (() => {
  const src = fs.readFileSync(path.join(repo, 'services/subscription-gateway/src/http/routes_accounts.ts'), 'utf8');
  const block = /const KNOWN_PROVIDERS[^{]*\{([^}]*)\}/.exec(src)?.[1] ?? '';
  return new Set([...block.matchAll(/["']?([\w-]+)["']?\s*:/g)].map((m) => m[1]));
})();

describe('phone app catalog', () => {
  it('has unique ids, names and store ids, and every app is on at least one store', () => {
    const ids = PHONE_APP_CATALOG.map((a) => a.id);
    expect(new Set(ids).size).toBe(ids.length);
    const names = PHONE_APP_CATALOG.map((a) => a.name.toLowerCase());
    expect(new Set(names).size).toBe(names.length);
    for (const platform of ['ios', 'android'] as const) {
      const all = PHONE_APP_CATALOG.flatMap((a) => a[platform]);
      expect(new Set(all).size, platform).toBe(all.length);
    }
    for (const app of PHONE_APP_CATALOG) expect(app.ios.length + app.android.length, app.id).toBeGreaterThan(0);
  });

  it('lines up with the connector catalog: real connector ids, same category', () => {
    const byId = new Map(connectors.map((c) => [c.id, c]));
    const linked = PHONE_APP_CATALOG.filter((a) => a.connector);
    expect(linked.length).toBeGreaterThan(80);
    for (const app of linked) {
      const connector = byId.get(app.connector!);
      expect(connector, app.id).toBeDefined();
      expect(app.category, app.id).toBe(connector!.category);
    }
  });

  it('names gateway vendors by the gateway’s own provider ids', () => {
    expect(knownProviders.has('chatgpt')).toBe(true);
    for (const app of PHONE_APP_CATALOG.filter((a) => a.provider)) expect(knownProviders.has(app.provider!), app.id).toBe(true);
    expect(catalogApp('gemini')?.provider).toBe('google');
    expect(catalogApp('copilot')?.provider).toBe('microsoft');
  });

  it('only ever probes AI chat apps by launching them', () => {
    const probed = Object.keys(IOS_PROBE_BUNDLES);
    expect(probed.length).toBeLessThanOrEqual(10);
    for (const id of probed) expect(catalogApp(id)?.category).toBe('AI chat');
    expect(probed).not.toContain('instagram');
  });

  it('builds the per-platform maps from the data', () => {
    expect(IOS_APP_BUNDLES.chatgpt).toEqual(['com.openai.chat']);
    expect(ANDROID_APP_PACKAGES.telegram).toEqual(['org.telegram.messenger']);
    expect(catalogAppByName('Google Drive')?.id).toBe('google_drive');
  });

  it('reports installed apps with names, and names lists saved by older builds', () => {
    expect(catalogPhoneApps(['notion.id', 'com.unknown'], 'android')).toEqual([
      { vendor: 'notion', signedIn: null, name: 'Notion', category: 'Productivity' },
    ]);
    expect(nameCatalogApps([{ vendor: 'gemini', signedIn: true }, { vendor: 'mystery', signedIn: null }])).toEqual([
      { vendor: 'gemini', signedIn: true, name: 'Gemini', category: 'AI chat', provider: 'google' },
      { vendor: 'mystery', signedIn: null },
    ]);
  });
});
