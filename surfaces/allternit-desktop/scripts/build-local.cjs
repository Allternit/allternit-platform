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
}

const extraArgs = [];
if (process.env.ALLTERNIT_BUILD_SUFFIX) {
  const pkg = require(path.join(__dirname, '..', 'package.json'));
  // An explicit -b<N> suffix carries the build number; without this the
  // bundle version fell back to a timestamp and never matched the DMG name.
  height ??= /^-b(\d+)$/.exec(process.env.ALLTERNIT_BUILD_SUFFIX)?.[1] ?? null;
  const buildVersion = process.env.ALLTERNIT_BUILD_VERSION || `${pkg.version}.${height ?? Date.now()}`;
  extraArgs.push(`-c.buildVersion=${buildVersion}`);
  try {
    fs.writeFileSync(
      path.join(__dirname, '..', 'resources', 'build-info.json'),
      JSON.stringify({
        version: pkg.version,
        buildVersion,
        buildSuffix: process.env.ALLTERNIT_BUILD_SUFFIX,
        builtAt: new Date().toISOString(),
      }, null, 2) + '\n'
    );
  } catch {
    // resources/ is staged by the prepare:* steps; if it is absent the app
    // simply reports no buildInfo.
  }
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

let command = process.execPath;
let args;
let shell = false;
try {
  const pkgJson = require.resolve('electron-builder/package.json', { paths: [path.join(__dirname, '..')] });
  args = [path.join(path.dirname(pkgJson), require(pkgJson).bin['electron-builder'])];
} catch {
  command = process.platform === 'win32' ? 'electron-builder.cmd' : 'electron-builder';
  shell = true;
}

const result = spawnSync(command, [...args, ...extraArgs, ...process.argv.slice(2)], {
  stdio: 'inherit',
  env: process.env,
  shell,
});
process.exit(result.status == null ? 1 : result.status);
