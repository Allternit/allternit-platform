# Allternit Desktop Architecture

## Overview

Allternit Desktop is a **cloud-connected client** for self-hosted Allternit backends.

```
┌─────────────────────────────────────────────────────────────────┐
│                      Allternit Desktop                                │
│                    (Electron App)                               │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Connection Setup Page (static/connect.html)            │   │
│  │  - VPS/Local mode selector                              │   │
│  │  - URL/port configuration                               │   │
│  │  - Connection test                                      │   │
│  └─────────────────────────────────────────────────────────┘   │
│                          │                                       │
│                          ▼ (on connect)                           │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Platform UI (loaded from user's backend)               │   │
│  │  - Shell UI                                             │   │
│  │  - Agent Hub                                            │   │
│  │  - Projects                                             │   │
│  │  - Chat                                                 │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ HTTPS/WebSocket
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│              User's Allternit Backend (Self-Hosted)                   │
│                                                                  │
│  Runs on:                                                       │
│  • User's VPS (cloud)                                          │
│  • User's local machine                                        │
│  • Docker container                                            │
│                                                                  │
│  Contains:                                                      │
│  • Vite + React SPA workspace UI (private Allternit/allternit-ai)                                   │
│  • Rust API Services (15+ services)                           │
│  • SQLite/PostgreSQL database                                 │
│  • All AI/ML infrastructure                                   │
└─────────────────────────────────────────────────────────────────┘
```

## Key Design Decisions

### 1. Hybrid: Bundled Sidecars + User-Hosted Backend

The initial design made the desktop a pure "dumb client" with no bundled
services. The shipped app sits between the two models:

- The Electron shell **bundles sidecar binaries** under `resources/bin/`:
  `allternit-api` (Rust API, port 8013 — proxies to Gizzi on 4096),
  `gizzi-code`, `allternit-mux`, vendored `ripgrep`, the voice-service
  PyInstaller binary, `mesh-node` (tsnet sidecar), and `lume` (macOS/Linux).
  See `../BUILD.md` and `scripts/build-desktop.sh`.
- The full self-hosted backend (VPS/local) is still installed and updated
  separately by the user.
- Desktop stores only connection configuration plus these bundled sidecars.

### 2. Connection Flexibility
Users can connect to:
- **Local**: `http://localhost:8013` (Rust `allternit-api` on the same machine; it proxies to Gizzi on 4096)
- **VPS**: `https://allternit.theirdomain.com` (backend on their cloud)

### 3. Auto-Discovery
Desktop automatically scans common ports to find local Allternit instances.

### 4. Separate Update Cycles
- **Desktop UI**: Auto-updates via electron-updater
- **Backend**: User controls when to update their VPS/local instance

## File Structure

```
allternit-desktop/
├── src/
│   ├── main/
│   │   ├── unified-main.ts   # Main process with connection mgmt
│   │   ├── backend-manager.ts, gizzi-manager.ts, mesh-manager.ts, lima.ts, …
│   │   └── manifest.ts       # Platform-locked binary manifest + checksums
│   └── preload/
│       └── index.ts          # IPC bridge
├── static/
│   └── connect.html          # Connection setup UI
├── resources/
│   ├── platform/             # Vite static export (offline fallback, from ai.allternit.com)
│   └── bin/                  # Bundled sidecars: allternit-api, gizzi-code, mesh-node, …
├── scripts/
│   ├── prepare-platform-static.cjs
│   ├── prepare-mesh-node.cjs
│   ├── prepare-api-binary.cjs
│   └── verify-packaged-resources.cjs
├── package.json              # electron-builder config + build/pack scripts
└── README.md
```

Last verified: 2026-09-03 against a0f8230b5 (ports from `src/main/config.ts`;
sidecar list from `scripts/build-desktop.sh` and `package.json`).

## Flow

1. **App Launch**
   - Show connection setup page
   - Load saved config (if any)
   - Auto-discover local services

2. **User Connects**
   - Select VPS or Local mode
   - Enter URL/port
   - Click "Test" to verify
   - Click "Connect"

3. **Connection Established**
   - Load platform UI from user's backend
   - Show system tray with connection status
   - Begin normal Allternit usage

4. **Connection Lost**
   - Show error in tray
   - Offer to retry or reconfigure

## Benefits

1. **Small App Size**: ~50MB vs ~500MB bundled
2. **No Local Services**: No port conflicts, no service management
3. **User Control**: User owns their data and infrastructure
4. **Flexible**: Works with VPS or local setups
5. **Simple Updates**: UI updates independently

## Comparison

| Aspect | Bundled (Initial) | Cloud-Connected (Final) |
|--------|-------------------|------------------------|
| Size | ~500MB | ~50MB |
| Services | Bundled | User-hosted |
| Data | Local only | User controls location |
| Setup | One-click | Two-step (backend + UI) |
| Updates | Complex | Simple |
| Target | Local only | VPS or local |

## Future Enhancements

1. **Embedded Backend**: Option to bundle minimal backend for offline use
2. **Multiple Connections**: Switch between different backends
3. **Connection Profiles**: Save multiple VPS configs
4. **Offline Mode**: Cache UI for offline viewing
