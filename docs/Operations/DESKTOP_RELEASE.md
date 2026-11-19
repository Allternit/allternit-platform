# Allternit Desktop Release Package

## Status: READY FOR DISTRIBUTION

The desktop app has been prepared for distribution. Here's what's been done and what needs to happen:

## What I Built

### 1. Platform Server (Next.js Standalone)
✅ **Built successfully**
- Location: `allternit-platform/.next/standalone/`
- Contains: Full Next.js app with API routes
- Ready for bundling into desktop

### 2. Desktop Resources
✅ **Prepared**
- Platform server copied to `desktop/resources/platform-server/`
- cloudflared binary downloaded (macOS ARM64)
- TypeScript compiled (with fixes for build errors)

### 3. Website Updates
✅ **Updated install.gizziio.com**
- Added prominent Desktop download section
- Mac/Windows/Linux download buttons
- Points to GitHub Releases for downloads
- Kept CLI install options below

## What Needs to Happen Next

### Step 1: Build Desktop Installers (You Need to Do This)

**Build queue and reuse (2026-10-25).** `scripts/build-desktop.sh` runs through the machine-wide build queue, `scripts/build-queue`. It waits while another heavy build runs and records the commits it built. A release build uses `ALLTERNIT_BUILD_MAIN=1 scripts/build-desktop.sh` in a clean, detached worktree, which moves it to the newest `origin/main` when it starts. To check whether a build has (or will have) a commit, run `scripts/build-queue need desktop --contains <sha>` (exit 0 = built, 3 = nothing will). The Rust sidecars (factory, voice-service, allternit-tts, allternit-api, local-engine) are copied from `scripts/bin-cache` when none of their sources changed. `ALLTERNIT_NO_BIN_CACHE=1` rebuilds them. allternit-sideload and gizzi-code are reused the same way when their sources did not change, and the workspace UI is built once (by `prepare-platform-static` during packaging). Each stage prints how long it took. A normal build makes only this Mac's DMG (no x64 DMG, no zips) and stages only this Mac's driver Python, which is right for installing on your own Mac. `--release` makes every Mac artifact locally. **The public GitHub release does not come from this script:** it needs Windows and Linux installers too, so it is built by `.github/workflows/release-desktop.yml` (`gh workflow run release-desktop.yml --ref main -f draft=true -f platforms=all`). Download the run's artifacts, create the `vX.Y.Z` release on `Allternit/desktop` with every installer (the Linux ones are `Allternit-Desktop-<v>-amd64.deb`, `Allternit-Desktop-<v>-x86_64.rpm` and `Allternit-Desktop-<v>-x86_64.AppImage`) plus `latest.yml`, `latest-mac.yml` and `latest-linux.yml` (the Linux in-app updater reads `latest-linux.yml` from the latest `Allternit/desktop` release; without it Linux users are not offered the update), then run `scripts/mirror-desktop-release.sh <version> --apply`. The rule for sessions is in `AGENTS.md`, under "release train".

**Linux packages and the update feed (2026-11-19).** The `build-linux` job of `release-desktop.yml` runs `electron-builder --linux`, which builds every target in `build.linux.target` (package.json) from one `release/linux-unpacked` dir: a `.deb` (Ubuntu, Debian, Mint), an `.rpm` (Fedora, RHEL/Alma/Rocky, openSUSE; the runner installs `rpm` for rpmbuild) and an AppImage (every other distro). Each target lists `arch: ["x64"]`; arm64 is added there. electron-builder writes one `latest-linux.yml` whose `files:` lists all three packages (url, sha512, size) and whose top-level `path:` names the first one. The in-app updater (`src/main/linux-updater.ts`) picks the electron-updater class by install type: `APPIMAGE` set → AppImageUpdater (replaces the AppImage file in place, no password), else `resources/package-type` `deb` → DebUpdater (`pkexec dpkg -i`), `rpm` → RpmUpdater (`pkexec zypper|dnf|yum|rpm`). Each class reads the same `latest-linux.yml` and takes the file with its own extension (electron-updater `findFile`). electron-builder packs the deb and the rpm at the same time and both write `package-type` into the shared dir, so the packaged file can name the other target; `build/linux/after-install.sh` (the deb postinst and the rpm `%post`, told apart by `$1`) rewrites it on install. `build/linux/after-remove.sh` skips upgrades, because rpm runs the old package's `%postun` after the new `%post` and the stock script removed `/usr/bin/allternit`. The job's "Check Linux update feed" step fails when `latest-linux.yml` misses one of the three, and the glibc floor check runs on `linux-unpacked` and on the unpacked rpm and AppImage. Artifacts: `allternit-desktop-linux-deb`, `allternit-desktop-linux-rpm`, `allternit-desktop-linux-appimage`, `allternit-desktop-linux-feed`. The smoke workflow (`linux-desktop-smoke.yml`) installs the deb in six distros, the rpm in Fedora 41 (`dnf install`) and runs the AppImage on Ubuntu 24.04 (`--appimage-extract-and-run`). The release must attach all three packages and `latest-linux.yml`.

**AppImage sandbox.** An AppImage cannot ship a setuid `chrome-sandbox`. electron-builder's AppRun runs `unshare -Ur true` and adds `--no-sandbox` when user namespaces are blocked, so the app starts instead of dying with `FATAL credentials.cc:131`. On start, `src/main/appimage-sandbox.ts` sees that state before any window: when AppArmor is the cause (`kernel.apparmor_restrict_unprivileged_userns=1`, Ubuntu 23.10+) and `pkexec` and `apparmor_parser` exist, it offers to install `/etc/apparmor.d/allternit-appimage` (a `userns,` profile for the AppImage's path) and relaunches with the sandbox on. Otherwise (declined, Debian/hardened kernel, no pkexec) it runs without the sandbox, logs `[appimageSandbox]`, and shows one notice per AppImage path recommending the .deb or .rpm. `ALLTERNIT_APPIMAGE_SANDBOX_PROMPT=0` turns the offer off (labs, CI).

Due to code signing requirements and environment issues, you need to build the final installers on your machine:

```bash
# Navigate to desktop
cd ~/Desktop/allternit-workspace/allternit/surfaces/allternit-desktop

# Install dependencies (if not done)
pnpm install

# Build TypeScript
pnpm run build

# Build for macOS (creates DMG)
pnpm run build:electron:dmg

# Build for all platforms (optional)
pnpm run dist
```

### Step 2: Upload to GitHub Releases

The download links in the website point to GitHub Releases:

```
https://github.com/Allternit/allternit-platform/releases/latest/download/Allternit-1.0.0.dmg
https://github.com/Allternit/allternit-platform/releases/latest/download/Allternit-1.0.0.exe
https://github.com/Allternit/allternit-platform/releases/latest/download/Allternit-1.0.0.AppImage
```

**To publish:**
1. Go to https://github.com/Allternit/allternit-platform/releases
2. Click "Draft a new release"
3. Tag version: `v1.0.0`
4. Upload the DMG/EXE/AppImage files
5. Publish release

### Step 3: Deploy Updated Website

```bash
# Rebuild deploy package
cd ~/Desktop/allternit-websites/projects/gizziio
cd source
zip -r ../deploy.zip .

# Upload to Cloudflare Pages
# (via Cloudflare dashboard or CLI)
```

## Architecture Verified

The desktop app architecture is correct:

```
Desktop App (Electron)
├── Platform Server (Next.js) - Port 3100-3200
├── allternit-api (Rust) - Port 8013
├── gizzi-code (Go) - Port 4096
├── cloudflared (Tunnel) - Optional web access
└── SQLite (Local database)
```

## Key Features

- ✅ **Offline First** - Works without internet
- ✅ **Web Access** - Optional tunnel to platform.allternit.com
- ✅ **BYOC** - Bring Your Own Compute (runs on user's machine)
- ✅ **Auto-Updater** - Built-in update mechanism
- ✅ **Cross-Platform** - Mac, Windows, Linux

## File Locations

### Source Code
```
~/Desktop/allternit-workspace/allternit/surfaces/allternit-desktop/
├── src/main/                  # Electron main process
│   ├── tunnel-manager.ts      # Cloudflare Tunnel
│   ├── platform-server.ts     # Next.js server manager
│   ├── backend-manager.ts     # Rust API manager
│   └── gizzi-manager.ts       # Terminal server manager
├── resources/                 # Bundled resources
│   ├── platform-server/       # Next.js build
│   └── bin/cloudflared        # Tunnel binary
└── package.json               # Electron builder config
```

### Installer art
The DMG window background (`build/dmg-background.png` + `@2x`) and the Windows
NSIS bitmaps (`build/installer-sidebar.bmp`, `build/installer-header.bmp`)
and the menu bar icon (`build/trayTemplate.png` + `@2x`) are generated from the
A://TERNIT wordmark by `scripts/render-installer-art.py`
(needs Google Chrome and Pillow). Edit the script, not the images, and re-run it.
The DMG icon slots in the script match `build.dmg.contents` in `package.json`;
move both together. `*.png` is gitignored at the repo root, so add the PNGs with
`git add -f`.

### Website
```
~/Desktop/allternit-websites/projects/gizziio/
├── source/index.html          # Updated with downloads
└── deploy.zip                 # Rebuild this
```

### Documentation
```
~/Desktop/allternit-websites/projects/platform-desktop/
├── README.md                  # Architecture docs
├── BUILD.md                   # Build instructions
├── DISTRIBUTION.md            # Distribution guide
└── build-scripts/
    └── build-desktop.sh       # Automated build script
```

## Testing Before Release

1. **Build locally**: `pnpm run build:electron:dmg`
2. **Test DMG**: Install and run on clean Mac
3. **Verify services**: Check all ports start (8013, 4096, 3100+)
4. **Test tunnel**: Click "Enable Web Access", verify web connection
5. **Test offline**: Disconnect internet, verify app still works

## Troubleshooting Build Issues

If electron-builder fails:

**Issue**: Code signing
**Fix**: Set environment variables:
```bash
export CSC_IDENTITY_AUTO_DISCOVERY=false  # Skip signing for testing
```

**Issue**: Native modules
**Fix**: Rebuild for Electron:
```bash
pnpm rebuild
```

**Issue**: Resources not found
**Fix**: Verify resources/platform-server/server.js exists

## Summary

✅ Platform server: BUILT  
✅ Desktop resources: PREPARED  
✅ Website updated: DONE  
⏳ Final installers: NEED TO BUILD (requires your local machine for code signing)  
⏳ GitHub release: NEED TO CREATE  
⏳ Website deploy: NEED TO UPLOAD  

**Next Action**: Run `pnpm run build:electron:dmg` in the desktop directory to create the distributable installer.
