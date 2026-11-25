#!/usr/bin/env node
/**
 * Bundles @allternit/share-tunnel (platform/packages/share-tunnel, TypeScript source) with its deps
 * (@opentunnel/client, effect) into dist/vendor/share-tunnel.mjs. Desktop main is plain `tsc` with
 * rootDir src/main, so it cannot compile a workspace package's source; src/main/share-manager.ts loads
 * this bundle with a dynamic import. The package's deps come from the root `pnpm install` (the package
 * is a workspace member). Electron 41 ships Node 24, above the package's Node 22 floor.
 */

const fs = require('node:fs');
const path = require('node:path');
const esbuild = require('esbuild');

const desktopRoot = path.resolve(__dirname, '..');
const entry = path.resolve(desktopRoot, '../../platform/packages/share-tunnel/src/index.ts');
const outfile = path.join(desktopRoot, 'dist', 'vendor', 'share-tunnel.mjs');

async function main() {
  if (!fs.existsSync(entry)) throw new Error(`missing ${entry}`);
  fs.mkdirSync(path.dirname(outfile), { recursive: true });
  await esbuild.build({
    entryPoints: [entry],
    outfile,
    bundle: true,
    format: 'esm',
    platform: 'node',
    target: ['node22'],
    // CommonJS deps inside an ESM bundle still call require().
    banner: { js: "import { createRequire as __shareTunnelRequire } from 'node:module'; const require = __shareTunnelRequire(import.meta.url);" },
    sourcemap: false,
    legalComments: 'none',
    logLevel: 'warning',
  });
  console.log('[build-share-tunnel] wrote', path.relative(desktopRoot, outfile));
}

main().catch((error) => {
  console.error('[build-share-tunnel] failed:', error);
  process.exit(1);
});
