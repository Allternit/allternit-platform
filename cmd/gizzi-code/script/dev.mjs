#!/usr/bin/env bun
/**
 * `bun run dev` / `bun run start`: run the TUI from source with the same
 * compile-time feature flags the production build uses (script/features.mjs).
 * Plain `bun ./src/cli/main.ts` evaluates every feature() to false.
 */
import { fileURLToPath } from "url";
import { resolveFeatures } from "./features.mjs";

const MAIN = fileURLToPath(new URL("../src/cli/main.ts", import.meta.url));

const flags = resolveFeatures().map((f) => `--feature=${f}`);
const proc = Bun.spawn(
  [process.execPath, ...flags, "--conditions=browser", MAIN, ...process.argv.slice(2)],
  { stdio: ["inherit", "inherit", "inherit"] },
);
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(sig, () => proc.kill(sig));
process.exit(await proc.exited);
