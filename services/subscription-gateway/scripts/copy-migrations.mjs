// Copies SQL migrations next to the compiled store so `node dist/main.js`
// finds them at the same relative location as under src/.
import { cpSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const from = join(here, "..", "src", "store", "migrations");
const to = join(here, "..", "dist", "store", "migrations");
mkdirSync(to, { recursive: true });
cpSync(from, to, { recursive: true });
