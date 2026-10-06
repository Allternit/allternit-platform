# Desktop Build Guide

## Prerequisites

- Node.js 20+
- npm or pnpm
- For Mac: Xcode Command Line Tools (for native modules)
- For Windows: Windows Build Tools

## Build Steps

### Step 1: Build Platform Static Export (Vite)

The desktop app loads the hosted workspace UI by default; a Vite static
export of private `Allternit/allternit-ai` is bundled as the offline fallback
and served by the Rust API (`cmd/allternit-api/src/platform_static.rs`).
Do not package `surfaces/platform.allternit.com` — that is the cloud console.

```bash
cd surfaces/allternit-desktop

# Builds Allternit/allternit-ai (sibling, ALLTERNIT_AI_PATH, or .hosted-ui) → resources/platform/
npm run prepare:platform-static
```

> Note: the old `../allternit-platform` directory (Next.js, `.next/standalone`)
> no longer exists. All platform-static staging goes through
> `scripts/prepare-platform-static.cjs`.

### Step 2: (obsolete — removed)

Steps that copied `.next/standalone` into `resources/platform-server/` and
downloaded a `cloudflared` binary were removed 2026-09: the platform is a
Vite app and `cloudflared` is no longer referenced by the desktop main
process or packaging scripts.

### Step 2b: Stage mesh-node sidecar

mesh-node is the tsnet userspace sidecar (`infrastructure/mesh/tsnet-ios/cmd/mesh-node`) that joins the Allternit tailnet without a system VPN — it's what lets the desktop app reach gizzi instances at `100.64.0.0/10` addresses. The packaged app resolves it from `resources/bin/mesh-node[.exe]` (see `src/main/mesh-manager.ts`).

Stage it for your current platform:

```bash
npm run prepare:mesh-node
```

The script (`scripts/prepare-mesh-node.cjs`) acquires the binary in this order:

1. **Vendor copy** — copies from the repo vendor tree `cmd/gizzi-code/vendor/mesh-node/<platform>-<arch>/` (the same binary gizzi-code's `mesh.ts` uses) if present.
2. **Build from source** — runs `infrastructure/mesh/tsnet-ios/build-sidecar.sh` (darwin-arm64, linux-x64), or an equivalent `go build` for win32-x64, when Go is on PATH.
3. **Release download** — downloads the latest `gizzi-code/*` GitHub release asset (v0.2.2+ archives ship `mesh-node` next to `gizzi-code`) and extracts just the sidecar.

The script is idempotent (skips when `resources/bin/mesh-node` already exists) and is already wired into `build:electron`, `build:electron:dmg`, `pack`, and `dist`. The staged binary is gitignored like the rest of `resources/bin/`.

# Step 2c: Stage allternit-api Rust binary

The desktop shell embeds the Rust API backend at `resources/bin/allternit-api[.exe]`.
If you have already built it (`cargo build --release -p allternit-api`), copy it in:

```bash
npm run prepare:api-binary
```

This is also called automatically by `npm run build:electron`, `npm run dist`,
etc. In practice the binary **must be built locally and staged** — the script
(`scripts/prepare-api-binary.cjs`) only copies from a local cargo build
(`target/release` or `target/debug` at the repo root). Its manifest-download
fallback (`src/main/manifest.ts`) points at `github.com/allternit/platform`
release assets that do not exist yet (and most manifest checksums are empty
placeholders), so the download path will fail:

```bash
cargo build --release -p allternit-api   # must exist first
npm run prepare:api-binary               # copies it into resources/bin/
```

### Step 3: Build Desktop App

```bash
# Install desktop dependencies
npm install

# Build TypeScript
npm run build

# Build Electron app (macOS)
npm run build:electron:dmg

# Or build for all platforms
npm run dist
```

Local builds are stamped with a build number so consecutive builds of the
same version are distinguishable: the `build:electron*`, `pack`, and `dist`
scripts run through `scripts/build-local.cjs`, which sets
`ALLTERNIT_BUILD_SUFFIX=-b<git rev-list count>` and electron-builder expands
it into the artifact name (e.g. `Allternit-Desktop-1.1.1-b4177-arm64.dmg`).
The wrapper also passes `-c.buildVersion=<version>.<height>` (CFBundleVersion /
Windows FileVersion) and writes `resources/build-info.json`, which is packed
next to the sidecars and surfaced in-app: Settings → About shows
"Allternit Desktop 1.1.1-b4177 · build 1.1.1.4177" via `app:get-info`.
CI/release builds keep clean version-only names and report no buildInfo (the
wrapper skips CI and the workflow pins the suffix to empty; the
`resources/build-info.json` extraResource is simply absent there). Override
with `ALLTERNIT_BUILD_SUFFIX="-custom"` (or `=""` for clean names locally)
and `ALLTERNIT_BUILD_VERSION` for the buildVersion string.

### Step 4: Verify Build

Check the output:

```bash
ls -la release/
# Should see:
# - Allternit-1.0.0.dmg (Mac)
# - Allternit-1.0.0.exe (Windows)
# - Allternit-1.0.0.AppImage (Linux)
```

## Automated Build Script

Use the codified pipeline at the repo root — it runs every step above plus the
gizzi-code binary, allternit-mux, vendored ripgrep, the voice-service sidecar,
and Lume, then patches SHA256 checksums into `src/main/manifest.ts`:

```bash
./scripts/build-desktop.sh [--skip-platform] [--skip-api] [--skip-electron]
```

Last verified: 2026-09-03 against a0f8230b5 (`scripts/build-desktop.sh`,
`scripts/prepare-platform-static.cjs`, and all `prepare:*` package.json
scripts confirmed present; `surfaces/allternit-platform` confirmed absent).

## Code Signing & Notarization

The packaging config contains explicit placeholders for platform signing. Set the
following environment variables before running `npm run dist` to produce signed
release artifacts.

### macOS

| Variable | Purpose |
|----------|---------|
| `APPLE_ID` | Apple developer account email |
| `APPLE_ID_PASSWORD` | App-specific password (not your Apple ID password) |
| `APPLE_TEAM_ID` | 10-character Apple Developer Team ID |

`electron-builder` will pick the first valid Developer ID identity automatically
(`build.mac.identity: null`). The `scripts/notarize.cjs` afterSign hook skips
with a warning when the env vars are absent on local builds, and hard-fails on
CI when they are missing, so unsigned release builds cannot pass silently.

### Windows

| Variable | Purpose |
|----------|---------|
| `CSC_LINK` | URL or path to the PKCS#12 certificate file |
| `CSC_KEY_PASSWORD` | Password for the certificate |
| `WIN_CSC_LINK` | Windows-only override for `CSC_LINK` |
| `WIN_CSC_KEY_PASSWORD` | Windows-only override for `CSC_KEY_PASSWORD` |

Set `build.win.certificateFile` / `certificatePassword` to a real value when using
a checked-in certificate. Leaving them as `null` lets `electron-builder` fall back
to the standard `CSC_*` environment variables.

### Linux

AppImage and `.deb` packages are not code-signed. Ensure the package metadata in
`build.linux`, `build.appImage`, and `build.deb` is updated before publishing.

## Distribution

After building, upload to install.gizziio.com:

```bash
# Copy to web server
scp release/Allternit-1.0.0.dmg user@install.gizziio.com:/var/www/install/
scp release/Allternit-1.0.0.exe user@install.gizziio.com:/var/www/install/
scp release/Allternit-1.0.0.AppImage user@install.gizziio.com:/var/www/install/
```

Update download links in:
- `allternit-websites/projects/gizziio/source/index.html`
- `www.allternit.com` download page
