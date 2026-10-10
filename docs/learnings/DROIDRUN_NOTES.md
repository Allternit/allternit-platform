# DroidRun Mobile Harness P1 Integration Notes

> **Historical (2026-10-09):** this provider and the canonical environment/lease routes it used were removed in the D0 cleanup. Computers are driven through the computer toolset; see `surfaces/docs/core/canonical-computer-use.mdx`.

**Worktree:** `~/Desktop/allternit-workspace/allternit-session-droidrun-p1`  
**Branch:** `session/droidrun-p1`  
**Provider ID:** `mobile.droidrun.canonical`

## What Changed

1. **Vendored DroidRun mobile harness**
   - Cloned `https://github.com/droidrun/mobile-harness` into
     `domains/computer-use/core/mobile-harness/droidrun/`.
   - The harness itself is Markdown + operational guides; the runtime control
     API comes from the `mobilerun-core` Python package.

2. **New canonical provider / environment backend**
   - `domains/computer-use/core/providers/droidrun_canonical.py`
   - Implements both the canonical `ComputerProvider` protocol and the
     `EnvironmentBackend` protocol so the same instance is usable from:
     - `CanonicalComputerService` (`/providers`, `/observe`, `/transactions`)
     - `EnvironmentBackendService` (`/environments/{id}/mobile/actions`)

3. **Router registration**
   - `domains/computer-use/core/gateway/canonical_router.py`
   - Imports `DroidRunCanonicalProvider`.
   - Registers it with `service` and `_environment_backends` in
     `_initialize_providers()`.

4. **Optional dependency**
   - `domains/computer-use/core/pyproject.toml`
   - Added `[project.optional-dependencies] droidrun = ["mobilerun-core[local]>=0.1.0"]`.

5. **This file**
   - Summarizes changes, validation steps, and known blockers.

## Supported Actions

| Mobile action | DroidRun path | Notes |
|---------------|---------------|-------|
| `screenshot` | `device.screenshot()` | Returns base64 PNG via environment backend; `ImageEvidence` via canonical observe. |
| `tap` | `device.tap_text()` or `adb shell input tap` | Coordinate tap falls back to ADB. |
| `swipe` | `device.swipe()` or `adb shell input swipe` | Coordinates + duration. |
| `type` | `device.type()` or `adb shell input text` | Portal/ADB-native preferred; ADB fallback. |
| `key` | `device.press_key()` or `adb shell input keyevent` | ADB fallback. |
| `launch_app` | `device.start_app(package)` | Package name required. |
| `shell` | `adb shell ...` | Requires `local-android-adb` backend. |
| `push_file` | `adb push ...` | Requires `local-android-adb` backend. |
| `pull_file` | `adb pull ...` | Requires `local-android-adb` backend. |
| `ui_tree` | `device.ui()` | Serialized in mobile-action response; converted to `ElementNode`s in canonical observation. |
| `plan` | Thin observe-act stub | Takes a `goal`, captures one observation, returns. Intended to be extended into a real planner. |

## Environment Provisioning

Create an Android environment with `provider_id: mobile.droidrun.canonical` and
`metadata` selecting the backend:

```json
{
  "owner_id": "...",
  "provider_id": "mobile.droidrun.canonical",
  "os": "android",
  "isolation": "host",
  "metadata": {
    "backend": "local-android-adb",
    "serial": "<adb-serial>"
  }
}
```

Other backends:

- `local-android-http` — add `portal_url` and optionally `portal_token`.
- `cloud` — add `cloud_device_id`; requires `MOBILERUN_CLOUD_API_KEY`.

## Validation Performed

```bash
cd ~/Desktop/allternit-workspace/allternit-session-droidrun-p1/domains/computer-use/core

# Syntax check
python3 -m py_compile providers/droidrun_canonical.py gateway/canonical_router.py

# Provider import + capability/manifest smoke test (no device needed)
python3 -c "
import asyncio, sys
sys.path.insert(0, '.')
from providers.droidrun_canonical import DroidRunCanonicalProvider
from contracts.canonical import OperatingSystem

async def main():
    p = DroidRunCanonicalProvider()
    caps = await p.capabilities()
    assert caps.provider_id == 'mobile.droidrun.canonical'
    assert OperatingSystem.ANDROID.value in caps.operating_systems
    assert 'tap' in caps.actions and 'plan' in caps.actions
    manifest = p.manifest()
    assert manifest.provider_id == 'mobile.droidrun.canonical'
    assert 'mobile' in manifest.capabilities
    print('OK')

asyncio.run(main())
"
```

## Test Path

1. Install the optional dependency:
   ```bash
   pip install -e 'domains/computer-use/core[droidrun]'
   ```
2. Connect an Android device with ADB enabled.
3. Start the gateway (dev server is disallowed in this session; run tests only).
4. POST `/v1/computer-use/canonical/environments` with `provider_id: mobile.droidrun.canonical`.
5. POST `/v1/computer-use/canonical/environments/{id}/provision`.
6. POST `/v1/computer-use/canonical/environments/{id}/mobile/actions` with:
   ```json
   {"action": "screenshot", "arguments": {}, "lease_id": "...", "holder_id": "...", "approval_id": "..."}
   ```
7. Verify the provider also appears in `GET /v1/computer-use/canonical/providers`.

## Blockers / Follow-ups

- `mobilerun-core` is **not installed** in this worktree; the provider reports
  itself unavailable until it is. No live Android device testing was performed.
- The exact shape of `device.ui()` and `device.screenshot()` was inferred from
  the DroidRun README. If the real package returns a different structure, the
  `_convert_ui_tree` and `_image_evidence` helpers need adjustment.
- Shell, push, and pull only work with the `local-android-adb` backend. Cloud
  and Portal-HTTP backends will raise a clear error for those actions.
- The `plan` action is a stub that observes once; it needs a real planner/agent
  loop for production use.
- No unit tests were added; only syntax and smoke validation were run.
