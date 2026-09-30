import { listen } from "@allternit/apps-server";
import { parseArgs, str } from "../args.js";
import type { Context } from "../context.js";
import { loadApp } from "../load.js";

export interface DevHandle {
  url: string;
  close(): Promise<void>;
}

/** `allternit dev [entry] [--port 3000]`: run the server and print how to add it as a connector. */
export async function dev(ctx: Context, argv: string[]): Promise<DevHandle> {
  const { positional, flags } = parseArgs(argv);
  const { app, file } = await loadApp(ctx.cwd, positional[0]);
  const port = Number(str(flags.port) ?? ctx.env.PORT ?? 3000);
  const running = await listen(app, { port: Number.isFinite(port) ? port : 3000 });
  const manifest = app.manifest();
  ctx.log(`${manifest.name} ${manifest.version} (${file})`);
  ctx.log(`  MCP server   ${running.url}`);
  ctx.log(`  Manifest     ${new URL(running.url).origin}/allternit.app.json`);
  ctx.log(`  Tools        ${manifest.tools.map((t) => t.name).join(", ") || "(none)"}`);
  ctx.log(`  Views        ${manifest.views.map((v) => v.uri).join(", ") || "(none)"}\n`);
  ctx.log("Add it as a connector:");
  ctx.log("  Allternit reaches your server from its own infrastructure, so the URL must be public https.");
  ctx.log("  Expose this port through a tunnel, then create the connector:\n");
  ctx.log("  curl -X POST https://api.allternit.com/mcp/connectors \\");
  ctx.log('    -H "Content-Type: application/json" -H "Authorization: Bearer <token>" \\');
  ctx.log(`    -d '{"name":"${manifest.name}","name_id":"${manifest.server.name}","url":"https://<your-tunnel-host>${manifest.server.path}","type":"http"}'\n`);
  ctx.log("Press Ctrl+C to stop.");
  return running;
}
