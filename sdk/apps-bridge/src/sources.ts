/** Node-only: the dependency-free runtime scripts as strings, for inlining into View HTML. */
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

function read(name: string): string {
  const here = dirname(fileURLToPath(import.meta.url));
  for (const dir of [here, join(here, "..", "runtime")]) {
    const file = join(dir, name);
    if (existsSync(file)) return readFileSync(file, "utf8");
  }
  throw new Error(`apps-bridge: runtime file ${name} not found`);
}

/** `window.allternit` (x-allternit host extensions). */
export const allternitExtSource = (): string => read("allternit-ext.js");
/** `window.openai` compatibility shim. */
export const openaiCompatSource = (): string => read("openai-compat.js");
