/** Node-only: the view kit runtime as a string, for inlining into View HTML. */
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export function kitSource(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  for (const dir of [here, join(here, "..", "runtime")]) {
    const file = join(dir, "allternit-ui.js");
    if (existsSync(file)) return readFileSync(file, "utf8");
  }
  throw new Error("apps-ui: runtime/allternit-ui.js not found");
}
