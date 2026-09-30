// Regenerate the static .html fixtures from markup.ts:  npx tsx fixtures/generate.mjs
import { writeFileSync } from "node:fs";
import { SCENARIOS, renderPage } from "./markup.ts";
for (const [name, s] of Object.entries(SCENARIOS)) writeFileSync(new URL(`./${name}.html`, import.meta.url), renderPage(s) + "\n");
console.log(`wrote ${Object.keys(SCENARIOS).length} fixtures`);
