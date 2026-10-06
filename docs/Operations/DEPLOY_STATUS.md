# Deployment Status

## Last Commit
- **Hash**: `1a6ae002`
- **Message**: fix: add 'manual' to WizardData infraType type
- **Time**: Just pushed

## Deployment Status
Web surfaces deploy to **Cloudflare Pages**, not Vercel.

Check status: GitHub Actions `deploy-cloudflare-pages.yml` on `Allternit/allternit-platform`.

## What's Deployed

### API Endpoint ✅
`POST /api/v1/runtime/backend/manual`
- Registers backend by URL without SSH
- Verifies backend health
- Returns 404 until deployment completes

### UI Component ✅
"Enter Backend URL" option in:
- Onboarding flow (InfrastructureStep)
- Settings → VPS Connections

## Current Issue: Local Backend Auto-Detection

**Problem**: When user selects "Use Local Backend", the UI assumes it's running without checking.

**Expected Behavior**:
1. Frontend probes `http://localhost:8013/v1/global/health`
2. If backend responds → auto-activate
3. If no backend → show "Start local backend" instructions

**Actual Behavior**:
- Shows "Local backend detected" message immediately
- No actual health check performed
- User must manually know to start backend on port 8013

## Your Current Setup

| Component | Status |
|-----------|--------|
| Backend running | ✅ Port 4096 |
| Tunnel active | ✅ https://molecules-dsc-specifications-dangerous.trycloudflare.com |
| UI deployed | ⏳ Waiting for build |

## How to Test (Once Deployed)

1. Go to https://platform.allternit.com
2. During onboarding, select **"Enter Backend URL"**
3. Fill in:
   - Name: `MacBook Test`
   - URL: `https://molecules-dsc-specifications-dangerous.trycloudflare.com`
   - Token: `Basic Z2l6emk6YzVkNDY1NmU3ZjM3NTNiYzBkODk5NmQ0ZGJlYmY0OGNlY2JkNzM2MmJlMDZlNjBm`
4. Click "Connect to Backend"

## Next Steps

1. Wait for the Cloudflare Pages deploy to finish
2. Test the "Enter Backend URL" flow
3. Fix local backend auto-detection (add health check probe)
