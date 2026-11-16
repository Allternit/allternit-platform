#!/usr/bin/env node
/**
 * electron-builder afterPack hook (macOS): puts the Files drive File Provider
 * into the app bundle and signs it (plan step 23, spike 6).
 *
 *   Contents/PlugIns/AllternitFileProvider.appex   the extension (sandboxed)
 *   Contents/MacOS/allternit-files-domains         the helper Electron main runs
 *
 * Built by `npm run prepare:file-provider` (native/file-provider/build.sh).
 *
 * Signing order matters. electron-builder never signs Contents/PlugIns (its
 * own ignore rule) and skips the helper (mac.signIgnore in package.json), so
 * both are signed here, inside-out, with their own entitlements, before
 * electron-builder signs and seals the app around them. notarize.cjs
 * (afterSign) then notarizes the whole bundle, extension included.
 *
 * The extension and helper share a team-prefixed App Group
 * (<TEAM>.com.allternit.desktop.files). It needs no provisioning profile,
 * but it needs a team, so an ad-hoc build (identity "-", CI without
 * certificates) ships without Files in Finder and says so in the log.
 *
 * Env:
 *   ALLTERNIT_SKIP_FILE_PROVIDER=1      do not embed
 *   ALLTERNIT_FILE_PROVIDER_IDENTITY    signing identity (else CSC_NAME, else mac.identity)
 *   ALLTERNIT_APPLE_TEAM_ID / APPLE_TEAM_ID   team id (else read from the certificate)
 */
'use strict';

const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const NATIVE = path.join(__dirname, '..', 'native', 'file-provider');
const BUILD = path.join(NATIVE, 'build');
const APPEX = 'AllternitFileProvider.appex';
const HELPER = 'allternit-files-domains';

/** Picks the keychain identity electron-builder will use, or null for ad-hoc. */
function resolveIdentity(configured) {
  const wanted = (process.env.ALLTERNIT_FILE_PROVIDER_IDENTITY || process.env.CSC_NAME || configured || '').trim();
  if (!wanted || wanted === '-') return null;
  const listing = execFileSync('security', ['find-identity', '-v', '-p', 'codesigning'], { encoding: 'utf8' });
  const identities = [...listing.matchAll(/^\s*\d+\)\s+([0-9A-F]{40})\s+"([^"]+)"/gm)].map((m) => ({ hash: m[1], name: m[2] }));
  // build-local.cjs passes the name without its "Developer ID Application: " /
  // "Apple Development: " prefix, the way electron-builder accepts it.
  const pick =
    identities.find((i) => i.name === wanted || i.hash === wanted) ||
    identities.find((i) => i.name === `Developer ID Application: ${wanted}`) ||
    identities.find((i) => i.name === `Apple Development: ${wanted}`) ||
    identities.find((i) => i.name.endsWith(`: ${wanted}`));
  if (!pick) throw new Error(`[file-provider] signing identity "${wanted}" is not in the keychain`);
  return pick;
}

function teamIdOf(identity) {
  const fromEnv = process.env.ALLTERNIT_APPLE_TEAM_ID || process.env.APPLE_TEAM_ID;
  if (fromEnv) return fromEnv.trim();
  // An Apple Development name ends in a personal id, not the team: the team
  // is the certificate's OU.
  const pem = execFileSync('security', ['find-certificate', '-c', identity.name, '-p'], { encoding: 'utf8' });
  const subject = execFileSync('openssl', ['x509', '-noout', '-subject'], { input: pem, encoding: 'utf8' });
  const ou = /OU\s*=\s*([A-Z0-9]{10})\b/.exec(subject);
  if (!ou) throw new Error(`[file-provider] no team id (OU) in the certificate of "${identity.name}"; set ALLTERNIT_APPLE_TEAM_ID`);
  return ou[1];
}

function fillTemplate(file, values) {
  let text = fs.readFileSync(file, 'utf8');
  for (const [key, value] of Object.entries(values)) text = text.split(`__${key}__`).join(value);
  const left = /__[A-Z_]+__/.exec(text);
  if (left) throw new Error(`[file-provider] ${path.basename(file)} still has ${left[0]}`);
  return text;
}

function codesign(target, identity, { entitlements, identifier, timestamp }) {
  const args = ['--force', '--options', 'runtime', '--sign', identity.hash, timestamp ? '--timestamp' : '--timestamp=none'];
  if (identifier) args.push('--identifier', identifier);
  if (entitlements) args.push('--entitlements', entitlements);
  args.push(target);
  execFileSync('codesign', args, { stdio: 'inherit' });
}

exports.default = async function embedFileProvider(context) {
  if (context.electronPlatformName !== 'darwin') return;
  if (process.env.ALLTERNIT_SKIP_FILE_PROVIDER === '1') {
    console.log('[file-provider] ALLTERNIT_SKIP_FILE_PROVIDER=1: not embedding the Files File Provider.');
    return;
  }
  const extBinary = path.join(BUILD, 'AllternitFileProvider');
  const helperBinary = path.join(BUILD, HELPER);
  if (!fs.existsSync(extBinary) || !fs.existsSync(helperBinary)) {
    throw new Error('[file-provider] native/file-provider/build is missing: run `npm run prepare:file-provider` first.');
  }

  const packager = context.packager;
  const identity = resolveIdentity(packager.platformSpecificBuildOptions.identity);
  if (!identity) {
    console.warn(
      '[file-provider] Ad-hoc signed build: Files in Finder needs a team signature (Developer ID or Apple Development) ' +
        'for its App Group, so this build ships WITHOUT the File Provider extension.',
    );
    return;
  }
  const team = teamIdOf(identity);
  const appId = packager.appInfo.id || packager.config.appId;
  const values = {
    BUNDLE_ID: `${appId}.fileprovider`,
    APP_GROUP: `${team}.${appId}.files`,
    VERSION: packager.appInfo.buildVersion || packager.appInfo.version,
  };
  // Notarization needs a secure timestamp; a local Apple Development build
  // works offline without one.
  const timestamp = identity.name.startsWith('Developer ID Application:');

  const app = path.join(context.appOutDir, `${packager.appInfo.productFilename}.app`);
  const appex = path.join(app, 'Contents', 'PlugIns', APPEX);
  fs.rmSync(appex, { recursive: true, force: true });
  fs.mkdirSync(path.join(appex, 'Contents', 'MacOS'), { recursive: true });
  fs.mkdirSync(path.join(appex, 'Contents', 'Resources'), { recursive: true });
  fs.writeFileSync(path.join(appex, 'Contents', 'Info.plist'), fillTemplate(path.join(NATIVE, 'Extension', 'Info.plist'), values));
  fs.copyFileSync(extBinary, path.join(appex, 'Contents', 'MacOS', 'AllternitFileProvider'));
  fs.chmodSync(path.join(appex, 'Contents', 'MacOS', 'AllternitFileProvider'), 0o755);
  for (const icns of fs.readdirSync(path.join(BUILD, 'badges')).filter((f) => f.endsWith('.icns'))) {
    fs.copyFileSync(path.join(BUILD, 'badges', icns), path.join(appex, 'Contents', 'Resources', icns));
  }
  const helper = path.join(app, 'Contents', 'MacOS', HELPER);
  fs.copyFileSync(helperBinary, helper);
  fs.chmodSync(helper, 0o755);

  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'allternit-file-provider-'));
  try {
    const extEntitlements = path.join(tmp, 'extension.entitlements');
    const helperEntitlements = path.join(tmp, 'helper.entitlements');
    fs.writeFileSync(extEntitlements, fillTemplate(path.join(NATIVE, 'Extension', 'AllternitFileProvider.entitlements'), values));
    fs.writeFileSync(helperEntitlements, fillTemplate(path.join(NATIVE, 'Helper', 'helper.entitlements'), values));
    codesign(helper, identity, { entitlements: helperEntitlements, identifier: `${appId}.files-domains`, timestamp });
    codesign(appex, identity, { entitlements: extEntitlements, timestamp });
    execFileSync('codesign', ['--verify', '--strict', appex], { stdio: 'inherit' });
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
  console.log(`[file-provider] Embedded ${APPEX} + ${HELPER} (${identity.name}, group ${values.APP_GROUP}).`);
};
