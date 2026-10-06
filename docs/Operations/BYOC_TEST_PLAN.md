# BYOC (Bring Your Own Compute) Test Plan

## Goal
Test the complete flow: Cloudflare Pages frontend → SSH to VPS → Install backend → Connect directly to backend

## Prerequisites

### 1. VPS Requirements
- Linux server (Ubuntu 22.04+ recommended)
- SSH access (key or password)
- Public IP or hostname
- Ports: 22 (SSH), 4096 (backend)
- Architecture: x86_64 or ARM64

### 2. Frontend environment variables
Set these in GitHub Actions secrets / Cloudflare Pages project env (not Vercel):

```
# Database (for connection metadata storage)
DATABASE_URL=sqlite://./data.db  # or PostgreSQL

# Clerk Auth
NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY=pk_live_...
CLERK_SECRET_KEY=sk_live_...

# OAuth (for MCP server tokens)
OAUTH_TOKEN_SECRET=your-secret-here

# Upstash Redis (optional - for multi-replica OAuth)
UPSTASH_REDIS_REST_URL=
UPSTASH_REDIS_REST_TOKEN=
```

### 3. Backend Binary Hosting
The installer needs to download the backend binary. Options:

**Option A: GitHub Releases (Recommended)**
- Upload `gizzi-code` binary to GitHub releases
- Set `ALLTERNIT_GITHUB_REPO=Allternit/allternit-platform`
- Set `ALLTERNIT_GITHUB_REF=v1.0.0`

**Option B: Custom URL**
- Host binary at `https://your-cdn.com/gizzi-code-linux-amd64`
- Set `ALLTERNIT_BINARY_BASE_URL=https://your-cdn.com`

## Test Flow

### Phase 1: Deploy frontend to Cloudflare Pages

```bash
# Push to GitHub (triggers deploy-cloudflare-pages.yml)
git push origin main
```

**Verify**: https://platform.allternit.com loads without errors

### Phase 2: Add SSH Connection

1. **Login** to platform.allternit.com
2. **Navigate**: Settings → VPS Connections → "Add Connection"
3. **Enter VPS details**:
   - Name: "Test VPS"
   - Host: your-vps-ip-or-hostname
   - Port: 22
   - Username: your-ssh-username
   - Auth: SSH Key or Password

**Expected**: Test connection succeeds, shows OS info

### Phase 3: Install Backend

1. **Click**: "Install Backend" on the connection
2. **Watch progress**:
   - Connecting → Detecting OS → Downloading → Installing → Verifying → Complete

**What happens behind the scenes**:
```
Cloudflare Pages frontend → SSH to VPS → Download install.sh → Run installer
                                           ↓
                              Creates systemd service
                              Starts on port 4096
                                           ↓
                              Returns gateway URL to frontend
```

**Verify on VPS**:
```bash
# Check service status
systemctl status allternit-backend

# Check port is listening
ss -tlnp | grep 4096
curl -H "Authorization: Basic $(echo -n 'gizzi:PASSWORD' | base64)" http://localhost:4096/v1/global/health
```

### Phase 4: Connect Terminal

1. **Navigate**: Shell → New Session
2. **Select**: Your VPS connection
3. **Open**: Terminal should connect via WebSocket

**Expected**: Terminal opens, shows VPS shell prompt

**Behind the scenes**:
```
Browser → GET /api/v1/runtime/backend → Returns {gateway_url: "http://vps:4096"}
Browser → WebSocket to ws://vps:4096/ws/terminal/{sessionId}
```

### Phase 5: Test File Operations

1. **In terminal**: Click file browser icon
2. **Browse**: /home, /tmp, etc.
3. **Upload**: Drag file into browser
4. **Download**: Click file → Download

**Expected**: Files transfer between browser and VPS

## Troubleshooting

### SSH Connection Fails
```bash
# Test from your machine first
ssh -v user@vps-ip

# Check VPS firewall
sudo ufw status
sudo iptables -L | grep 22

# Check Cloudflare Pages / GitHub Actions deploy logs
```

### Backend Install Fails
```bash
# Check VPS has required deps
ldd --version  # glibc 2.31+
which curl wget bash

# Manual test of installer
curl -fsSL https://install.allternit.com | bash

# Check logs on VPS
sudo journalctl -u allternit-backend -f
sudo tail -f /var/log/allternit.log
```

### Terminal Won't Connect
```bash
# Check backend is running on VPS
curl http://localhost:4096/v1/global/health

# Check firewall
sudo ufw allow 4096/tcp

# Check browser console for CORS errors
```

### "Runtime backend is unavailable"
- Backend not installed → Run installer
- Backend crashed → Check logs
- Wrong URL → Verify /api/v1/runtime/backend returns correct gateway_url

## Security Checklist

- [ ] SSH keys used instead of passwords (recommended)
- [ ] Backend auth token is random per-install
- [ ] VPS firewall restricts 4096 if needed
- [ ] HTTPS only for production
- [ ] Clerk JWT properly validated

## Success Criteria

| Feature | Status |
|---------|--------|
| Sign in via Clerk | ⬜ |
| Add SSH connection | ⬜ |
| Test SSH connection | ⬜ |
| Install backend | ⬜ |
| Terminal connects | ⬜ |
| File browser works | ⬜ |
| File upload/download | ⬜ |
| Backend survives reboot | ⬜ |
