# Allternit iOS

Native iOS workspace for the Allternit autonomous-agent platform.

## Requirements

- macOS 14.6+
- Xcode 16.2+ (Swift 6.0)
- iOS 17.0+ deployment target
- [XcodeGen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`)

## Build

The `.xcodeproj` is generated from `project.yml`:

```bash
cd surfaces/allternit-mobile/ios
xcodegen generate
xcodebuild -scheme Allternit -destination 'platform=iOS Simulator,name=iPhone 16' build
```

The first time you build, copy the secrets template and add your Clerk publishable key:

```bash
cp Config/Secrets.xcconfig.example Config/Secrets.xcconfig
# Edit Config/Secrets.xcconfig and set CLERK_PUBLISHABLE_KEY
```

`Config/Secrets.xcconfig` is gitignored so the key is never committed.

The vendored binaries `Frameworks/Mesh.xcframework`, `Frameworks/libgit2.xcframework` and `Frameworks/SherpaOnnxC.xcframework` aren't in git either; their build scripts are listed in the repo `.gitignore`. In a new worktree, symlink them and copy `Secrets.xcconfig` from a checkout that already has them. Without them, `xcodebuild` fails with "There is no XCFramework found":

```bash
SRC=<checkout that builds>/surfaces/allternit-mobile/ios
for f in Mesh libgit2 SherpaOnnxC; do ln -s "$SRC/Frameworks/$f.xcframework" Frameworks/; done
cp "$SRC/Config/Secrets.xcconfig" Config/
```

DEBUG builds also expose a **"Continue without signing in"** button on the login gate when the Clerk key is missing, so you can open the workspace UI without a configured Clerk app.

To run tests:

```bash
xcodebuild -scheme Allternit -destination 'platform=iOS Simulator,name=iPhone 16' test
```

## Code intelligence (SourceKit-LSP)

SourceKit-LSP (used by Claude Code's `swift-lsp` plugin, VS Code and other editors) can't read an Xcode project by itself. Without help it opens each `.swift` file alone, as a macOS file: `No such module 'UIKit'`, and go-to-definition across files finds nothing. `xcode-build-server` connects it to the project:

```bash
brew install xcode-build-server
cd surfaces/allternit-mobile/ios
xcode-build-server config -project Allternit.xcodeproj -scheme Allternit   # writes buildServer.json
xcodebuild build -project Allternit.xcodeproj -scheme Allternit \
  -destination 'generic/platform=iOS Simulator' CODE_SIGNING_ALLOWED=NO     # gives it compile flags
```

`buildServer.json` holds absolute paths for one checkout, so it is gitignored. Run the steps again in each worktree, since every worktree path gets its own DerivedData folder. Build once more after you add files.

## Project structure

```
ios/
├── App/                 # App entry point, delegate, lifecycle
├── Core/                # Stores, API clients, models, design system
│   ├── LiveActivity/    # Shared ActivityAttributes for Dynamic Island
│   ├── API/Models/      # REST/SSE data models
│   └── ...
├── Features/            # SwiftUI surfaces (Chat, Agents, Code, Settings, ...)
├── AllternitWidgets/    # Live Activity widget extension
├── Tests/               # Unit tests
├── maestro/             # End-to-end Maestro flows
├── Assets.xcassets      # App icons, colors, images
└── project.yml          # XcodeGen project definition
```

## Dynamic Island / Live Activities

The app exposes two local (non-push) Live Activities:

- **Loop Live Activity** — shows iteration progress for a running `Loop`.
- **Bot Live Activity** — summarizes the dominant operational status across all subscribed bots.

Both are implemented with ActivityKit and rendered in the `AllternitWidgets` extension. The main app targets start/update/end activities via:

- `LoopLiveActivityManager` (driven by `LoopStore`)
- `BotLiveActivityManager` (driven by `BotStatusStore`)

Live Activities require `NSSupportsLiveActivities` in `Info.plist` (already set).

## Configuration

Build-time configuration lives in `project.yml`:

- `CLERK_PUBLISHABLE_KEY` — Clerk public key (client-safe by design).
- `ALLTERNIT_API_BASE_URL` — backend base URL.
- `ALLTERNIT_GIZZI_CODE_URL` — gizzi-code pty/loop server URL.
- `ALLTERNIT_MESH_CONTROL_URL` — Headscale control plane URL.

Debug defaults point at local dev servers (`127.0.0.1`). Release defaults point at the hosted Allternit infrastructure.

## Contributing

See [`CONTRIBUTING.md`](./CONTRIBUTING.md).

## Rating prompt

The app may show Apple's rating sheet (`SKStoreReviewController`) after a chat answer. Rules:

- At least 5 successful chat answers on at least 2 different days.
- Shown about 2 seconds after an answer finishes, never during a streaming answer.
- At most once per app version, and not within 120 days of the last ask.
- Never in debug builds, with the `-sample-workspace` launch argument, or under UI tests.
- Apple decides whether the sheet shows (at most 3 times a year).

Code: `Core/ReviewPromptStore.swift` holds the counters and the rules, wired in `Features/Chat/ViewModels/ChatViewModel.swift` and `App/AllternitApp.swift`. Counters are in `UserDefaults` on the device; nothing is sent to Allternit. Settings also has a **Rate Allternit** row that opens `https://apps.apple.com/app/id6828841999?action=write-review`.

To reset while testing, delete the app and reinstall it.

## TestFlight release

Build from current `origin/main`, including the intended native route changes.
An update to the hosted facility does not change the Office route compiled into
an older installed iPhone app. Build 13 uses the legacy bot floor; the next
native release must include platform PR #2035 to open the full facility.

A fresh worktree from `origin/main` cannot archive yet. Git does not track
these files, so add them first:

- `Frameworks/Mesh.xcframework`: copy it from another checkout. No fetch
  script exists for it.
- `Frameworks/libgit2.xcframework`: run `Frameworks/fetch-libgit2.sh`, or copy
  it.
- `Frameworks/SherpaOnnxC.xcframework`: run `Frameworks/fetch-sherpa-onnx.sh`,
  or copy it.
- `Config/Secrets.xcconfig`: start from `Secrets.xcconfig.example`.

If they are missing, the archive fails with "There is no XCFramework found",
or XcodeGen reports an invalid config file path.

Keep `CURRENT_PROJECT_VERSION` in sync for the app, widgets, broadcast extension
and tests in `project.yml`, then regenerate the project with XcodeGen. Run the
signed archive through `scripts/build-queue run ios --repo <checkout>`. After
archive validation, export and upload that completed archive separately.
Use the established local signing configuration and an export options plist
with `method=app-store-connect` and `destination=upload`. Do not commit signing
configuration, credentials, frameworks or archive output.

Treat archive success, upload success and TestFlight availability as separate
states. Verify the archive's bundle version, included Office source, all
extension versions and signing before uploading. Keep the exact source commit
and Apple's upload result in the release record. Check processing before saying
the build is available to testers. A TestFlight upload does not authorize a
public App Store release.
