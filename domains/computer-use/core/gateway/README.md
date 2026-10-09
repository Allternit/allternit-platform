# Allternit Computer Use Gateway

FastAPI gateway that bridges GIZZI Code to browser/desktop automation adapters.

## Architecture

```
GIZZI Code (TypeScript)
    │
    ▼ HTTP POST /v1/execute
Computer Use Gateway (Python/FastAPI)
    │
    ├──► Playwright Adapter ──► Browser automation
    ├──► CDP Adapter ──► Chrome DevTools Protocol
    └──► Accessibility Adapter ──► Native desktop (Quartz/AX, macOS)
```

## Quick Start

### Install

```bash
cd packages/computer-use/gateway
python -m venv .venv
source .venv/bin/activate  # or .venv\Scripts\activate on Windows
pip install -r requirements.txt
```

### Run

```bash
uvicorn main:app --host 127.0.0.1 --port 8080 --reload
```

### Health Check

```bash
curl http://127.0.0.1:8080/health
```

### Smoke Test

```bash
curl -X POST http://127.0.0.1:8080/v1/execute \
  -H "Content-Type: application/json" \
  -d '{
    "action": "goto",
    "session_id": "sess_123",
    "run_id": "run_123",
    "target": "https://example.com",
    "parameters": {
      "message_id": "msg_1",
      "call_id": "call_1"
    }
  }'
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | 8080 | Server port |
| `HOST` | 127.0.0.1 | Bind address |

## Integration with GIZZI

Enable the browser tool in GIZZI:

```bash
export GIZZI_ENABLE_BROWSER_TOOL=true
export Allternit_COMPUTER_USE_URL=http://localhost:8080

gizzi browser goto https://example.com
```

## Development Phases

### Phase 1: Stub (Current)
- [x] FastAPI scaffold
- [x] Request/response contract
- [x] Validation
- [x] Stub responses

### Phase 2: Playwright Adapter
- [ ] Session manager (persist browser per session_id)
- [ ] `goto` action
- [ ] `screenshot` action
- [ ] Base64 image artifacts

### Phase 3: Full Adapters
- [ ] CDP adapter
- [ ] Desktop adapter
- [ ] Artifact persistence
- [ ] Receipt logging

## API Contract

See `browser.ts` in GIZZI Code for TypeScript side.
Key points:
- `session_id` persists browser state across calls
- `run_id` identifies the logical run
- `adapter_preference` is hint, gateway may override
- Screenshots return as `data:image/png;base64,...` URLs

## Related Documents

- `spec/session-surfaces/SessionProjection.md` - Session/projection architecture
- `cmd/gizzi-code/src/runtime/tools/builtins/browser.ts` - GIZZI tool bridge
