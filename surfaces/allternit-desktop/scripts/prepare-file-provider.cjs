/**
 * Builds the Files drive File Provider extension and its helper (plan step
 * 23) with native/file-provider/build.sh, so the afterPack hook
 * (scripts/embed-file-provider.cjs) can put them in the app bundle.
 *
 * macOS only; a no-op elsewhere. ALLTERNIT_SKIP_FILE_PROVIDER=1 skips it
 * (the afterPack hook then skips embedding too).
 *
 * Idempotent: does nothing when build/ is newer than every Swift source it
 * is made from (the kit, the shared iOS extension sources, this folder).
 */
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

if (process.platform !== 'darwin') {
  console.log('[file-provider] Not macOS: nothing to build.');
  process.exit(0);
}
if (process.env.ALLTERNIT_SKIP_FILE_PROVIDER === '1') {
  console.log('[file-provider] ALLTERNIT_SKIP_FILE_PROVIDER=1: skipped.');
  process.exit(0);
}

const native = path.resolve(__dirname, '..', 'native', 'file-provider');
const ios = path.resolve(__dirname, '..', '..', 'allternit-mobile', 'ios');
const contractGen = path.resolve(__dirname, '..', '..', '..', 'contracts', 'files-drive', 'gen');
const sources = [
  path.join(ios, 'Packages', 'AllternitFilesKit', 'Sources'),
  path.join(ios, 'AllternitFileProvider'),
  contractGen,
  path.join(native, 'Extension'),
  path.join(native, 'Helper'),
  path.join(native, 'Badges'),
  path.join(native, 'build.sh'),
];
const outputs = ['AllternitFileProvider', 'allternit-files-domains'].map((f) => path.join(native, 'build', f));

function newestMtime(target) {
  const stat = fs.statSync(target);
  if (!stat.isDirectory()) return stat.mtimeMs;
  let newest = stat.mtimeMs;
  for (const entry of fs.readdirSync(target)) newest = Math.max(newest, newestMtime(path.join(target, entry)));
  return newest;
}

if (outputs.every((f) => fs.existsSync(f))) {
  const built = Math.min(...outputs.map((f) => fs.statSync(f).mtimeMs));
  if (sources.every((s) => !fs.existsSync(s) || newestMtime(s) <= built)) {
    console.log('[file-provider] Up to date.');
    process.exit(0);
  }
}

const result = spawnSync('bash', [path.join(native, 'build.sh')], { stdio: 'inherit' });
process.exit(result.status == null ? 1 : result.status);
