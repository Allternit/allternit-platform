# allternit-sideload

The Allternit Desktop sidecar that signs the Phone Link iPhone helper with the
user's own Apple ID. A free Apple ID works. Xcode is not needed.

The helper is two packages from our iCTRL fork: the XCUITest runner
(`iCTRLUITests-Runner.ipa`) and the host app (`iCTRL.ipa`). Desktop downloads
the unsigned builds and checks their pinned checksums (`phone-ios-runner.ts`
`ensureRunnerArtifacts`). It then runs this program to sign them and installs
the result with go-ios. **This program never talks to the phone.**

It is built on [isideload](https://github.com/nab138/isideload) 0.4.6 (MIT).
isideload handles Apple ID login with 2FA, anisette v3, the development
certificate, App IDs, device registration and provisioning profiles. Signing
goes through apple-codesign-quick.

## Build and test

This crate is its own cargo workspace with its own `Cargo.lock`. isideload needs
`aes-gcm ^0.11.1`, but allternit-api's `russh 0.60.3` pins
`aes-gcm =0.11.0-rc.3`. Inside the root workspace, cargo resolves that conflict
by quietly downgrading russh in allternit-api.

```sh
CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.shared-target/phone-link-ios \
  cargo test --manifest-path cmd/allternit-sideload/Cargo.toml
CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.shared-target/phone-link-ios \
  cargo build --release --manifest-path cmd/allternit-sideload/Cargo.toml
```

Desktop packaging runs `npm run prepare:sideload`
(`surfaces/allternit-desktop/scripts/prepare-sideload.cjs`), and the release
workflow builds a universal binary. Both steps are **optional** for now. See
"Anisette" below.

## Protocol

One sign per process. The binary is started as `allternit-sideload --json`.
Requests come in as JSON lines on stdin and events go out as JSON lines on
stdout. The authoritative copies are `src/protocol.rs` and the comment above
`SideloadSigner` in `surfaces/allternit-desktop/src/main/phone-ios-runner.ts`.
Keep the two in sync.

Desktop → sidecar:

```json
{"op":"sign","apple_id":"me@example.com","password":"…","udid":"00008110-…","device_name":"iPhone",
 "runner_ipa":"/abs/iCTRLUITests-Runner.ipa","host_ipa":"/abs/iCTRL.ipa","out_dir":"/abs/signed/<udid>",
 "anisette_url":"https://ani.jkcoxson.com","bundle_prefix":"com.allternit.phonelink"}
{"op":"two_factor","code":"123456"}
{"op":"two_factor_sms"}
{"op":"cancel"}
```

- You can leave out `password`, or send it empty, when a saved session exists.
  If no session can be used, the reply is `password_required`.
- `anisette_url` and `bundle_prefix` are optional. `bundle_prefix` is the
  manifest's `bundlePrefix`.
- Closing stdin cancels, the same as `cancel`, and the process exits with code
  130.

Sidecar → Desktop:

```json
{"event":"progress","fraction":0.4,"step":"Setting up your developer certificate"}
{"event":"two_factor_required","sms":false,"phone":"•••• 12","retry_reason":"Incorrect verification code"}
```

`two_factor_sms` answers a device-route prompt (`sms:false`) by having Apple
text the code to the trusted number in `phone` instead; the next prompt then
has `sms:true`. With no trusted number it asks the devices again.

```json
{"event":"done","runner_ipa":"/abs/runner-signed.ipa","host_ipa":"/abs/host-signed.ipa",
 "bundle_prefix":"com.allternit.phonelink.abcde12345",
 "runner_bundle_id":"com.allternit.phonelink.abcde12345.ictrl.uitests.xctrunner",
 "host_bundle_id":"com.allternit.phonelink.abcde12345.ictrl","team_id":"ABCDE12345",
 "expires_at":"2026-10-23T12:00:00Z","session_reused":false}
{"event":"error","code":"app_id_limit","message":"…"}
```

`expires_at` is the earlier of the two profiles' expiry dates. A free account's
profile lasts 7 days. Desktop re-signs a day before that (`RESIGN_MARGIN_MS`).

Error codes, with how Desktop shows them (`phone-link-ios.ts` `signRunner`):

| code | meaning | Desktop |
|---|---|---|
| `bad_credentials` | Apple rejected the Apple ID or password (GSA -20101/-22406/-20283) | asks again |
| `password_required` | no password was sent and the saved session can't be used | asks again |
| `two_factor_failed` | the code was wrong, timed out after 10 min, or 2FA was cancelled | asks again |
| `app_id_limit` | a free account can create 10 new App IDs per 7 days (dev error 9120, or our own check before calling Apple) | "weekly limit, try again in a few days" |
| `certificate_limit` | too many development certificates (dev error 7460). We never revoke them | "remove one at developer.apple.com" |
| `device_limit` | too many registered devices | "remove one at developer.apple.com" |
| `anisette_unavailable` | the anisette server is unreachable or refused the request | "try again in a few minutes" |
| `invalid_request` | Desktop sent a malformed request | generic |
| `signing_failed` | anything else, with a one-line detail | generic |

## What it does

1. It loads the saved session for this Apple ID. If it can't, it logs in with
   the password, sending `two_factor_required` and then reading a `two_factor`
   line from stdin.
2. It picks the team. It reuses the last team for this Apple ID, or takes the
   first one.
3. It registers the phone's UDID with the team.
4. It gets the development certificate. isideload finds the existing
   certificate for our stored key and machine name ("Allternit Desktop"), and
   only creates one if that certificate is gone.
5. It unpacks both .ipa files and rewrites their bundle ids to the team's
   prefix (`src/bundle.rs`). **Each `PlugIns/*.xctest` takes the runner's new
   id and gets no App ID of its own.**
6. It registers only the App IDs that are missing, and checks Apple's
   remaining allowance before asking. Then it downloads one profile per app.
7. It signs inside-out with isideload's `sign::sign` (apple-codesign-quick) and
   repacks to `<out_dir>/runner-signed.ipa` and `<out_dir>/host-signed.ipa`.

### The .xctest fix (and why we don't call `Sideloader::sign_app`)

isideload treats every `PlugIns/*` bundle as an app extension. It expects a
plug-in's id to extend the app's id (`<app>.<suffix>`), and it registers an App
ID and a profile for each plug-in. An XCUITest runner's test bundle is not an
extension. It must carry the **runner's** bundle id and be signed with the
runner's team and certificate, with no profile or entitlements of its own.
This is what `ictrl-fork/Scripts/sign-ipa.sh` does with codesign. If the test
bundle gets its own App ID, or a signature from a different team, the runner
installs and then crashes at launch. On this runner, isideload's
`update_bundle_id` simply refuses: "Extension … is not part of the main app
bundle identifier".

This crate therefore drives isideload's lower-level public APIs itself:
`DeveloperSession`, `AppIdsApi`, `DevicesApi`, `CertificateIdentity::retrieve`
and `sign::sign`. It does its own id rewrite, so no vendored fork or
`[patch.crates-io]` is needed. apple-codesign-quick looks up profiles and
entitlements by bundle id and only falls back to the main ones for the root
bundle. Because the .xctest shares the runner's id and is in neither map, it is
signed with the runner's identifier and team, an empty entitlements dict and no
embedded profile. The upstream fix is drafted in `UPSTREAM-PR.md`.

## Security

- **The password** arrives exactly once, inside the stdin `sign` line, and is
  never read from argv or env. Desktop starts the sidecar with only `PATH` and
  `HOME`. The line buffer and the parsed password are `Zeroizing<String>`, so
  they are wiped on drop. The buffer is pre-sized so reallocations don't leave
  copies behind. The type's `Debug` prints `<redacted>`, and parse errors never
  echo input. Nothing is logged, and stderr stays empty. Limit: isideload's SRP
  code and serde_json's scratch buffer for escaped strings may hold short-lived
  copies that we can't zero.
- **Apple state** is stored in the macOS Keychain under service
  `com.allternit.sideload`, one generic-password item per key:
  - `session/<sha256(lowercased apple id)>`: the Xcode app token, its expiry,
    the adsid, the GrandSlam `spd` (to refresh the token without the password)
    and the team id. It is zeroized in memory, and its `Debug` hides the
    tokens.
  - `<sha256(apple id)>/key`: the development certificate's private key
    (isideload).
  - `anisette_state`: the anisette machine identity (isideload). Keeping it is
    what lets Apple skip 2FA on later logins from this Mac.

  The password itself is never stored. Signing out means deleting these items
  (`security delete-generic-password -s com.allternit.sideload -a <key>`). A
  Desktop "forget Apple ID" action is a TODO.
- **Bundle input**: the .ipa files are verified by Desktop before we see them.
  Unpacking refuses entries that escape the work folder (zip `enclosed_name`).
- **Off macOS** the crate compiles (cfg-gated Keychain), but state lives only
  in memory. Every run would then log in and create a new certificate, so do
  not ship it on Windows or Linux until a real store is added there.

## Free-account limits we design around

- **10 new App IDs per 7 days.** Ids are stable per team:
  `com.allternit.phonelink.<teamid lowercased>.ictrl[.uitests.xctrunner]`.
  This is the same rule as `ictrl-fork/Scripts/setup-signing.sh`. Re-signs
  reuse the same two App IDs and spend nothing. The first link spends two.
- **Profiles expire after 7 days.** `expires_at` is returned so Desktop
  re-signs before then, which with a saved session needs no password or 2FA.
- **Development certificates.** One certificate per machine name and key is
  reused. On the limit we return `certificate_limit` and **never** revoke,
  because revoking breaks the user's other sideloaded apps (AltStore,
  SideStore).
- **Registered devices.** If the device limit is hit, the result is
  `device_limit`.
- **Rate limits.** isideload retries HTTP 429 from Apple up to 10 times.

## Anisette

Apple's login needs "anisette" headers, which are normally produced by Apple's
own software on a Mac or iPhone.

- **On a Mac (first choice):** `src/mac_anisette.rs` asks the Mac's own
  AuthKit, as Xcode and AltServer do (`AOSUtilities
  retrieveOTPHeadersForDSID:` and `AKDevice currentDevice`). No server and no
  stored machine state. isideload has no public way to build `AnisetteData`,
  so `vendor/isideload` is 0.4.6 with one added constructor (`vendor/README.md`).
  On 2026-10-18 Apple answered `-80045` ("This action cannot be completed at
  this time") to a made-up Apple ID through every emulated-device server we
  tried (anisette.allternit.com, ani.sidestore.app) and `-20970` (wrong
  account details, so the headers were accepted) through the Mac's own.
  `-80045` is classified as `anisette_unavailable`, not `bad_credentials`.
- **Fallback:** when AuthKit gives nothing, a remote **anisette v3** server.

- **Default here:** `https://ani.jkcoxson.com`, the constant
  `DEFAULT_ANISETTE_URL` in `src/main.rs`. On macOS 27 AuthKit gives no
  headers (AOSKit logs `Info request failed: -45070`), so this server does the
  work. Checked 2026-10-30 with a made-up Apple ID: Apple answered `-20970`
  (headers accepted) through this server and refused
  `anisette.allternit.com`. The server sees device headers, not the Apple ID
  or password. `anisette.allternit.com` still runs on allternit-standby.
- **Override order:** the request's `anisette_url`, then `--anisette-url URL`,
  then `$ALLTERNIT_ANISETTE_URL`, then the default. Desktop passes
  `ALLTERNIT_ANISETTE_URL` from its own environment through the request,
  because the sidecar's environment is stripped. Only `https://` URLs are
  accepted, except `http://` to 127.0.0.1 or localhost.
- **Third-party fallback:** isideload's default is `https://ani.jkcoxson.com`
  (`isideload::anisette::remote_v3::DEFAULT_ANISETTE_V3_URL`). Other public
  servers are listed in SideStore's
  [anisette-servers](https://github.com/SideStore/anisette-servers). They work,
  but every user's login metadata would then pass through someone else's
  server. That is a privacy and availability dependency we shouldn't ship
  silently. If we use one at all, it should be an explicit choice by the user.

Candidate open-source servers to self-host. Licences are as reported by GitHub
on 2026-10-16:

| project | what | licence |
|---|---|---|
| [Dadoum/anisette-v3-server](https://github.com/Dadoum/anisette-v3-server) | D server, anisette v1 + v3, Docker image `dadoum/anisette-v3-server`, port 6969 | **no licence file** (all rights reserved by default); depends on Provision |
| [Dadoum/Provision](https://github.com/Dadoum/Provision) | the ADI library + v1 server underneath | LGPL-2.1 (`COPYING-LGPLv2`) |
| [SideStore/omnisette-server](https://github.com/SideStore/omnisette-server) | Rust HTTP wrapper around omnisette, v1 + v3 | **no licence declared** |
| [SideStore/apple-private-apis](https://github.com/SideStore/apple-private-apis) (`omnisette` crate) | Rust anisette providers incl. remote v3 client | MPL-2.0 |
| [SideStore/anisette-servers](https://github.com/SideStore/anisette-servers) | list of public servers | AGPL-3.0 (the list) |

These servers run Apple's own Android ADI libraries (`libCoreADI.so` and
`libstoreservicescore.so`, downloaded from Apple's Apple Music APK) inside an
emulation layer. Hosting one means running Apple's proprietary binaries on our
servers. Before we host, the decision needs: legal review of that, a
licence-clean choice (anisette-v3-server and omnisette-server declare no
licence, so ask the authors or pick Provision's LGPL server), per-user rate
limiting, and no logging of request bodies.

## Open risks

- **Anisette hosting:** not decided. See above.
- **Apple Terms of Service.** Signing for your own device with a free account
  is what Xcode does. Doing it with a third-party client that impersonates
  Xcode (`clientId XABBG36SBA`) is something Apple tolerates (AltStore,
  SideStore and Sideloadly all do it) but does not sanction. Apple can change
  the protocol or block it at any time.
- **Licences of the build.** isideload is MIT, but it pulls in
  `apple-codesign-quick`, which is **LGPL-2.1-or-later**, and that ends up
  statically linked in this binary. Shipping it in Desktop needs either
  dynamic linking or LGPL compliance (offering the object files or source for
  relinking). Get a licence review before this goes into a release.
- **Error-code coverage.** The Apple developer-services codes for the App ID
  limit (9120) and the certificate limit (7460) are the ones AltServer,
  SideStore and isideload use. The device limit is matched on message text.
  Real accounts may produce variants. These fall back to `signing_failed`
  with a detail line.
- **Keychain prompts.** Items belong to the sidecar's code signature. A
  Desktop update signed with the same Developer ID keeps access. A
  differently signed build, such as an unsigned local build, makes macOS
  prompt.
