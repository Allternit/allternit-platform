// Imports the Claude connector pack manifest into the cloud-api catalog snapshot.
//
// Rule (Eoj, E1): list every pack entry that has a public https server URL,
// interactive ones included. Entries without one are dropped, with the reason
// printed per group. Existing snapshot entries and their fields are kept; a pack
// entry that matches one (id, or name) adds its fields to it, the rest are
// appended as new apps. Icon provenance (E2) goes to data/catalog-sources.json;
// the icon files themselves are a later step.
//
// Run from services/open-connector:
//   node scripts/import-claude-pack.ts --pack <manifest.json> [--ai-repo <allternit-ai checkout>]
// The screen pack ids are read from `src/lib/app-screens/{packs,auto}` on the
// ai repo's origin/main (git ls-tree). Without --ai-repo no screen_pack is set.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

type PackEntry = {
  title: string;
  slug: string;
  oneLiner: string;
  iconUrl: string;
  serverUrl: string;
  author: string;
  authorUrl: string;
  type: string;
  categories: string[];
  interactive: boolean;
  isNew: boolean;
  trending: boolean;
  tier: string;
  popularity: number;
  iconSource: string;
  iconFetch: string;
  file: string;
  lowres: boolean;
};
type App = Record<string, unknown> & { id: string; name: string };

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const snapshotPath = resolve(root, "../../cmd/allternit-cloud-api/src/services/connections_catalog_apps.json");
const sourcesPath = join(root, "data/catalog-sources.json");

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : undefined;
}
const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "");

const packPath = arg("pack");
if (!packPath) throw new Error("usage: import-claude-pack.ts --pack <manifest.json> [--ai-repo <path>]");
const manifest = JSON.parse(readFileSync(packPath, "utf8")) as PackEntry[];

// Screen pack ids that exist in the ai repo.
const screenPacks = new Set<string>();
const aiRepo = arg("ai-repo");
if (aiRepo) {
  const out = execFileSync("git", ["-C", aiRepo, "ls-tree", "-r", "--name-only", "origin/main", "src/lib/app-screens"], {
    encoding: "utf8",
    maxBuffer: 1 << 24,
  });
  for (const line of out.split("\n")) {
    const m = /^src\/lib\/app-screens\/(?:packs|auto)\/([^/]+)\.json$/.exec(line);
    if (m) screenPacks.add(m[1]);
  }
}

function publicHttps(url: string): boolean {
  try {
    const u = new URL(url);
    return u.protocol === "https:" && !/^(localhost|127\.|10\.|192\.168\.|\[)/.test(u.hostname) && u.hostname.includes(".");
  } catch {
    return false;
  }
}

// A vendor tracker token in a pack URL (accuweather) trips secret scanning and is
// not needed to connect: remove the `tracker` query parameter.
function cleanUrl(url: string): string {
  try {
    const u = new URL(url);
    if (!u.searchParams.has("tracker")) return url;
    u.searchParams.delete("tracker");
    return u.toString().replace(/\?$/, "");
  } catch {
    return url;
  }
}

// Drop groups, each with a reason.
const dropped = new Map<string, string[]>();
const drop = (reason: string, slug: string) => dropped.set(reason, [...(dropped.get(reason) ?? []), slug]);
const listed: PackEntry[] = [];
const seenSlugs = new Set<string>();
for (const e of manifest) {
  if (seenSlugs.has(e.slug)) {
    drop("duplicate slug", e.slug);
    continue;
  }
  seenSlugs.add(e.slug);
  e.serverUrl = e.serverUrl && cleanUrl(e.serverUrl);
  if (!e.serverUrl) drop(e.type === "local" ? "local (stdio) connector: no server URL" : "remote connector with no server URL", e.slug);
  else if (!publicHttps(e.serverUrl)) drop("server URL is not a public https URL", e.slug);
  else if (!e.iconSource || !(e.iconUrl || e.iconFetch)) drop("no icon source", e.slug);
  else listed.push(e);
}

const snapshot = JSON.parse(readFileSync(snapshotPath, "utf8")) as { version: number; generated_from: string; apps: App[] };
const byId = new Map(snapshot.apps.map((a) => [a.id, a]));
const byName = new Map(snapshot.apps.map((a) => [norm(a.name), a]));

let merged = 0;
let added = 0;
let withScreens = 0;
const sources: Record<string, unknown> = {};
for (const e of listed) {
  const key = norm(e.slug);
  const fields = {
    slug: e.slug,
    title: e.title,
    one_liner: e.oneLiner,
    author: e.author,
    author_url: e.authorUrl,
    server_url: e.serverUrl,
    categories: e.categories,
    interactive: e.interactive,
    is_new: e.isNew,
    trending: e.trending,
    tier: e.tier,
    popularity: e.popularity,
    icon_source: e.iconSource,
    ...(screenPacks.has(key) ? { screen_pack: key } : {}),
  };
  let hit = byId.get(key) ?? byName.get(norm(e.title));
  // An app already filled by another pack entry is not overwritten.
  if (hit && "slug" in hit) hit = undefined;
  if (fields.screen_pack) withScreens++;
  if (hit) {
    Object.assign(hit, fields);
    merged++;
  } else {
    // New app: reached through its MCP server URL; "mcp" is not an api_key type,
    // so the catalog treats it as an oauth-style connect.
    snapshot.apps.push({
      id: byId.has(key) ? `${key}_${merged + added}` : key,
      name: e.title,
      category: e.categories[0] ?? "",
      blurb: e.oneLiner,
      auth: "mcp",
      ...fields,
    });
    added++;
  }
  sources[e.slug] = {
    iconUrl: e.iconUrl || e.iconFetch,
    iconSource: e.iconSource,
    iconFetch: e.iconFetch,
    file: e.file,
    lowres: e.lowres,
    listedFrom: "claude-connector-pack",
  };
}

writeFileSync(snapshotPath, `${JSON.stringify(snapshot, null, 2)}\n`);
writeFileSync(
  sourcesPath,
  `${JSON.stringify({ version: 1, source: "claude-connector-pack manifest", entries: sources }, null, 2)}\n`,
);

console.log(`pack entries: ${manifest.length}`);
console.log(`listed: ${listed.length} (merged into existing apps: ${merged}, added: ${added}, with screen pack: ${withScreens})`);
const droppedTotal = [...dropped.values()].reduce((n, l) => n + l.length, 0);
console.log(`dropped: ${droppedTotal}`);
for (const [reason, slugs] of dropped) console.log(`  ${slugs.length}  ${reason}`);
console.log(`snapshot apps: ${snapshot.apps.length}`);
