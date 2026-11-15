// Regenerates ../../Features/Onboarding/MeetBotCatalogData.swift from the web catalog.
// Usage (from this folder): AI_REPO=~/Desktop/allternit-workspace/allternit-ai node build.mjs && node gen.mjs
// (gen.mjs writes MeetBotCatalogData.swift here; move it into Features/Onboarding.)
// Needs a node_modules symlink here pointing at $AI_REPO/node_modules.
import { readdirSync } from 'node:fs';
const AI = process.env.AI_REPO || (process.env.HOME + '/Desktop/allternit-workspace/allternit-ai');
const esbuildDir = readdirSync(AI + '/node_modules/.pnpm').filter((d) => /^esbuild@0\.2/.test(d)).sort().pop();
const esbuild = await import(AI + '/node_modules/.pnpm/' + esbuildDir + '/node_modules/esbuild/lib/main.js');
const raw = { name: 'raw', setup(b) {
  b.onResolve({ filter: /\?raw$/ }, a => ({ path: (a.resolveDir + '/' + a.path.replace(/\?raw$/, '')), namespace: 'rawns' }));
  b.onLoad({ filter: /.*/, namespace: 'rawns' }, async (a) => ({ contents: (await import('node:fs')).readFileSync(a.path, 'utf8'), loader: 'text' }));
}};
await esbuild.build({ entryPoints: ['entry.ts'], bundle: true, platform: 'node', format: 'esm', outfile: 'bundle.mjs',
  tsconfig: AI + '/tsconfig.json', absWorkingDir: process.cwd(),
  nodePaths: [AI + '/node_modules'],
  jsx: 'automatic', loader: { '.svg': 'empty', '.css': 'empty', '.png': 'empty', '.md': 'text', '.json': 'json', '.woff2': 'empty', '.jpg':'empty','.webp':'empty','.mp3':'empty','.wasm':'empty' },
  define: { 'process.env.NODE_ENV': '"production"', 'import.meta.env': '{}' }, packages: 'external', logLevel: 'error', plugins: [raw] });
