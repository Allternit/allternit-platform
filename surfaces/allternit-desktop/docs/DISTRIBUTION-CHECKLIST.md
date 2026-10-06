# Allternit Desktop — distribution checklist (macOS, Windows, Linux)

Living owner list for **shipping** installers. Unsigned local `release/mac-arm64`
packs are iteration only. Do not notarize those after the fact — rebuild with
certs so entitlements, hardened runtime, and `afterSign` all run.

**Status as of 2026-09-06**

| Track | Status |
|---|---|
| Product / ACI / Fabric Transport | In-scope engineering done on this Mac |
| Apple Developer Program | **Applied — in review** (org **Allternit LLC**, order `64BQ2F3SY7`) |
| Windows code-signing cert | Not purchased |
| Linux publish channel | Not chosen |
| Secrets rotation | After this workstream, not mid-session |

Fill the blanks as facts land. Do not paste passwords, `.p12` bytes, or
app-specific passwords into this file.

---

## Already true in the repo (do not redo)

| Item | Value |
|---|---|
| Bundle / app id | `com.allternit.desktop` |
| Product name | `Allternit Desktop` |
| Copyright | `Copyright © 2026 Allternit` |
| macOS entitlements | `build/entitlements.mac.plist` (hardened runtime, JIT, network, virtualization, screen capture, mic) |
| Notary hook | `scripts/notarize.cjs` via `build.afterSign` (`notarytool`) |
| Local unsigned hatch | `build.mac.identity: null` + skip notary when `APPLE_*` unset locally |
| CI hard-fail | `notarize.cjs` throws on CI if `APPLE_ID` / `APPLE_ID_PASSWORD` / `APPLE_TEAM_ID` missing |
| Release workflow | `.github/workflows/release-desktop.yml` (tag `desktop-v*`) |
| CI secret names (mac) | `APPLE_ID`, `APPLE_ID_PASSWORD`, `APPLE_TEAM_ID` |
| Windows publisher placeholder | `build.win.signtoolOptions.publisherName`: `Allternit` |
| Linux metadata | maintainer `Allternit <support@allternit.com>`, vendor `Allternit`, AppImage + deb targets |
| Auto-update feed (runtime) | `updateElectronApp({ repo: 'allternit/desktop' })` in `src/main/unified-main.ts` |

**Known mismatch to fix before first signed release:** `package.json` `build.publish` still says `Allternit/desktop`. Runtime + `src/main/manifest.ts` use `allternit/desktop`. Pick one GitHub repo and make publish + updater + README agree.

---

## Shared (all three platforms)

Do these while Apple is in review.

- [x] Bundle id frozen: `com.allternit.desktop`
- [x] Legal entity name for **all** certs (must match Apple team + Windows subject): **Allternit LLC**
- [x] Enrollment type: **Organization** (Allternit LLC)
- [x] Account Holder Apple ID (in review): `cartlidge.joseph@proton.me`
- [x] Apple 2FA on that Apple ID (confirmed on). Required for enrollment, App Store Connect, and `notarytool`. This is Apple 2FA (trusted phone + trusted device), not Proton-mail 2FA.
- [ ] Decide GitHub Releases repo: `allternit/desktop` vs `Allternit/desktop` — then fix `build.publish`
- [ ] Create GitHub Actions secrets **names** on that repo (values empty until certs exist):
  - [ ] `APPLE_ID`
  - [ ] `APPLE_ID_PASSWORD` (or switch hook to App Store Connect API key)
  - [ ] `APPLE_TEAM_ID`
  - [ ] `CSC_LINK` (Windows `.p12` as file or base64)
  - [ ] `CSC_KEY_PASSWORD`
  - [ ] `WIN_CSC_LINK` / `WIN_CSC_KEY_PASSWORD` (optional Windows-only overrides)
  - [ ] `DESKTOP_RELEASE_TOKEN` (GitHub PAT for `electron-builder` publish, if not using `GITHUB_TOKEN`)
- [ ] Wire `CSC_*` into `release-desktop.yml` Windows job (not present today — only Apple vars are passed)
- [ ] First signed version number plan (do not ship `1.1.0` unsigned as “the” production download)
- [ ] After this workstream: rotate session-touched secrets (Clerk, operator keys, Apple app-specific password, `.p12` password, `DESKTOP_RELEASE_TOKEN`)

---

## macOS — notarized DMG

### Can do now (before approval)

- [x] Apple Developer Program application submitted (in review)
- [x] Enrollment order id: `64BQ2F3SY7`
- [x] Enrolled as organization **Allternit LLC** (Account Holder `cartlidge.joseph@proton.me`)
- [x] Apple 2FA on `cartlidge.joseph@proton.me` (confirmed on)
- [ ] Prefer **App Store Connect API key** (`.p8` + Key ID + Issuer ID) over an app-specific password — you can only mint the key after approval; decide now so `notarize.cjs` is updated once, not twice
- [ ] Build machine: this Mac’s Keychain will hold Developer ID certs; plan a dedicated login keychain, not iCloud keychain
- [ ] Do **not** flip `build.mac.identity` off `null` until the Developer ID Application cert is in Keychain
- [ ] Do **not** notarize the current unsigned `release/mac-arm64/*.app`

### After Apple approves

- [ ] Note Team ID (10 chars): `__________`
- [ ] Certificates → Developer ID Application (signs the `.app`)
- [ ] Certificates → Developer ID Installer (only if we ship `.pkg`; DMG uses Application)
- [ ] Download certs into the build Mac Keychain; confirm `security find-identity -v -p codesigning` shows the CN
- [ ] Set `CSC_NAME` to that CN, e.g. `Developer ID Application: Allternit LLC (TEAMID)`
- [ ] Mint notary login: app-specific password **or** App Store Connect API key
- [ ] Put `APPLE_ID`, `APPLE_ID_PASSWORD` (or API key), `APPLE_TEAM_ID` in GitHub secrets — not in the repo
- [ ] Remove or override `"identity": null` for the release build
- [ ] Do **not** set `CSC_IDENTITY_AUTO_DISCOVERY=false` on that run
- [ ] `cd surfaces/allternit-desktop && pnpm run build:electron:dmg`
- [ ] Pass:
  - `spctl --assess --verbose "release/mac-arm64/Allternit Desktop.app"`
  - `xcrun stapler validate` on the `.app` and the `.dmg`
  - Gatekeeper opens the DMG with no right-click bypass (production gate **I1**)

---

## Windows — Authenticode (SmartScreen)

Not notarization. A purchased code-signing certificate. Start this **now** — EV identity checks often take longer than Apple review.

### Can do now

- [ ] Choose **EV** (SmartScreen reputation starts cleaner) vs **OV** (cheaper, SmartScreen nags until reputation builds)
- [ ] Vendor: DigiCert / Sectigo / SSL.com (pick one)
- [ ] Same legal name as Apple team: **Allternit LLC**
- [ ] Hardware token / USB key plan if EV (CI must be able to sign — cloud HSM or a dedicated Windows runner with the token)
- [ ] Subject CN must match the cert. Today `publisherName` is `"Allternit"` — set it to **Allternit LLC** on the first signed build and never change it without an updater migration
- [ ] Add `CSC_LINK` + `CSC_KEY_PASSWORD` to the Windows job in `release-desktop.yml`

### After the cert arrives

- [ ] Export `.p12` (or attach EV token to the Windows runner)
- [ ] Set `CSC_LINK` / `CSC_KEY_PASSWORD` in GitHub secrets
- [ ] Build on **Windows CI** (`windows-latest`), not cross-sign from this Mac
- [ ] `pnpm run dist -- --win --publish never` on that runner
- [ ] Confirm the `.exe` is signed (`signtool verify /pa`)
- [ ] Lock `win.signtoolOptions.publisherName` to the cert subject; do not change it without an updater migration
- [ ] Unsigned CI `.exe` stays a smoke artifact until this is done

---

## Linux — not Developer ID

No Apple/Authenticode equivalent for the AppImage we emit today. Pick a **channel**, then sign that channel.

### Can do now

- [ ] Choose one shipping channel:
  - GPG-signed `.deb` repo
  - Flathub
  - AppImage **plus** a detached GPG signature published next to it
- [ ] Generate an Allternit packaging GPG key (offline / hardware if possible). Record key id here, not the private key: `________________`
- [ ] Publish the public key (GitHub + website) before the first signed artifact
- [ ] If Flathub: create the Flathub account / repo under Allternit
- [ ] Confirm `build.linux` maintainer/vendor strings match the GPG identity
- [ ] Keep unsigned AppImage/deb as CI smoke only — do not put them on the download page as the Linux product

### After the channel exists

- [ ] Sign the artifact (debsig / `gpg --detach-sign` / Flathub bot)
- [ ] Publish `latest-linux.yml` only for signed AppImages
- [ ] Document verify steps on the download page

---

## First signed release day (all three)

- [ ] Tag `desktop-vX.Y.Z` (not an unsigned 1.1.0 dir pack)
- [ ] `release-desktop.yml` produces notarized macOS DMG + stapled ticket
- [ ] Windows job has `CSC_*` and emits a signed Setup `.exe`
- [ ] Linux job either skipped for “shipping” or emits the chosen signed channel
- [ ] GitHub Release includes `latest-mac.yml`, `latest.yml`, and (if shipping Linux AppImage) `latest-linux.yml`
- [ ] README download table only links artifacts that passed the checks above
- [ ] Rotate secrets after the session that handled the certs

---

## Commands (copy when identities exist)

```bash
# macOS shipping DMG — cert in Keychain, APPLE_* set, identity NOT null
cd surfaces/allternit-desktop
unset CSC_IDENTITY_AUTO_DISCOVERY
export CSC_NAME="Developer ID Application: Allternit LLC (TEAMID)"
pnpm run build:electron:dmg

# Windows — on windows-latest, p12 in CSC_LINK
pnpm run dist -- --win --publish never
```
