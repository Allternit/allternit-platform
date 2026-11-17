/**
 * Stage the Mac Bluetooth keyboard helper (`allternit-bthid`) into resources/bin/.
 *
 * Desktop runs it for the `bt-mac` Phone Use route (src/main/phone-bt-mac.ts):
 * this Mac shows up as a Bluetooth keyboard and mouse ("Allternit Keyboard")
 * that the person pairs with their own iPhone. It is a small SwiftPM package
 * (cmd/allternit-bthid, using surfaces/allternit-mobile/ios/Packages/AllternitBTHID;
 * public CoreBluetooth only, macOS 14+), built as a universal binary
 * (arm64 + x86_64) so one file serves both Mac DMGs.
 *
 * If a release job already built and copied the binary here, this is a no-op.
 * macOS only: Windows and Linux skip it and the route reports unavailable there.
 *
 * REQUIRED on macOS by default: a failed build stops packaging.
 * ALLTERNIT_REQUIRE_BTHID=0 lets packaging continue without it (the route then
 * says it is not in this build); ALLTERNIT_SKIP_BTHID=1 skips it on purpose.
 */

'use strict';

const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const desktopDir = path.resolve(__dirname, '..');
const repoRoot = path.resolve(desktopDir, '..', '..');
const packageDir = path.join(repoRoot, 'cmd', 'allternit-bthid');
const name = 'allternit-bthid';
const dest = path.join(desktopDir, 'resources', 'bin', name);
const required = process.env.ALLTERNIT_REQUIRE_BTHID !== '0';

function log(message) {
  process.stdout.write(`[prepare-bthid] ${message}\n`);
}

function alreadyStaged() {
  try {
    return fs.existsSync(dest) && fs.statSync(dest).size > 64 * 1024;
  } catch {
    return false;
  }
}

function swiftBuild(args) {
  log(`swift build -c release ${args.join(' ')} --package-path cmd/allternit-bthid`);
  const result = spawnSync('swift', ['build', '-c', 'release', ...args, '--package-path', packageDir], {
    cwd: repoRoot,
    stdio: 'inherit',
    env: process.env,
  });
  return result.status === 0;
}

function main() {
  if (process.platform !== 'darwin') {
    log('skipped (macOS only)');
    return;
  }
  if (process.env.ALLTERNIT_SKIP_BTHID === '1') {
    log('skipped (ALLTERNIT_SKIP_BTHID=1); the Bluetooth keyboard route will be unavailable in this build');
    return;
  }
  if (alreadyStaged()) {
    log(`already staged ${dest}`);
    return;
  }
  // Universal first (needs Xcode's build system); a single-arch build is the fallback for this Mac only.
  const universal = path.join(packageDir, '.build', 'apple', 'Products', 'Release', name);
  const single = path.join(packageDir, '.build', 'release', name);
  let source = null;
  if (swiftBuild(['--arch', 'arm64', '--arch', 'x86_64']) && fs.existsSync(universal)) {
    source = universal;
  } else {
    log('universal build failed; building for this Mac only');
    if (swiftBuild([]) && fs.existsSync(single)) source = single;
  }
  if (!source) throw new Error('allternit-bthid binary not found after swift build');
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(source, dest);
  fs.chmodSync(dest, 0o755);
  log(`staged ${source} -> ${dest}`);
}

try {
  main();
} catch (err) {
  if (required) {
    process.stderr.write(`[prepare-bthid] ✗ ${err.message}\n`);
    process.exit(1);
  }
  process.stderr.write(
    `[prepare-bthid] WARNING: ${err.message}. Packaging continues without it ` +
      '(ALLTERNIT_REQUIRE_BTHID=0); the Bluetooth keyboard route will say it is not in this build.\n'
  );
}
