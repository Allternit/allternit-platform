#!/usr/bin/env node
// Bundle the app's Motion renderer for the cloud-api host.
//
//   node scripts/build-motion-render.mjs [--ai <allternit-ai checkout>] [--check]
//
// Reads src/lib/artifacts/motion/{render,schema}.ts from an allternit-ai
// checkout (default: ../allternit-ai next to this repo, or $ALLTERNIT_AI_DIR),
// bundles them with cmd/allternit-cloud-api/render/motion/src/runner.mjs into
// render/motion/dist/render.mjs (@napi-rs/canvas stays external) and records
// the source commit and file hashes in dist/SOURCE.json.
//
// The bundle is checked in and shipped by deploy-contabo.sh; regenerate it
// whenever allternit-ai changes those two files. `--check` rebuilds in memory
// and exits 1 when the checked-in bundle is stale (renderer changed in the
// app, or the runner changed here) without writing anything.
//
// One-time setup: `npm ci` in cmd/allternit-cloud-api/render/motion (installs
// esbuild, a dev dependency that is not deployed).

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const pkg = path.join(root, 'cmd/allternit-cloud-api/render/motion');
const dist = path.join(pkg, 'dist');
const SOURCES = ['render.ts', 'schema.ts'];

const argv = process.argv.slice(2);
const check = argv.includes('--check');
const aiFlag = argv.indexOf('--ai');
const ai = path.resolve(aiFlag >= 0 ? argv[aiFlag + 1] : process.env.ALLTERNIT_AI_DIR || path.join(root, '..', 'allternit-ai'));
const motionDir = path.join(ai, 'src/lib/artifacts/motion');

for (const f of SOURCES) {
  if (!existsSync(path.join(motionDir, f))) {
    console.error(`Can't find ${path.join(motionDir, f)}. Pass --ai <allternit-ai checkout> or set ALLTERNIT_AI_DIR.`);
    process.exit(2);
  }
}

const sha = (buf) => createHash('sha256').update(buf).digest('hex');
const sources = Object.fromEntries(SOURCES.map((f) => [f, sha(readFileSync(path.join(motionDir, f)))]));
let commit = 'unknown';
try {
  commit = execFileSync('git', ['-C', ai, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
} catch {
  // Not a git checkout: the file hashes still pin the renderer.
}

const { build } = createRequire(path.join(pkg, 'package.json'))('esbuild');
const result = await build({
  entryPoints: [path.join(pkg, 'src/runner.mjs')],
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node22',
  external: ['@napi-rs/canvas'],
  alias: { 'allternit-ai': path.join(ai, 'src') },
  legalComments: 'none',
  write: false,
  logLevel: 'warning',
});
const bundle = result.outputFiles[0].text;
const meta = JSON.stringify({ allternit_ai_commit: commit, sources, runner: sha(readFileSync(path.join(pkg, 'src/runner.mjs'))) }, null, 2) + '\n';

const bundlePath = path.join(dist, 'render.mjs');
const metaPath = path.join(dist, 'SOURCE.json');
if (check) {
  const old = existsSync(bundlePath) ? readFileSync(bundlePath, 'utf8') : '';
  const oldMeta = existsSync(metaPath) ? JSON.parse(readFileSync(metaPath, 'utf8')) : { sources: {} };
  const stale = old !== bundle;
  const changed = SOURCES.filter((f) => oldMeta.sources?.[f] !== sources[f]);
  if (stale || changed.length) {
    console.error(`Motion render bundle is stale (${changed.join(', ') || 'runner'} changed). Run: node scripts/build-motion-render.mjs --ai ${ai}`);
    process.exit(1);
  }
  console.log('Motion render bundle is current.');
} else {
  mkdirSync(dist, { recursive: true });
  writeFileSync(bundlePath, bundle);
  writeFileSync(metaPath, meta);
  console.log(`Wrote ${path.relative(root, bundlePath)} (${(bundle.length / 1024).toFixed(0)} KB) from allternit-ai ${commit.slice(0, 10)}`);
}
