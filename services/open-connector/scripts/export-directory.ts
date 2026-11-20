import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { loadProviderSources } from "./provider-source.ts";

// Slim app directory snapshot for the cloud-api connections catalog.
// Run from services/open-connector. `--check` fails if the committed file is stale.

const repoRoot = resolve(process.cwd(), "../..");
const curatedPath = join(repoRoot, "cmd/allternit-api/assets/open-design/connectors.json");
const outputPath = join(repoRoot, "cmd/allternit-cloud-api/assets/connections-catalog-apps.json");

interface Row {
  id: string;
  name: string;
  category: string;
  blurb: string;
  auth: string;
}

function blurb(text: string | undefined): string {
  const flat = (text ?? "").replace(/\s+/g, " ").trim();
  if (flat.length <= 140) return flat;
  return `${flat.slice(0, 139).trimEnd()}…`;
}

async function build(): Promise<string> {
  const rows = new Map<string, Row>();
  for (const { definition: p } of await loadProviderSources()) {
    rows.set(p.service, {
      id: p.service,
      name: p.displayName,
      category: p.categories[0] ?? "Other",
      blurb: blurb(p.description),
      auth: p.authTypes[0] ?? p.auth[0]?.type ?? "no_auth",
    });
  }
  const curated = JSON.parse(await readFile(curatedPath, "utf8")) as {
    connectors: Array<{ id: string; name: string; category?: string; description?: string; auth?: { provider?: string } }>;
  };
  for (const c of curated.connectors) {
    rows.set(c.id, {
      id: c.id,
      name: c.name,
      category: c.category ?? rows.get(c.id)?.category ?? "Other",
      blurb: blurb(c.description),
      auth: c.auth?.provider ?? rows.get(c.id)?.auth ?? "no_auth",
    });
  }
  const apps = [...rows.values()].sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  const doc = {
    version: 1,
    generated_from: "services/open-connector provider definitions + cmd/allternit-api open-design connectors.json (curated wins on id)",
    apps,
  };
  return `${JSON.stringify(doc, null, 2)}\n`;
}

const next = await build();
if (process.argv.includes("--check")) {
  let current = "";
  try {
    current = await readFile(outputPath, "utf8");
  } catch {}
  if (current !== next) {
    console.error("connections-catalog-apps.json is stale: run `npm run export:directory` in services/open-connector.");
    process.exit(1);
  }
  console.log("connections-catalog-apps.json is up to date.");
} else {
  await mkdir(dirname(outputPath), { recursive: true });
  await writeFile(outputPath, next);
  console.log(`Wrote ${JSON.parse(next).apps.length} apps to ${outputPath}`);
}
