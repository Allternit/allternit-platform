# agent-desktop P1 Integration Notes

> **Historical (2026-10-09):** this provider and the canonical environment/lease routes it used were removed in the D0 cleanup. Computers are driven through the computer toolset; see `surfaces/docs/core/canonical-computer-use.mdx`.

## Summary
Wired `lahfir/agent-desktop` as a new canonical computer-use provider `desktop.agent-desktop.canonical` in Allternit. The provider spawns the upstream Rust CLI as a subprocess, translates its JSON envelopes into canonical observations/actions, and leases an `allternit.host` environment before driving the local GUI.

## Worktree
- **Path:** `~/Desktop/allternit-workspace/allternit-session-agent-desktop-p1`
- **Branch:** `session/agent-desktop-p1`

## Files added

| File | Purpose |
|------|---------|
| `domains/computer-use/core/native/agent-desktop-rs/` | Vendored upstream `lahfir/agent-desktop` Rust source (shallow clone). `LICENSE` preserved. |
| `domains/computer-use/core/providers/agent_desktop_transport.py` | Subprocess transport: discovery, JSON-envelope parsing, timeout/error handling. |
| `domains/computer-use/core/providers/agent_desktop_canonical.py` | Canonical provider implementation: `capabilities`, `discover_roots`, `observe`, `execute_step`. |
| `docs/learnings/AGENT_DESKTOP_NOTES.md` | This file. |

## Files changed

- `domains/computer-use/core/gateway/canonical_router.py`
  - Imported `AgentDesktopTransport` and `AgentDesktopCanonicalProvider`.
  - Added discovery/registration block in `_initialize_providers()`.
  - Passed `_environments` and `_environment_backends.backend("allternit.host")` into the provider for host-access leasing.

## Provider behavior

- **Provider ID:** `desktop.agent-desktop.canonical`
- **Platform:** macOS only for P1 (matches upstream).
- **Discovery:** looks for `agent-desktop` via `ALLTERNIT_AGENT_DESKTOP_PATH`, `shutil.which`, then common paths (`~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin`, `/opt/homebrew/bin`).
- **Observe:**
  - Calls `agent-desktop snapshot --app <app> -i --include-bounds`.
  - Walks the returned accessibility tree.
  - Maps upstream qualified refs (`@<snapshot>:eN`) to canonical `@eN` refs; stores the reverse mapping per state.
  - Captures a screenshot via `agent-desktop screenshot --app <app>` and writes it to `_state_dir/artifacts`.
- **Actions:** maps canonical actions to CLI commands:
  - `click` → `click`
  - `doubleClick` → `double-click`
  - `rightClick` → `right-click`
  - `typeText` → `type`
  - `setText` → `set-value`
  - `keypress` → `press`
  - `scroll` → `scroll --direction <dir> --amount <n>`
  - `focus` → `focus`
  - `launchApp` → `launch`
  - `closeApp` → `close-app`
  - `clipboardRead` → `clipboard-get`
  - `clipboardWrite` → `clipboard-set`
- **Environment:** each `observe`/`execute_step` ensures an `allternit.host` lease is active, reusing the existing `HostEnvironmentBackend`.

## How to test

### 1. Build agent-desktop (optional)
If the CLI is not already installed:

```bash
cd ~/Desktop/allternit-workspace/allternit-session-agent-desktop-p1/domains/computer-use/core/native/agent-desktop-rs
cargo build --release
# binary will be at target/release/agent-desktop
```

### 2. Python syntax/import validation

```bash
cd ~/Desktop/allternit-workspace/allternit-session-agent-desktop-p1/domains/computer-use/core
~/Desktop/allternit-workspace/allternit/.venv/bin/python -m py_compile \
  providers/agent_desktop_transport.py \
  providers/agent_desktop_canonical.py \
  gateway/canonical_router.py

~/Desktop/allternit-workspace/allternit/.venv/bin/python -c \
  "from providers.agent_desktop_transport import AgentDesktopTransport; \
   from providers.agent_desktop_canonical import AgentDesktopCanonicalProvider; \
   print('imports ok')"
```

### 3. Runtime smoke test (requires macOS + Accessibility permission)

```bash
# Ensure agent-desktop is on PATH, or set:
export ALLTERNIT_AGENT_DESKTOP_PATH=/path/to/agent-desktop

# Launch the gateway (or use existing canonical service) and query providers:
curl -s http://localhost:8000/v1/computer-use/canonical/providers | jq '.providers[].provider_id'
```

You should see `desktop.agent-desktop.canonical` in the list when the binary is discoverable.

## Known limitations / follow-up

- The Rust CLI is vendored but **not built automatically**; consumers must build or install `agent-desktop` separately.
- macOS 13+ only; cross-platform support is out of scope for P1.
- No telemetry consent override wiring yet (upstream does not appear to expose a telemetry toggle at the CLI layer).
- Action mapping covers the most common desktop actions; additional agent-desktop commands (e.g., `select`, `toggle`, `expand`, `drag`, notifications) can be added by extending `execute_step`.
