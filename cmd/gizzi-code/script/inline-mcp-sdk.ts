#!/usr/bin/env bun
/**
 * Formerly: copied the vendored v1 `@modelcontextprotocol/sdk` sources into
 * src/cli/ui/ink-app/mcp-sdk/ and rewrote imports to relative paths.
 *
 * gizzi-code now uses the SDK v2 packages (`@modelcontextprotocol/client|server|core`),
 * which the production bundler inlines from node_modules directly, so no source
 * rewriting is needed or wanted. This entry point is kept so old instructions still do
 * something useful: it runs the v2 packaging check (`script/vendor-mcp-sdk.ts`).
 */

const proc = Bun.spawn([process.execPath, `${import.meta.dir}/vendor-mcp-sdk.ts`, ...process.argv.slice(2)], {
  stdio: ["inherit", "inherit", "inherit"],
})
process.exit(await proc.exited)
