# Phone-Harness iOS Provider — P1 Integration Notes

> **Historical (2026-10-09):** this provider and the canonical environment/lease routes it used were removed in the D0 cleanup. Computers are driven through the computer toolset; see `surfaces/docs/core/canonical-computer-use.mdx`.

## Goal
Wire the vendored `phone-harness` project as the canonical iOS mobile computer-use
provider `mobile.phone.canonical` in Allternit.

## Worktree

```
~/Desktop/allternit-workspace/allternit/allternit-session-phone-harness-p1
```

Branch: `session/phone-harness-p1`

## Files changed

| Path | Change |
|------|--------|
| `domains/computer-use/core/mobile-harness/phone-harness/` | Forked/vendored `https://github.com/ShawnPana/phone-harness` (src, README, LICENSE, SKILL.md, install.md, pyproject.toml, agent-workspace, launcher). |
| `domains/computer-use/core/providers/phone_harness_canonical.py` | New canonical provider. Wraps phone-harness with fallback drivers `pymobiledevice3` -> `idb`. Implements `capabilities`, `observe`, `execute_step`, `discover_roots`, `close`, plus a direct `mobile_action` helper. |
| `domains/computer-use/core/gateway/canonical_router.py` | Imports and registers `PhoneHarnessCanonicalProvider`; extends `POST /v1/computer-use/canonical/environments/{environment_id}/mobile/actions` to dispatch iOS actions to the phone-harness provider while keeping the existing Android path. |
| `domains/computer-use/core/contracts/canonical.py` | Added `IOS = "ios"` to the `OperatingSystem` enum so iOS environments are canonically representable. |
| `domains/computer-use/core/pyproject.toml` | Added `[project.optional-dependencies] mobile = [...]` with `pymobiledevice3` and macOS pyobjc frameworks needed by phone-harness. |
| `docs/learnings/PHONE_HARNESS_NOTES.md` | This file. |

## Supported iOS mobile actions

The provider supports both canonical `ActionStep` names and the simplified
`/mobile/actions` API:

| Mobile action | Canonical step | Arguments |
|---------------|----------------|-----------|
| `screenshot` | `observe` | — |
| `tap` / `click` | `tap` | `x`, `y` |
| `swipe` / `scroll` | `swipe` | `x1`, `y1`, `x2`, `y2`, `duration_ms` |
| `type` | `typeText` | `text` |
| `home` | `keypress` with `key=home` | — |
| `app_launch` | `navigate` | `bundle_id_or_name` / `app` |
| `accessibility_tree` | `accessibility_tree` | — |

## Driver selection

`PhoneHarnessCanonicalProvider` picks the first usable driver at init:

1. **phone-harness** (macOS + iPhone Mirroring + Quartz/Vision/AppKit)
2. **pymobiledevice3** (USB/Wi-Fi iOS services)
3. **idb** (Facebook/Meta iOS Debug Bridge CLI)

If none are ready the provider still registers so `/providers` advertises it
honestly, but the manifest includes a `limitations` marker and runtime calls
raise a clear error.

## Validation run

```bash
cd ~/Desktop/allternit-workspace/allternit/allternit-session-phone-harness-p1/domains/computer-use/core

# Syntax check
python3 -m py_compile \
  providers/phone_harness_canonical.py \
  gateway/canonical_router.py \
  contracts/canonical.py \
  mobile-harness/phone-harness/src/phone_harness/*.py

# Import / registration check (uses repo venv)
~/Desktop/allternit-workspace/allternit/domains/computer-use/core/.venv/bin/python -c "
import asyncio
from providers.phone_harness_canonical import PhoneHarnessCanonicalProvider
from core.canonical_service import CanonicalComputerService

async def main():
    svc = CanonicalComputerService()
    p = PhoneHarnessCanonicalProvider('/tmp/phone-artifacts')
    await svc.register(p)
    print('registered:', [m.provider_id for m in svc.capabilities()])

asyncio.run(main())
"
```

Both checks pass. No runtime iOS device or mirroring session is required for
import/registration validation.

## Test path

1. Install mobile dependencies:
   ```bash
   pip install -e "domains/computer-use/core[mobile]"
   ```
2. On macOS with iPhone Mirroring paired:
   ```bash
   cd domains/computer-use/core/mobile-harness/phone-harness
   ./phone-harness --doctor
   ```
3. Create an iOS environment via the canonical API and call
   `POST /v1/computer-use/canonical/environments/{id}/mobile/actions`
   with `{"action": "screenshot", "arguments": {}}`.
4. Verify the provider appears at
   `GET /v1/computer-use/canonical/providers` with
   `provider_id: "mobile.phone.canonical"`.

## Blockers / follow-ups

- **Runtime verification blocked**: This environment has no paired iPhone or
  iPhone Mirroring session, so live tap/screenshot tests cannot be run.
- **pymobiledevice3 gesture gap**: The fallback driver does not implement
  `swipe` because pymobiledevice3 lacks a direct HID gesture API. For gestures
  in that mode, install `idb`.
- **Accessibility tree on phone-harness**: Derived from OCR text boxes, not a
  native iOS AX hierarchy. A future pass could integrate
  `pymobiledevice3`'s accessibility snapshot for richer semantics.
- **Environment backend**: iOS actions currently route through the host-side
  provider, not through an `EnvironmentBackend`. A longer-term integration could
  add an iOS environment backend for sandboxed/remote devices.
