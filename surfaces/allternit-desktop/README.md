# Allternit Desktop

Self-hosted AI platform, fully bundled into one desktop app (~850MB): the Electron
shell ships with the web UI, the Rust `allternit-api` server, gizzi-code, and the
supporting services, so it runs the whole stack locally out of the box. You can
also point it at your own Allternit instance running on a VPS.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Allternit Desktop (This App)                       │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  • Electron UI + bundled web build (~850MB total)        │   │
│  │  • Rust API server (allternit-api, spawned locally)      │   │
│  │  • gizzi-code brain, allternit-mux, voice service       │   │
│  │  • vendored ripgrep, Lume, platform static export       │   │
│  │  • Auto-updater (update-electron-app / Squirrel)        │   │
│  │  • System tray                                          │   │
│  └─────────────────────────────────────────────────────────┘   │
│         │ HTTPS/WebSocket (local: 127.0.0.1:8013 by default)     │
│         ▼                                                        │
└─────────────────────────────────────────────────────────────────┘
   Bundled stack runs locally by default; optionally connect to:
                      │
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│         Your Own Allternit Backend (Self-Hosted, Optional)             │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  • Allternit Platform (Vite + React SPA)                │   │
│  │  • API Server (Rust, allternit-api)                     │   │
│  │  • Kernel Service                                       │   │
│  │  • All other services                                   │   │
│  │  • SQLite/PostgreSQL database                           │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                  │
│  Runs on:                                                        │
│  • Your VPS (cloud)                                             │
│  • Your local machine                                           │
│  • Docker container                                             │
└─────────────────────────────────────────────────────────────────┘
```

## Installation

### Download

Releases live on the public repo `github.com/Allternit/desktop` — the same
feed the in-app auto-updater polls (hourly). The current release is **v1.1.1**
(`desktop-v1.1.1` in this repo's history; the publish checklist for mirroring
it to `Allternit/desktop` is in `docs/DISTRIBUTION-CHECKLIST.md`). Install:

```bash
curl -fsSL https://install.allternit.com/install | bash
```

or download the DMG from `https://github.com/Allternit/desktop/releases/latest`.

Release builds are produced by `.github/workflows/release-desktop.yml`
(tag `desktop-v*`) — see **Development** below. CI artifacts are currently
**unsigned/unnotarized** (no Apple Developer ID cert, no Windows cert — see
`docs/SIGNING.md`), so macOS Gatekeeper requires right-click → Open on first
launch, and the Windows `.exe` will show SmartScreen warnings.

## Setup

### First Launch

1. **Install Allternit Backend** (if you haven't):
   ```bash
   # On your VPS or local machine
   curl -fsSL https://allternit.com/install-backend.sh | bash
   ```

2. **Open Allternit Desktop**

3. **Configure Connection**:
   - **Local Mode**: Connect to `localhost:8013` (the Rust `allternit-api` binary; it proxies to Gizzi on 4096)
   - **VPS Mode**: Connect to your VPS URL (e.g., `https://allternit.yourdomain.com`)

4. **Click "Connect"**

## Connection Modes

### Local Mode
Run the full Allternit stack on your local machine:

```bash
# Start all services
./dev/scripts/start-all-services.sh

# Or use the simplified start
allternit start
```

Then in Allternit Desktop, select **Local** mode with port `8013`.

### VPS Mode
Host Allternit on your own VPS:

```bash
# On your VPS
ssh user@your-vps
curl -fsSL https://allternit.com/install-backend.sh | bash
# Configure HTTPS with your domain
```

Then in Allternit Desktop, select **VPS** mode and enter your URL.

## Updating

### Desktop App (UI)
Auto-updates use `update-electron-app` (Squirrel) against the GitHub Releases
feed of `Allternit/desktop` — explicit in `src/main/unified-main.ts`
(`updateElectronApp({ repo: 'Allternit/desktop', updateInterval: '1 hour' })`)
and in `build.publish` in `package.json`. The app checks the latest release
on that repo about once an hour and prompts via the renderer when a new
version is available.

### Backend (Your Server)
You control when to update your backend:

```bash
# SSH to your VPS
ssh user@your-vps

# Update Allternit backend
cd ~/allternit
git pull
# Rebuild and restart — cloud API deploy loop: docs/Operations/CLOUD_API_VPS_DEPLOY.md
# (scripts/deploy-cloud-api.sh, or systemd restart for self-hosted allternit-api)

# Or with Docker
docker pull allternit/backend:latest
docker-compose up -d
```

## Configuration

Settings stored in:
- **macOS**: `~/Library/Application Support/Allternit Desktop/`
- **Windows**: `%APPDATA%/Allternit Desktop/`
- **Linux**: `~/.config/Allternit Desktop/`

### Environment Variables

| Variable | Description |
|----------|-------------|
| `ALLTERNIT_DESKTOP_LOG_LEVEL` | Log level (debug, info, warn, error) |

## Troubleshooting

### "Cannot connect to backend"

1. Check if backend is running:
   ```bash
   curl http://localhost:8013/health
   ```

2. Check firewall/VPS security groups

3. Verify HTTPS certificate (for VPS mode)

### Reset Connection

Delete the config file to reset:
```bash
# macOS
rm ~/Library/Application\ Support/Allternit\ Desktop/config.json

# Windows
rmdir /s "%APPDATA%\Allternit Desktop"

# Linux
rm -rf ~/.config/Allternit\ Desktop
```

## Development

```bash
cd surfaces/allternit-desktop

# Install dependencies
npm install

# Run in dev mode
npm run dev

# Build main/preload/renderer TypeScript
npm run build

# Stage the Rust API binary after `cargo build --release -p allternit-api`
npm run stage:api-binary

# Verify packaged resources before packaging
npm run verify:packaged-resources

# Build full release artifacts (requires signing certs for signed builds)
npm run dist
```

Full pipeline (platform static export, gizzi-code, allternit-mux, ripgrep,
voice service, Rust API, Lume, Electron package, checksum patch) in one shot:

```bash
./scripts/build-desktop.sh          # from the repo root; --skip-platform / --skip-api / --skip-electron
```

Last verified: 2026-09-03 against a0f8230b5 (scripts/build-desktop.sh and
package.json scripts confirmed present).

See [docs/SIGNING.md](./docs/SIGNING.md) for codesigning, notarization, and
auto-update configuration. The three-platform shipping list (including work
that can happen before Apple approval) is
[docs/DISTRIBUTION-CHECKLIST.md](./docs/DISTRIBUTION-CHECKLIST.md).

## Data & Privacy

- **Desktop app**: Only stores connection settings locally
- **Your data**: Stored on YOUR backend (VPS or local)
- **No telemetry**: We don't track usage
- **No central servers**: You control everything

## Support

- **Documentation**: https://docs.allternit.com/desktop
- **Backend Setup**: https://docs.allternit.com/self-host
- **Issues**: no public issue tracker yet (the old `github.com/allternit/desktop` repo does not exist)

## License

UNLICENSED (as declared in `package.json`; no OSI license is granted).
