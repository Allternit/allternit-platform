# uhp-gateway

UHP (Unified Harness Protocol) 2026-08-11 gateway: turns prompts into real
CLI-agent turns by driving the ao engine (herdr) over its Unix-socket JSON-RPC
API. One headless CLI invocation per turn; the per-session workspace directory
is the checkpoint.

## Spawn gate (audit S1)

Every third-party harness used to run in full-bypass mode with no Allternit
gate in front of it. Each turn now goes through `src/spawn_gate.rs` before
launch, and the policy lives in CommRails (`commrails/src/hook`):

- **Hard floor**: denied for every hooked harness, bound or not, in every
  permission mode. It covers recursive `rm` of `/`, `~` or `$HOME` (and
  top-level system dirs), `mkfs`, `dd` or a redirect onto a disk device, fork
  bombs, shutdown and reboot, `git push --force` or a delete to main/master,
  and keychain dumps (`security find-generic-password` and similar).
- **Gate 2**: when a turn is bound to a WIH (request
  `metadata.allternit_wih_id`, plus `ALLTERNIT_COMMRAILS_ROOT` on the gateway),
  every tool call is checked by `Gate::pre_tool`. The WIH must be open-signed
  and the tool must be allowed. Every write path must resolve inside the
  workspace and be covered by a lease held by **this** WIH. Any path it cannot
  resolve is denied.
- **Ledger**: every WIH-bound decision and every denial is appended as a
  `HarnessToolGated` event. Refused spawns are appended as `HarnessSpawnRefused`.
- **Fail closed**: if `allternit-commrails` is missing, Claude turns are
  refused rather than run unhooked. Bad hook input and gate or lease errors
  produce a deny, and a hook panic exits 2, which blocks the call.

Environment:

| Var | Meaning |
|---|---|
| `ALLTERNIT_COMMRAILS_BIN` | Path to `allternit-commrails` (else `PATH`). |
| `ALLTERNIT_COMMRAILS_ROOT` | CommRails root that holds WIHs, leases and the ledger. Required for WIH-bound turns. Unbound turns log to the session dir. |

### Per-harness status

| Harness | Launch flags now | Gate class | What enforces policy | On a WIH that requires leased writes |
|---|---|---|---|---|
| claude / claude-code | `--permission-mode acceptEdits --settings <session settings>` (was `--dangerously-skip-permissions`) | **hook** | A PreToolUse hook (`allternit-commrails hook claude-pretool`) runs on every tool call: hard floor, then Gate 2 and the WIH's own lease. The settings allow rules keep headless runs prompt-free. | admitted |
| codex | `-c sandbox_mode="workspace-write" -c approval_policy="never" -c sandbox_workspace_write.network_access=true` (was `--dangerously-bypass-approvals-and-sandbox`) | **sandbox** | The codex OS sandbox confines writes to the session workspace. No Allternit hook yet (codex 0.158 hooks need persisted trust). | **refused** |
| gemini | `--approval-mode yolo` | **ungated** | nothing | **refused** |
| qwen | `--yolo` | **ungated** | nothing | **refused** |
| cline | `--auto-approve true` | **ungated** | nothing | **refused** |
| pi | `--approve --no-extensions` | **ungated** | nothing | **refused** |
| opencode | `--auto --pure` | **ungated** | nothing | **refused** |
| kimi | (none; headless `-p`) | **ungated** | nothing | **refused** |
| dsh | job JSON | **ungated** | nothing | **refused** |

Unbound turns (no WIH) are admitted for every harness, so plain delegation
behaves as before. The hard floor still applies to Claude.

The same classification and argv rewrite apply to
`allternit-commrails orchestrator spawn [--wih <id>] ...`
(`commrails/src/orchestrator`): it rewrites Claude's and Codex's bypass flags
and refuses ungated harnesses (including `agy`) on a leased WIH.

The agent-orchestrator `ao-spawn` shim and the ao engine's `ao spawn` /
`ao recover --apply` apply the same classes to the shell launch line through
`tools/agent-orchestrator/scripts/ao-spawn-gate` and its byte-for-byte engine
mirror `infrastructure/executor/ao-engine/src/cli/ao_gate.rs` (parity:
`infrastructure/executor/ao-engine/tests/ao_parity/gate_parity.sh` and
`run.sh`). Claude's `--dangerously-skip-permissions` becomes
`--permission-mode acceptEdits --settings <hook settings>`, codex's bypass
becomes the `-c` sandbox flags above (the same flags `gate_argv` now emits),
every other harness is logged `gate=ungated` in
`~/.agent-orchestrator/logs/spawn-gate.log`, and a missing
`allternit-commrails` binary refuses a Claude spawn.
