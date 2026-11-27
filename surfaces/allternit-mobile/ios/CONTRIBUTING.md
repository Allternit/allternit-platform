# Contributing to Allternit iOS

Thanks for helping out. This document covers the basics of getting the iOS app running and submitting changes.

## Prerequisites

1. Clone the repo and open `surfaces/allternit-mobile/ios`.
2. Install XcodeGen:
   ```bash
   brew install xcodegen
   ```
3. Install dependencies and generate the Xcode project:
   ```bash
   xcodegen generate
   ```
4. Copy the secrets template (it holds the production Clerk publishable key, which is public):
   ```bash
   cp Config/Secrets.xcconfig.example Config/Secrets.xcconfig
   ```

## Running the app

1. Open `Allternit.xcodeproj` in Xcode or build from the command line:
   ```bash
   xcodebuild -scheme Allternit -destination 'platform=iOS Simulator,name=iPhone 16' build
   ```
2. The app supports a `-skip-auth` launch argument for local UI development (DEBUG builds only).
3. For full functionality you need local backend services:
   - `allternit-api` on `127.0.0.1:8013`
   - `gizzi-code` pty/loop server on `127.0.0.1:4096`

## Making changes

- **Project changes**: edit `project.yml`, then run `xcodegen generate`. Do not hand-edit `.pbxproj` files.
- **New source files**: place them in the appropriate `App/`, `Core/`, `Features/`, or `AllternitWidgets/` directory, then regenerate the project.
- **Live Activities**: shared `ActivityAttributes` types go in `Core/LiveActivity/`. Widget rendering goes in `AllternitWidgets/`.
- **Tests**: add XCTest files to `Tests/`. The `AllternitTests` target is configured to `@testable import Allternit`.

## Code style

- Swift 6.0, strict concurrency enabled.
- Prefer `@MainActor` for ObservableObject stores.
- Keep views decoupled from stores via `@EnvironmentObject` or injected dependencies.
- Match the existing file comment conventions for cross-surface references.

## Testing

Run unit tests before opening a PR:

```bash
xcodebuild -scheme Allternit -destination 'platform=iOS Simulator,name=iPhone 16' test
```

For UI smoke tests we use Maestro:

```bash
# shared iOS + Android flows live in ../maestro/audit
../maestro/audit/run-audit.sh --platform ios ../maestro/audit/01_login_gate.yaml
```

## Secrets and credentials

- Do not commit API keys, tokens, or private certificates.
- `CLERK_PUBLISHABLE_KEY` is injected through `Config/Secrets.xcconfig` (gitignored). Copy from `Config/Secrets.xcconfig.example` (production publishable key; change it only for your own Clerk instance) and never commit the resulting file.
- Runtime device tokens and Clerk session tokens are stored in the iOS Keychain and are never logged.

## License

The repository root should contain a `LICENSE` file before public release. If you are forking or redistributing the iOS project, use the license declared at the repository root. If no root license exists yet, ask a maintainer before assuming terms.

## Opening a pull request

1. Ensure the app builds and tests pass locally.
2. Keep the diff focused on one concern.
3. Update this README or `project.yml` docs if your change affects build/configuration steps.
4. If you add a new Live Activity or widget, include a screenshot or screen recording in the PR description.
