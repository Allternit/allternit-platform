#!/usr/bin/env node
/**
 * Builds the hidden call bridge window (src/renderer/call-bridge) into
 * dist/renderer/call-bridge: bridge.js (livekit-client bundled, browser IIFE)
 * and index.html. Loaded by src/main/phone-call-bridge-window.ts.
 */

const fs = require('node:fs');
const path = require('node:path');
const esbuild = require('esbuild');

const desktopRoot = path.resolve(__dirname, '..');
const srcDir = path.join(desktopRoot, 'src', 'renderer', 'call-bridge');
const outDir = path.join(desktopRoot, 'dist', 'renderer', 'call-bridge');

async function main() {
  fs.mkdirSync(outDir, { recursive: true });
  await esbuild.build({
    entryPoints: [path.join(srcDir, 'bridge.ts')],
    outfile: path.join(outDir, 'bridge.js'),
    bundle: true,
    format: 'iife',
    platform: 'browser',
    target: ['chrome120'],
    minify: true,
    sourcemap: false,
    legalComments: 'none',
    logLevel: 'warning',
  });
  fs.copyFileSync(path.join(srcDir, 'index.html'), path.join(outDir, 'index.html'));
  console.log('[build-call-bridge] wrote', path.relative(desktopRoot, outDir));
}

main().catch((error) => {
  console.error('[build-call-bridge] failed:', error);
  process.exit(1);
});
