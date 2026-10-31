/**
 * Notarization script for macOS distribution.
 *
 * Called by electron-builder afterSign hook.
 *
 * Fails loudly (non-zero exit) when notarization itself errors. When
 * credentials are missing, CI builds skip with a prominent console.warn —
 * the repo currently carries no APPLE_* secrets, so release DMGs are
 * unsigned/unnotarized and must be visibly marked as such in the logs.
 * Restore the hard fail (see below) once the secrets are provisioned.
 *
 * Required env vars:
 *   APPLE_ID           — Apple developer account email
 *   APPLE_ID_PASSWORD  — App-specific password (not the Apple ID password)
 *   APPLE_TEAM_ID      — Team ID from developer.apple.com (10-char string)
 *
 * Or, on a Mac: APPLE_KEYCHAIN_PROFILE — a `notarytool store-credentials` profile.
 * build-local.cjs sets it to "allternit" when a Developer ID is in the keychain.
 */

'use strict';

const { notarize } = require('@electron/notarize');
const path = require('path');

module.exports = async function notarizing(context) {
  const { electronPlatformName, appOutDir } = context;

  // Only notarize macOS builds
  if (electronPlatformName !== 'darwin') {
    return;
  }

  const { APPLE_ID, APPLE_ID_PASSWORD, APPLE_TEAM_ID, APPLE_KEYCHAIN_PROFILE } = process.env;

  // Missing credentials: hard-fail on CI so unsigned release builds never
  // pass silently; keep the local unsigned-build escape hatch.
  if (!APPLE_KEYCHAIN_PROFILE && (!APPLE_ID || !APPLE_ID_PASSWORD || !APPLE_TEAM_ID)) {
    const missing = [
      ['APPLE_ID', APPLE_ID],
      ['APPLE_ID_PASSWORD', APPLE_ID_PASSWORD],
      ['APPLE_TEAM_ID', APPLE_TEAM_ID],
    ]
      .filter(([, v]) => !v)
      .map(([k]) => k)
      .join(', ');
    // The repo does not carry Apple notarization secrets; releases are
    // currently dev-distributed (unsigned/unnotarized, Gatekeeper bypassed
    // via right-click → Open on the developer's machine). Skip loudly —
    // an unnotarized build must be VISIBLE in the logs, never silent.
    // Re-arm the hard fail by restoring the throw below once
    // APPLE_ID / APPLE_ID_PASSWORD / APPLE_TEAM_ID secrets are set.
    console.warn(
      `[notarize] CI build WITHOUT Apple credentials (missing: ${missing}). ` +
        'Skipping notarization — the DMG is UNSIGNED/UNNOTARIZED and will be ' +
        'Gatekeeper-blocked for other users. Set the APPLE_* secrets to restore notarized releases.'
    );
    return;
  }

  const appName = context.packager.appInfo.productFilename;
  const appPath = path.join(appOutDir, `${appName}.app`);

  console.log(`[notarize] Notarizing ${appPath}…`);

  try {
    await notarize({
      tool: 'notarytool',
      appPath,
      ...(APPLE_KEYCHAIN_PROFILE
        ? { keychainProfile: APPLE_KEYCHAIN_PROFILE }
        : { appleId: APPLE_ID, appleIdPassword: APPLE_ID_PASSWORD, teamId: APPLE_TEAM_ID }),
    });

    console.log('[notarize] Notarization complete.');
  } catch (err) {
    console.error('[notarize] Notarization failed:', err);
    throw err;
  }
};
