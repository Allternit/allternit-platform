# Allternit Websites - Deployment Guide

Complete deployment guide for Allternit web properties and desktop distribution.

## Quick Reference

| Site | Package | Target | Status |
|------|---------|--------|--------|
| www.allternit.com | `projects/allternit/deploy.zip` (798K) | Cloudflare Pages | ✅ Ready |
| docs.allternit.com | `projects/allternit-docs/deploy.zip` (127K) | Cloudflare Pages | ✅ Ready |
| institute.allternit.com | `projects/allternit-protocol-institute/deploy.zip` (78K) | Cloudflare Pages | ✅ Ready |
| docs.gizziio.com | `projects/gizzi-code-docs/deploy.zip` (63K) | Cloudflare Pages | ✅ Ready |
| install.gizziio.com | `projects/gizziio/deploy.zip` (11K) | Cloudflare Pages | ✅ Ready |
| platform.allternit.com | `projects/platform-allternit/deploy.zip` (5.5M) | Cloudflare Pages | ✅ Ready |

## Architecture

### Production APIs (control plane / data plane)

One public origin. Browsers never call `:8013`.

```
Browser / SDK / CLI
        │ HTTPS + Clerk JWT or virtual key
        ▼
┌──────────────────────────────────────────┐
│  Cloud API — control plane               │
│  https://api.allternit.com               │
│  cmd/allternit-cloud-api (:8082)         │
│  Postgres                                │
│  pairing, relay, billing, models,        │
│  hosted runtimes, provisioned fleet      │
└──────────────────┬───────────────────────┘
                   │ outbound WebSocket relay
                   ▼
┌──────────────────────────────────────────┐
│  Allternit API — data plane              │
│  cmd/allternit-api (:8013, not public)   │
│  per-instance SQLite                     │
│  Mode 1  local desktop                   │
│  Mode 2  user-paired VPS                 │
│  Mode 3  Allternit-provisioned instance  │
└──────────────────────────────────────────┘
```

Public docs: https://docs.allternit.com/api/overview

Desktop still bundles a local Allternit API + Gizzi for mode 1:

```
┌─────────────────────────────────────────────────────────────────┐
│                    PRIMARY: Desktop App                         │
│                  (What users use daily)                         │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Allternit Desktop (Electron)                           │   │
│  │                                                         │   │
│  │  • Platform UI                      - Port 3100-3200    │   │
│  │  • allternit-api (data plane)       - Port 8013         │   │
│  │  • gizzi-code                       - Port 4096         │   │
│  │  • SQLite database                                      │   │
│  │  • agent-daemon → api.allternit.com (outbound relay)    │   │
│  └─────────────────────────────────────────────────────────┘   │
└──────────────────────────────────┬──────────────────────────────┘
                                   │
                                   │ HTTPS (Cloud API) always
                                   │ optional tunnel for LAN UI
                                   ▼
┌─────────────────────────────────────────────────────────────────┐
│              Web surfaces                                       │
│           platform.allternit.com / ai.allternit.com             │
│           (Cloudflare Pages)                                    │
│                                                                 │
│  • Call https://api.allternit.com only                          │
│  • Data-plane routes 428 until a node is paired/online          │
└─────────────────────────────────────────────────────────────────┘
```

## What Gets Deployed vs Distributed

### Deployed (Cloudflare Pages)
- **Static websites**: Marketing, docs, download landing
- **Web platform**: UI shell that connects to desktop via tunnel

### Distributed (Downloads)
- **Desktop app**: Electron app with bundled backend services
- **Hosted on**: install.gizziio.com

## Deployment Steps

### Step 1: Deploy Static Sites (5 minutes)

Upload to Cloudflare Pages:

| Project | Zip File | Domain |
|---------|----------|--------|
| allternit | `projects/allternit/deploy.zip` | www.allternit.com |
| allternit-docs | `projects/allternit-docs/deploy.zip` | docs.allternit.com |
| allternit-protocol-institute | `projects/allternit-protocol-institute/deploy.zip` | institute.allternit.com |
| gizzi-code-docs | `projects/gizzi-code-docs/deploy.zip` | docs.gizziio.com |
| gizziio | `projects/gizziio/deploy.zip` | install.gizziio.com |

For each:
1. Cloudflare Dashboard → Pages → Create project
2. Upload zip file
3. Add custom domain
4. Deploy

### Step 2: Deploy Web Platform (5 minutes)

The web platform is a static Next.js export that connects to desktop via tunnel:

1. Create Cloudflare Pages project: `platform-allternit`
2. Upload `projects/platform-allternit/deploy.zip`
3. Add custom domain: `platform.allternit.com`
4. Set environment variables:
   ```
   NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_live_xxx
   CLERK_SECRET_KEY=sk_live_xxx
   ```

### Step 3: Build & Distribute Desktop App

The desktop app is **not deployed** - it's built and distributed.

**Location**: `allternit-workspace/allternit/surfaces/allternit-desktop/`

**Build:**

```bash
cd allternit-workspace/allternit/surfaces/allternit-desktop

# Install dependencies
npm install

# Build desktop app (includes platform-server)
./scripts/build-desktop.sh

# Output:
# release/Allternit-1.0.0.dmg      (Mac)
# release/Allternit-1.0.0.exe      (Windows)
# release/Allternit-1.0.0.AppImage (Linux)
```

**Distribute:**

Upload installers to install.gizziio.com:
```bash
# Upload to web server
scp release/Allternit-1.0.0.dmg user@install.gizziio.com:/var/www/install/
scp release/Allternit-1.0.0.exe user@install.gizziio.com:/var/www/install/
scp release/Allternit-1.0.0.AppImage user@install.gizziio.com:/var/www/install/
```

Update `projects/gizziio/source/index.html` with download links.

## Desktop App Components

The desktop app bundles multiple services:

### 1. Platform Server (Next.js)
- **Source**: `surfaces/allternit-platform/`
- **Build**: `ALLTERNIT_BUILD_MODE=desktop npm run build`
- **Output**: `.next/standalone/`
- **Port**: 3100-3200 (dynamic)
- **Purpose**: Serves UI and handles API routes

### 2. allternit-api (Rust)
- **Source**: `api/` or `cmd/allternit-api/`
- **Binary**: `allternit-api`
- **Port**: 8013
- **Purpose**: Core backend services (gateway, operator, tools)

### 3. gizzi-code (Go)
- **Source**: `cmd/gizzi-code/`
- **Binary**: `gizzi-code`
- **Port**: 4096
- **Purpose**: Terminal server, agent sessions, conversations

### 4. Cloudflare Tunnel (cloudflared)
- **Binary**: `resources/bin/cloudflared`
- **Purpose**: Optional web access
- **URL**: `https://xxxxx.trycloudflare.com`

## Web-to-Desktop Connection

When user enables "Web Access" in desktop:

```
1. Desktop starts cloudflared tunnel
   → Points to Platform Server (localhost:PORT)
   
2. Gets public URL
   → https://xxxxx.trycloudflare.com
   
3. Opens browser
   → platform.allternit.com/connect?tunnelUrl=xxxxx&token=yyy
   
4. Web platform stores URL
   → localStorage.setItem('tunnelUrl', 'xxxxx...')
   
5. All API calls go to tunnel
   → fetch(`https://${tunnelUrl}/api/...`)
```

### Web Platform /connect Page

The web platform needs a `/connect` page to receive tunnel URL:

```typescript
// app/connect/page.tsx
export default function ConnectPage() {
  const searchParams = useSearchParams();
  const tunnelUrl = searchParams.get('tunnelUrl');
  const token = searchParams.get('token');
  
  useEffect(() => {
    if (tunnelUrl && token) {
      localStorage.setItem('platform_tunnel_url', tunnelUrl);
      localStorage.setItem('platform_auth_token', token);
      window.location.href = '/shell';
    }
  }, [tunnelUrl, token]);
  
  return <div>Connecting to desktop...</div>;
}
```

### API Client

Web platform uses stored tunnel URL:

```typescript
async function apiCall(endpoint: string, data: any) {
  const tunnelUrl = localStorage.getItem('platform_tunnel_url');
  const token = localStorage.getItem('platform_auth_token');
  
  if (!tunnelUrl) {
    throw new Error('Desktop not connected. Open desktop app and enable Web Access.');
  }
  
  return fetch(`https://${tunnelUrl}${endpoint}`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${token}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(data)
  });
}
```

## Environment Variables

### Web Platform (Cloudflare Pages)

```bash
NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_live_xxx
CLERK_SECRET_KEY=sk_live_xxx
```

### Desktop App (Build Time)

```bash
# Code signing (Mac)
CSC_IDENTITY_AUTO_DISCOVERY=true
APPLE_ID=xxx
APPLE_APP_SPECIFIC_PASSWORD=xxx

# GitHub releases
GH_TOKEN=xxx
```

### Desktop App (Runtime)

Set by desktop app internally:
```bash
PORT=3100
ALLTERNIT_API_URL=http://127.0.0.1:8013
TERMINAL_SERVER_URL=http://127.0.0.1:4096
NODE_ENV=production
```

## Directory Structure

```
~/Desktop/allternit-websites/
├── README.md                          # Main overview
├── DEPLOYMENT_GUIDE.md               # This file
├── projects/
│   ├── allternit/                     # Marketing site
│   ├── allternit-docs/                # Documentation
│   ├── allternit-protocol-institute/  # Institute
│   ├── gizzi-code-docs/               # Gizzi docs
│   ├── gizziio/                       # Download landing
│   ├── platform-allternit/            # Web platform (Cloudflare)
│   └── platform-desktop/              # Desktop documentation
│       └── README.md                  # Desktop architecture spec
└── rebuild.sh                         # Rebuild script
```

## User Flow

### First Time

```
1. User visits www.allternit.com
   ↓
2. Clicks "Download Allternit"
   ↓
3. Downloads desktop installer
   ↓
4. Installs and opens app
   ↓
5. Desktop auto-starts all services
   ↓
6. Full platform ready (offline)
```

### Web Access (Optional)

```
1. User in desktop: "Enable Web Access"
   ↓
2. Tunnel starts
   ↓
3. Browser opens to platform.allternit.com
   ↓
4. Connected to desktop via tunnel
   ↓
5. Access from any device
   ↓
6. "Disable Web Access" when done
```

## Troubleshooting

### Web Platform Shows "Desktop Offline"
- Desktop app not running
- Tunnel not enabled
- Tunnel expired (restart web access)

### Desktop Won't Start
- Check ports 8013, 4096, 3100-3200 are free
- Check logs: `~/Library/Logs/Allternit/`

### Build Errors
- Run full build: `./scripts/build-desktop.sh`
- Ensure all binaries exist in resources/

## Security

- **Tunnel**: Outbound-only, HTTPS, temporary
- **Local services**: Bind to localhost only
- **Auth**: Session tokens per tunnel session
- **No persistent exposure**: Tunnel closes when disabled

## Related Documentation

- **Desktop Architecture**: `projects/platform-desktop/README.md`
- **Desktop Specs**: `allternit-workspace/DESKTOP_FIRST_REORGANIZATION_PLAN.md`
- **Main Repo**: https://github.com/Allternit/allternit-platform
