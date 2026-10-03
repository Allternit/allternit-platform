# Changelog

All notable changes to Allternit Desktop are documented here.
Releases are tagged `desktop-v<version>`.

## [Unreleased]

(nothing yet)

## [1.1.4] — 2026-10-03

**Voice, owned end to end.**

- **On-device voice.** Speech-to-text and the bot's voice now run on your computer, with no cloud required. Speech-to-text uses Moonshine (Parakeet as an optional accurate pack), with Silero VAD and Smart Turn end-of-turn detection. The voice is Kokoro v1.0 (54 voices). Voice packs download on first use: 39 MB for listening, 350 MB for the voice.
- **Talk to Gizzi and your bots:** a quick voice bar in any session, or full-screen voice with the orb. You can interrupt by talking. Settings › Voice: where voice runs (Automatic / This device / Allternit Cloud), voice picker, custom voices with consent on file, and Cloud Voice minutes.
- **Dictation** runs on the new engine. On first use it says "Downloading the voice pack (39 MB)…" instead of failing.
- **Licensing:** the voice synthesis program `allternit-tts` is GPL-3.0-or-later (it links espeak-ng); its source is at https://github.com/Gizziio/allternit-tts. The rest of the voice service contains no GPL code, and every release build checks that.
- whisper.cpp has been removed.

## [1.1.3] — 2026-10-01

First public release since 1.1.1, for macOS (Apple Silicon and Intel) and
Windows. Built by release-workflow run 36918022068.

### Fixed
- **The Windows installer builds again.** Two changes made after 1.1.1 broke
  the Windows release job: the workspace UI was resolved through bash, which
  returns a path Node cannot open on Windows, and the installer filename
  pattern used a build-suffix variable that Windows cannot hold when empty.
- Windows ships without `allternit-mux`, which is Unix-only (#781).

### Changed
- The app updates `allternit-api`, `gizzi-code` and the workspace screens
  from signed runtime packages, without a full app update (#1137).

## [1.1.1] — 2026-09-08

### Fixed
- **Native session pickup works out of the box.** Desktop 1.1.0 shipped the
  "Continue a CLI session" picker, but its bundled `allternit-api` and
  `gizzi-code` binaries were built ~1 hour before the native-sessions feature
  merged (2026-09-06 20:21 vs 21:34), so every `/api/v1/native-sessions/*`
  call fell through to the SPA HTML and the picker showed a JSON parse error.
  This release bundles current builds of both binaries — verified end-to-end:
  catalog, pickup, and origin banner all work in the packaged app.

## [1.1.0] — 2026-09-03

### Hardening (production-readiness audit, desktop track)

- CI honesty: desktop typecheck (`npm run typecheck` over main + preload) is a
  hard gate in `ci-desktop.yml` — failures fail the job.
- Notarization: `scripts/notarize.cjs` fails loudly (non-zero exit) when
  `APPLE_ID` / `APPLE_ID_PASSWORD` / `APPLE_TEAM_ID` are missing on CI builds,
  instead of silently shipping an unnotarized app. Local unsigned builds still
  skip with a warning.
- Release pipeline: build jobs (macOS, Windows, new Linux) no longer publish —
  they run electron-builder with `--publish never` and hold no
  `contents: write` permission or `GH_TOKEN`. Exactly one job
  (`release` → `softprops/action-gh-release`) publishes, after collecting
  artifacts from all build jobs. Eliminates the double-publish race between
  electron-builder's GitHub provider and the release job.
- Release pipeline: new `build-linux` job (ubuntu) builds the Linux unpacked
  dir target (`electron-builder --linux --dir`) as a smoke test for the Linux
  configuration. Artifacts are uploaded for inspection but not attached to the
  GitHub release.
- Version bumped to 1.1.0.

### Recent work covered by this release

- Hermes HUD port refinements and crash fixes (annotation overlay, system
  theming, drag handle, HUD IPC handlers, panel visibility smoke tests).
- Native macOS dictation helper packaged with the app.
- Mesh-node (tailnet) sidecar staged into packaged resources for macOS,
  Windows, and Linux builds.
- Clerk auth aligned to the production `clerk.allternit.com` instance.
- Desktop packaging pipeline hardening (DMG/installer/auto-update) and
  packaged-resources verification.
