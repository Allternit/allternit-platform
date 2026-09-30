import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import type { AllternitApp } from "@allternit/apps-server";
import { CliError } from "./context.js";

const DEFAULT_ENTRIES = ["server.ts", "src/server.ts", "server.js", "src/server.js"];

export function resolveEntry(cwd: string, entry?: string): string {
  const candidates = entry ? [entry] : DEFAULT_ENTRIES;
  for (const c of candidates) {
    const p = resolve(cwd, c);
    if (existsSync(p)) return p;
  }
  throw new CliError(entry ? `Entry file not found: ${entry}` : `No server entry found (looked for ${DEFAULT_ENTRIES.join(", ")}). Pass one explicitly.`);
}

const isApp = (v: unknown): v is AllternitApp =>
  typeof v === "object" && v !== null && typeof (v as AllternitApp).createServer === "function" && typeof (v as AllternitApp).manifest === "function";

/** Import the entry (TypeScript included, via tsx) and return its default export: a `defineApp()` result. */
export async function loadApp(cwd: string, entry?: string): Promise<{ app: AllternitApp; file: string }> {
  const file = resolveEntry(cwd, entry);
  let mod: Record<string, unknown>;
  try {
    const { tsImport } = await import("tsx/esm/api");
    mod = (await tsImport(pathToFileURL(file).href, import.meta.url)) as Record<string, unknown>;
  } catch (err) {
    throw new CliError(`Could not load ${file}: ${err instanceof Error ? err.message : String(err)}`);
  }
  // tsx may wrap CJS-style default exports one level deep.
  const candidate = (mod.default as { default?: unknown } | undefined)?.default ?? mod.default ?? mod.app;
  if (!isApp(candidate)) throw new CliError(`${file} must export default the result of defineApp().`);
  return { app: candidate, file };
}
