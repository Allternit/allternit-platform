import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import JSZip from "jszip";
import { hasErrors, MANIFEST_FILE, toPluginFiles, validateManifest, type AppManifest } from "@allternit/apps-server";
import { parseArgs, str } from "../args.js";
import { CliError, type Context } from "../context.js";
import { loadApp } from "../load.js";
import { printFindings } from "./test.js";

/** A minimal SKILL.md (frontmatter + description) so the package passes the parser's skill check. */
export function skillFor(manifest: AppManifest): string {
  const lines = [
    "---",
    `name: ${manifest.server.name}`,
    `description: ${JSON.stringify(manifest.description ?? `Use the ${manifest.name} tools.`)}`,
    "---",
    "",
    `# ${manifest.name}`,
    "",
    "Tools:",
    ...manifest.tools.map((t) => `- \`${t.name}\`: ${t.description}`),
    "",
  ];
  return lines.join("\n");
}

/** Build the package zip: plugin.json, mcp.json, allternit.app.json and one skill. */
export async function buildPackage(manifest: AppManifest, url: string): Promise<Uint8Array> {
  const findings = validateManifest(manifest);
  if (hasErrors(findings)) throw new CliError(`Manifest has errors: ${findings.filter((f) => f.severity === "error").map((f) => f.message).join("; ")}`);
  const files = toPluginFiles(manifest, url);
  const zip = new JSZip();
  zip.file("plugin.json", JSON.stringify(files["plugin.json"], null, 2));
  zip.file("mcp.json", JSON.stringify(files["mcp.json"], null, 2));
  zip.file(MANIFEST_FILE, JSON.stringify({ ...manifest, server: { ...manifest.server, url } }, null, 2));
  zip.file(`skills/${manifest.server.name}/SKILL.md`, skillFor(manifest));
  return zip.generateAsync({ type: "uint8array", compression: "DEFLATE" });
}

/** `allternit package [entry] --url <https mcp url> [--out app.zip]`; returns the written path. */
export async function packageCmd(ctx: Context, argv: string[]): Promise<string> {
  const { positional, flags } = parseArgs(argv);
  const { app } = await loadApp(ctx.cwd, positional[0]);
  const url = str(flags.url) ?? app.manifest().server.url;
  if (!url) throw new CliError("--url <public https MCP url> is required: the package points Allternit at your hosted server.");
  const manifest = app.manifest({ url });
  const lint = app.lint();
  if (hasErrors(lint)) {
    printFindings(ctx, lint);
    throw new CliError("Fix the errors above before packaging (run `allternit test`).");
  }
  const out = resolve(ctx.cwd, str(flags.out) ?? `${manifest.server.name}.zip`);
  let bytes: Uint8Array;
  try {
    bytes = await buildPackage(manifest, url);
  } catch (err) {
    throw err instanceof CliError ? err : new CliError(err instanceof Error ? err.message : String(err));
  }
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, bytes);
  ctx.log(`Wrote ${out} (${bytes.length} bytes)`);
  ctx.log("Next: host the server at that url, then `allternit submit`.");
  return out;
}
