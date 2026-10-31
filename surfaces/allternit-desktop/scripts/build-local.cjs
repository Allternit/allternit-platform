#!/usr/bin/env node
// Local electron-builder launcher. Stamps a build number into artifact names
// (Allternit-Desktop-1.1.1-b4177-arm64.dmg) so consecutive ad-hoc builds of
// the same version are distinguishable — the version field only moves on
// release commits, so without this every local rebuild overwrites the same
// filename and latest-mac.yml.
//
// Rules:
//   - CI/release builds keep clean version-only names: the suffix is only
//     set when CI / GITHUB_ACTIONS are unset.
//   - ALLTERNIT_BUILD_SUFFIX overrides everything (set to "" for clean names
//     even locally). Default local suffix: -b<git rev-list count>.
//   - When the suffix is applied, buildVersion (CFBundleVersion on macOS,
//     FileVersion on Windows) is set to <version>.<height> and a
//     resources/build-info.json is written next to the sidecars so the
//     running app can report its build number (app:get-info → buildInfo).
//     ALLTERNIT_BUILD_VERSION overrides the buildVersion string.
//   - ALLTERNIT_HOST_ONLY=1 (build-desktop.sh without --release): one DMG
//     for this Mac's arch instead of every mac target.
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

let height = null;
if (
  process.env.ALLTERNIT_BUILD_SUFFIX === undefined &&
  !process.env.CI &&
  !process.env.GITHUB_ACTIONS
) {
  const rev = spawnSync('git', ['rev-list', '--count', 'HEAD'], { encoding: 'utf8' });
  height = rev.status === 0 && rev.stdout.trim() ? rev.stdout.trim() : String(Date.now());
  process.env.ALLTERNIT_BUILD_SUFFIX = `-b${height}`;
} else if (process.env.ALLTERNIT_BUILD_SUFFIX === undefined) {
  // CI without a suffix: clean names. electron-builder fails on an undefined
  // ${env.ALLTERNIT_BUILD_SUFFIX} in artifactName ("cannot expand pattern").
  process.env.ALLTERNIT_BUILD_SUFFIX = '';
}

const extraArgs = [];
const pkg = require(path.join(__dirname, '..', 'package.json'));
let buildVersion;
if (process.env.ALLTERNIT_BUILD_SUFFIX) {
  // An explicit -b<N> suffix carries the build number; without this the
  // bundle version fell back to a timestamp and never matched the DMG name.
  height ??= /^-b(\d+)$/.exec(process.env.ALLTERNIT_BUILD_SUFFIX)?.[1] ?? null;
  buildVersion = process.env.ALLTERNIT_BUILD_VERSION || `${pkg.version}.${height ?? Date.now()}`;
  extraArgs.push(`-c.buildVersion=${buildVersion}`);
}
// Written on every build, CI release builds too (no suffix, no build
// number): a stale file from an earlier local build once shipped in public
// releases, so Settings showed "1.1.13 (build 1.1.1.<timestamp>)".
try {
  fs.writeFileSync(
    path.join(__dirname, '..', 'resources', 'build-info.json'),
    JSON.stringify({
      version: pkg.version,
      ...(buildVersion ? { buildVersion } : {}),
      buildSuffix: process.env.ALLTERNIT_BUILD_SUFFIX || '',
      builtAt: new Date().toISOString(),
    }, null, 2) + '\n'
  );
} catch {
  // resources/ is staged by the prepare:* steps; if it is absent the app
  // simply reports no buildInfo.
}

// Windows cannot hold an empty environment variable, so the clean-name case
// (ALLTERNIT_BUILD_SUFFIX="") reaches electron-builder as "not defined" and
// the ${env.ALLTERNIT_BUILD_SUFFIX} artifact patterns fail to expand. Pass
// the same patterns without the suffix instead.
if (process.platform === 'win32' && !process.env.ALLTERNIT_BUILD_SUFFIX) {
  extraArgs.push(
    '-c.artifactName=Allternit-Desktop-${version}-${arch}.${ext}',
    '-c.nsis.artifactName=Allternit-Setup-${version}.exe',
  );
}

// build-desktop.sh without --release: one DMG for this Mac (the mac config
// also makes x64 and zip artifacts, which only a public release needs).
// Arguments given on the command line win.
if (
  process.env.ALLTERNIT_HOST_ONLY === '1' &&
  process.platform === 'darwin' &&
  !process.argv.slice(2).some((a) => /^--(mac|arm64|x64|universal)$/.test(a))
) {
  extraArgs.push('--mac', 'dmg', process.arch === 'arm64' ? '--arm64' : '--x64');
}

let command = process.execPath;
// A Developer ID in the keychain signs and notarizes this build. Without one, a
// local build uses an Apple Development cert: a stable team ID means the Keychain
// keeps "Always Allow" across builds on this Mac (it won't open on other Macs).
// With neither, or on CI, the package.json identity "-" (ad-hoc) stands.
if (process.platform === 'darwin' && !process.env.CSC_IDENTITY_AUTO_DISCOVERY) {
  const { execFileSync } = require('child_process');
  const identities = execFileSync('security', ['find-identity', '-v', '-p', 'codesigning'], { encoding: 'utf8' });
  const developerId = identities.match(/"Developer ID Application: ([^"]+)"/);
  if (developerId) {
    extraArgs.push(`-c.mac.identity=${developerId[1]}`);
    process.env.APPLE_KEYCHAIN_PROFILE = process.env.APPLE_KEYCHAIN_PROFILE || 'allternit';
    console.log(`[build-local] Signing with Developer ID: ${developerId[1]}`);
  } else if (!process.env.CI) {
    const appleDevelopment = identities.match(/"Apple Development: ([^"]+)"/);
    if (appleDevelopment) {
      extraArgs.push(`-c.mac.identity=${appleDevelopment[1]}`);
      console.log(`[build-local] Signing with Apple Development: ${appleDevelopment[1]}`);
    }
  }
}

let args;
let shell = false;
try {
  const pkgJson = require.resolve('electron-builder/package.json', { paths: [path.join(__dirname, '..')] });
  args = [path.join(path.dirname(pkgJson), require(pkgJson).bin['electron-builder'])];
} catch {
  command = process.platform === 'win32' ? 'electron-builder.cmd' : 'electron-builder';
  shell = true;
}

const argv = [...args, ...extraArgs, ...process.argv.slice(2)];
const env = { ...process.env };
// Ad-hoc signing (mac.identity "-") checks every bundle file in parallel and
// hits EMFILE; see graceful-fs-preload.cjs.
if (process.platform === 'darwin') {
  const preload = `--require ${JSON.stringify(path.join(__dirname, 'graceful-fs-preload.cjs'))}`;
  env.NODE_OPTIONS = [env.NODE_OPTIONS, preload].filter(Boolean).join(' ');
}

const result = spawnSync(command, argv, {
  stdio: 'inherit',
  env,
  shell,
});
process.exit(result.status == null ? 1 : result.status);
