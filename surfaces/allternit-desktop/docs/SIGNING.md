# Allternit Desktop — Signing & Notarization

This document describes the environment variables and certificates required to
produce signed, notarized, and SmartScreen-friendly release artifacts.

**Owner checklist (macOS + Windows + Linux, including what to do while Apple
Developer Program is in review):** [DISTRIBUTION-CHECKLIST.md](./DISTRIBUTION-CHECKLIST.md).

---

## macOS

macOS builds require **Apple Developer ID Application** and **Developer ID
Installer** certificates, plus notarization credentials.

### Required environment variables

```bash
# Code signing identity — match the Common Name in Keychain / CI secret
CSC_NAME="Developer ID Application: Allternit Inc (XXXXXXXXXX)"

# Notarization (App Store Connect API key style is recommended)
APPLE_ID="developer@allternit.com"
APPLE_ID_PASSWORD="abcd-efgh-ijkl-mnop"   # app-specific password
APPLE_TEAM_ID="XXXXXXXXXX"                # 10-character team id
```

### Notarization flow

`electron-builder` calls `scripts/notarize.cjs` via the `afterSign` hook. The
script:

1. Hard-fails (non-zero exit) on CI builds when `APPLE_ID`, `APPLE_ID_PASSWORD`,
   or `APPLE_TEAM_ID` is missing — an unsigned/unnotarized release build must
   never pass silently. On local (non-CI) builds it skips with a warning
   instead, so unsigned local iteration still works.
2. Uses `@electron/notarize` with `notarytool` and the credentials above.
3. Staples the ticket to the `.app` bundle.

`package.json` sets `"notarize": false` so electron-builder does not run its
built-in legacy notarization path; our `afterSign` script handles it.

### Local unsigned build

For iteration on packaging logic without a signing certificate:

```bash
cd surfaces/allternit-desktop
pnpm run dist -- --publish never
```

The resulting `.app` will show a Gatekeeper warning on first launch, which is
expected for unsigned local builds.

## Windows

Windows builds should be signed with an **EV or standard code-signing
certificate** to avoid SmartScreen warnings and to allow `electron-updater` to
verify update signatures.

### Required environment variables

```bash
# PKCS#12 certificate file or Base64-encoded string
CSC_LINK="/path/to/allternit-windows.p12"
CSC_KEY_PASSWORD="xxxxxxxx"

# Optional: keep publisherName empty until you have a real signed cert.
# Setting publisherName on an unsigned build breaks auto-updates because
# electron-updater will reject the mismatch.
```

### Unsigned CI builds

The repository intentionally omits `win.signtoolOptions.publisherName`. This
lets unsigned nightly/CI builds still auto-update as long as the released
artifacts are later signed before publication. Once you start signing, add the
certificate subject to `win.signtoolOptions.publisherName` and never remove or
change it without a migration plan, or you will strand installed users.

## Linux

Linux packages (`.deb` and `.AppImage`) are not code-signed. Unsigned CI
artifacts are smoke tests, not production releases — do not publish them as
the shipping Linux product until package metadata and a distribution channel
are ready.

The `linux` block in `package.json` defines:

- `executableName`: `allternit`
- `desktop` entry with `Name`, `Comment`, and `Keywords`
- `maintainer` and `vendor` metadata for `.deb`

## Auto-updates

Desktop auto-updates use `update-electron-app`, which reads from the public
Electron update feed backed by GitHub Releases. The feed repository is
configured in `src/main/unified-main.ts` as:

```ts
repo: 'Allternit/desktop'
```

and in `package.json` `build.publish` as:

```json
{
  "provider": "github",
  "owner": "Allternit",
  "repo": "desktop"
}
```

Release artifacts must include `latest-mac.yml` (macOS), `latest.yml`
(Windows), and `latest-linux.yml` (Linux AppImage) for `electron-updater` to
find them. electron-builder emits these metadata files when publishing is
enabled.

## Staging resources before packaging

Before `pnpm run dist`, run the full staging pipeline so the packaged app
includes the Rust API binary, brain, voice service, and platform static export:

```bash
bash scripts/build-desktop.sh
```

For faster iteration when only the API changed:

```bash
cargo build --release -p allternit-api
npm run stage:api-binary
```
