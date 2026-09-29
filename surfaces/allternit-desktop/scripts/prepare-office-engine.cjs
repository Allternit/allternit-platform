#!/usr/bin/env node
/**
 * prepare:office-engine — stage the office-engine sidecar for packaging.
 *
 * Builds services/office-engine (tsc → dist) and bundles it with esbuild
 * into a single self-contained ESM file at resources/office-engine/dist/
 * index.js. A bundle is required (not just a file copy) because the
 * @allternit/office-* engine packages compile to extensionless ESM imports
 * that only bundlers resolve — raw Node cannot run them directly.
 *
 * electron-builder copies resources/office-engine into the app bundle via
 * the `extraResources` entry in package.json, and
 * src/main/office-engine-manager.ts runs `dist/index.js` there with
 * ELECTRON_RUN_AS_NODE=1 at runtime.
 */
const { execSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const { cargoBinaryCandidates } = require('./cargo-target.cjs');
const serviceDir = path.join(repoRoot, 'services', 'office-engine');
const outDir = path.join(desktopDir, 'resources', 'office-engine');

// @firecrawl/anydoc (docx/pptx/xlsx/pdf -> Markdown, used by src/markdown.ts)
// ships its napi binding as a native .node file behind a runtime
// process.platform/arch dispatch in its own index.js, with one
// optionalDependency per platform triple. esbuild can't bundle .node files
// (no loader for them) and doesn't know at build time which of those
// require() branches is real, so it must be told to leave the whole package
// external rather than try to inline it. src/markdown.ts already handles a
// missing/failed binding gracefully (anydocLoadError -> 503
// "anydoc_unavailable"), so leaving it external and copying only the
// current-platform binding preserves that same graceful-degradation
// contract instead of only working around the build error.
function currentNapiPlatformTriple() {
  const { platform, arch } = process;
  if (platform === 'darwin') return arch === 'arm64' ? 'darwin-arm64' : 'darwin-x64';
  if (platform === 'win32') return 'win32-x64-msvc';
  if (platform === 'linux') {
    // napi-rs distinguishes glibc/musl on Linux; glibc is the overwhelming
    // majority case and the only one this repo's Linux tooling targets today.
    return arch === 'arm64' ? 'linux-arm64-gnu' : 'linux-x64-gnu';
  }
  return null;
}

// linkedom (HTML parsing for the URL->Markdown path) has an optional peer
// dependency on `canvas`, a native Cairo binding, for <canvas>-element DOM
// support that office-engine's actual usage never touches. esbuild still
// sees the require() and fails the same way it does for anydoc's .node
// file. Unlike anydoc's portable per-platform napi prebuilds, this canvas
// build is compiled locally for this exact machine (node-gyp, not a
// downloaded prebuild) — staging it only makes the bundle runnable on the
// machine that built it, same as the other resources/bin/* binaries in this
// pipeline.
//
// Stage a scoped or unscoped node_modules package (dereferencing pnpm's
// symlinks into real files, since electron-builder's extraResources copy
// would not otherwise resolve symlinks pointing outside resources/) into
// outDir/node_modules so an esbuild --external import resolves at runtime.
function stagePackage(packageName) {
  const srcDir = path.join(repoRoot, 'node_modules', ...packageName.split('/'));
  if (!fs.existsSync(srcDir)) return false;
  const destDir = path.join(outDir, 'node_modules', ...packageName.split('/'));
  fs.mkdirSync(path.dirname(destDir), { recursive: true });
  execSync(`cp -RL "${srcDir}" "${destDir}"`);
  return true;
}

console.log('[prepare:office-engine] building services/office-engine…');
execSync('pnpm build', { cwd: serviceDir, stdio: 'inherit' });

console.log(`[prepare:office-engine] bundling → ${path.join(outDir, 'dist', 'index.js')}`);
fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(path.join(outDir, 'dist'), { recursive: true });
execSync(
  'pnpm exec esbuild dist/index.js --bundle --platform=node --format=esm --target=node20 ' +
    '--external:@firecrawl/anydoc --external:@firecrawl/anydoc-* --external:canvas ' +
    `--outfile="${path.join(outDir, 'dist', 'index.js')}" ` +
    '--banner:js="import{createRequire as __oeCreateRequire}from\'node:module\';const require=__oeCreateRequire(import.meta.url);"',
  { cwd: serviceDir, stdio: 'inherit' },
);

// The bundle is ESM; the staged directory needs this for Node to load it.
fs.writeFileSync(
  path.join(outDir, 'package.json'),
  JSON.stringify({ name: '@allternit/office-engine-bundled', private: true, type: 'module' }, null, 2),
);

// Stage @firecrawl/anydoc + the current platform's native binding next to
// the bundle so the externalized import('@firecrawl/anydoc') resolves at
// runtime. src/markdown.ts already handles a missing/failed binding
// gracefully (anydocLoadError -> 503 "anydoc_unavailable"), so a missing
// platform binding degrades the same way it always could, rather than
// crashing the bundle.
if (stagePackage('@firecrawl/anydoc')) {
  console.log('[prepare:office-engine] staged @firecrawl/anydoc');
} else {
  console.warn('[prepare:office-engine] WARNING: @firecrawl/anydoc not found in node_modules — anydoc conversion unavailable.');
}
const triple = currentNapiPlatformTriple();
if (triple && stagePackage(`@firecrawl/anydoc-${triple}`)) {
  console.log(`[prepare:office-engine] staged @firecrawl/anydoc-${triple} native binding`);
} else {
  console.warn(`[prepare:office-engine] WARNING: no @firecrawl/anydoc native binding for ${process.platform}/${process.arch} — ` +
    'anydoc conversion will report "napi binding failed to load" at runtime.');
}

// Stage canvas the same way; linkedom lazily requires it only if DOM canvas
// APIs are actually touched, which office-engine's usage never does, so a
// missing native build here is a latent no-op, not a crash.
if (stagePackage('canvas')) {
  console.log('[prepare:office-engine] staged canvas native binding');
} else {
  console.warn('[prepare:office-engine] WARNING: canvas not found in node_modules — linkedom DOM canvas APIs unavailable (not used by office-engine today).');
}

const xlsxName = process.platform === 'win32' ? 'allternit-xlsx-sidecar.exe' : 'allternit-xlsx-sidecar';
const xlsxSrcCandidates = [
  ...cargoBinaryCandidates(repoRoot, xlsxName, { releaseOnly: true }),
  path.join(
    repoRoot,
    'packages',
    '@allternit',
    'office-xlsx-engine',
    'crate',
    'target',
    'release',
    xlsxName,
  ),
];
const xlsxSrc = xlsxSrcCandidates.find((candidate) => fs.existsSync(candidate));
if (xlsxSrc) {
  const xlsxDestDir = path.join(outDir, 'bin');
  fs.mkdirSync(xlsxDestDir, { recursive: true });
  fs.copyFileSync(xlsxSrc, path.join(xlsxDestDir, xlsxName));
  console.log(`[prepare:office-engine] staged xlsx sidecar ${xlsxName}`);
} else {
  console.warn(
    '[prepare:office-engine] WARNING: xlsx sidecar binary not found — run `pnpm --filter @allternit/office-xlsx-engine sidecar:build`. Sheets will stay unavailable.',
  );
}

console.log('[prepare:office-engine] done');
