import { hasErrors, listen, validateManifest, type DirectoryFinding } from "@allternit/apps-server";
import { parseArgs, str } from "../args.js";
import { CliError, type Context } from "../context.js";
import { liveScan } from "../live.js";
import { loadApp } from "../load.js";

const order = { error: 0, warning: 1, info: 2 } as const;

export function printFindings(ctx: Context, findings: DirectoryFinding[]): void {
  for (const f of [...findings].sort((a, b) => order[a.severity] - order[b.severity])) {
    ctx.log(`  ${f.severity.padEnd(7)} ${f.code}${f.subject ? ` (${f.subject})` : ""}: ${f.message}`);
  }
}

/**
 * `allternit test [entry] [--url <mcp url>] [--strict]`: the directory scan rules + annotation checks.
 * Without --url it lints the declared app, then starts it on a free port and scans what an MCP client sees.
 * Returns the exit code: 1 on errors (or warnings with --strict).
 */
export async function test(ctx: Context, argv: string[]): Promise<number> {
  const { positional, flags } = parseArgs(argv, ["strict"]);
  const url = str(flags.url);
  const findings: DirectoryFinding[] = [];

  if (url) {
    ctx.log(`Scanning ${url}`);
    try {
      findings.push(...(await liveScan(url)).findings);
    } catch (err) {
      throw new CliError(`Could not connect to ${url}: ${err instanceof Error ? err.message : String(err)}`);
    }
  } else {
    const { app } = await loadApp(ctx.cwd, positional[0]);
    ctx.log(`Checking ${app.input.name}`);
    findings.push(...validateManifest(app.manifest()), ...app.lint());
    const running = await listen(app, { port: 0 });
    try {
      findings.push(...(await liveScan(running.url)).findings);
    } finally {
      await running.close();
    }
  }

  // Declared and live checks overlap; report each finding once.
  const seen = new Set<string>();
  const unique = findings.filter((f) => {
    const key = `${f.code}|${f.subject ?? ""}|${f.message}`;
    return seen.has(key) ? false : (seen.add(key), true);
  });
  if (unique.length === 0) ctx.log("  no findings");
  else printFindings(ctx, unique);
  const warnings = unique.filter((f) => f.severity === "warning").length;
  const failed = hasErrors(unique) || (flags.strict === true && warnings > 0);
  ctx.log(failed ? "\nFAILED" : "\nPASSED");
  return failed ? 1 : 0;
}
