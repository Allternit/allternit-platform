# gizzi-code Docs-to-Code Parity Audit

**Worktree:** `~/Desktop/allternit-workspace/allternit-session-gizzi-cleanup`  
**Doc source:** `surfaces/docs/` (Mintlify site)  
**Code scope:** `cmd/gizzi-code/src/`  
**Date:** 2026-08-17

---

## Executive summary

The Mintlify docs describe a mature product surface. In `cmd/gizzi-code` the **high-level shape is implemented** (interactive TUI, `gizzi exec`, auth, permission profiles, built-in tools, MCP client, Rails peer messaging), but there are **many mismatches between the docs and the actual CLI**: wrong env-var names, missing CLI flags, aspirational config sections, and several pages that actually describe separate packages (`allternit-api`, `allternit-sdk`, `allternit-rails`, `agent-daemon`, `allternit-mux`, `@allternit/cli-typescript`).

### Top findings

1. **Config schema mismatch** — `configuration.mdx` documents `[model]`, `[auth].default_profile`, `[permissions]`, `[ui]`, and `[telemetry]` tables that do not exist in gizzi-code's actual schema. The real config uses top-level keys, `auth.active_profile`, `permission`, `approval_policy`, `permission_profiles`, and `experimental.openTelemetry`.
2. **Auth env var mismatch** — Docs use `ALLTERNIT_API_KEY` and `ALLTERNIT_ENDPOINT`; gizzi-code uses provider-specific env vars (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.) and `ALLTERNIT_API_URL`.
3. **`gizzi exec --permission-profile` is missing** — `permission-profiles.mdx` and `ci-mode.mdx` show `gizzi exec --permission-profile <name>`, but the CLI only has `--permission-mode`.
4. **`gizzi auth login --base-url` is missing** — `authentication.mdx` shows `--base-url` on login; it is only accepted by `gizzi auth profile add`.
5. **Keyring is stubbed** — `credential_store = "keyring"` exists but the default backend throws `notImplementedKeyringBackend()`.
6. **Stream-JSON event names differ from docs** — Docs show `message`/`result` types; actual types are `text`, `tool_use`, `tool_running`, `step_start`, `step_finish`, `error`, `reasoning`, `background_task_started`, `background_task_finished`.
7. **`allternit` wrapper status/doctor output mismatch** — `allternit-wrapper.mdx` examples show `api`/`gateway` service checks; gizzi-code checks `API`/`Kernel` and omits gateway.
8. **Several docs describe separate packages** — `tool-belt.mdx`, `mcp.mdx`, `strict-tool-use.mdx` describe `allternit-sdk` + `allternit-api`; `commrails-cli.mdx`, `agent-daemon.mdx`, `allternit-mux.mdx`, `cli-typescript.mdx` describe standalone binaries/packages. These are implemented, but not inside `cmd/gizzi-code`.

---

## Priority gap list

| Priority | Gap | Doc page | Status | Recommendation |
|----------|-----|----------|--------|----------------|
| P0 | Config schema (`[model]`, `[permissions]`, `[ui]`, `[telemetry]`, `auth.default_profile`) does not match implementation | `configuration.mdx` | Missing/Partial | **Fix docs** to match implemented schema, or implement the documented schema if product wants it |
| P0 | `ALLTERNIT_API_KEY` / `ALLTERNIT_ENDPOINT` env vars are not implemented | `authentication.mdx`, `configuration.mdx` | Missing | **Implement** an `allternit` provider that reads `ALLTERNIT_API_KEY`, or fix docs |
| P0 | `gizzi exec --permission-profile <name>` does not exist | `permission-profiles.mdx`, `ci-mode.mdx`, `headless-execution.mdx` | Missing | **Implement** the flag and merge the named profile into the session ruleset |
| P1 | `gizzi auth login --base-url` missing | `authentication.mdx` | Missing | **Add** `--base-url` to login command |
| P1 | `keyring` credential store is stubbed | `authentication.mdx`, `configuration.mdx` | Partial | **Implement** real OS keyring backend, or remove `keyring` from docs |
| P1 | `GIZZI_CONFIG_PATH` / `GIZZI_CREDENTIAL_STORE` env vars not implemented | `configuration.mdx` | Missing | **Implement** or change docs to `GIZZI_CONFIG` / `GIZZI_CONFIG_DIR` |
| P1 | `stream-json` event schema mismatches docs | `headless-execution.mdx`, `ci-mode.mdx` | Partial | **Align** event names or update docs |
| P1 | Exit code `2` not produced for auth/config failures | `headless-execution.mdx`, `ci-mode.mdx` | Partial | **Implement** `CIMode.ExitCode` usage in `run.ts` |
| P2 | `allternit status` omits gateway; `doctor` checks wrong things | `allternit-wrapper.mdx` | Partial | **Fix** wrapper output/doctor checks, or update docs |
| P2 | Installation docs show stale package/binary names (`dist/gizzi`, `@gizzi/tui`) | `installation.mdx` | Partial | **Fix** docs and internal upgrade/uninstall code |
| P2 | `gizzi-core.mdx` describes a package (`@allternit/gizzi-core`) that does not exist | `core/gizzi-core.mdx` | Missing | **Clarify** docs or create the package/exports |
| P2 | `gizzi-runtime.mdx` `Agent` class / runtime tiers not in gizzi-code | `core/gizzi-runtime.mdx` | Missing | **Move** to `@allternit/core` docs or implement |
| P3 | Permission rule semantics differ (docs say first-match, code uses last-match) | `permission-profiles.mdx` | Partial | **Decide** intended behavior and fix code or docs |
| P3 | Tool pattern names differ (`web_search`/`code_execution`/`delete` vs `websearch`/`bash`/`edit`) | `permission-profiles.mdx` | Partial | **Align** rule keys with runtime tool names |
| P3 | `config.toml` `0o600` not applied for permission-profile writes | `configuration.mdx` | Partial | **Apply** secure mode to all config writes |

---

## Full parity table

### CLI surface (`surfaces/docs/cli/*.mdx`)

| Doc page | Feature | Status | Evidence in code | Gap note |
|----------|---------|--------|------------------|----------|
| `cli/overview.mdx` | `gizzi` binary | Implemented | `package.json:46-49`, `src/cli/main.ts:101` | — |
| `cli/overview.mdx` | Default `gizzi` → interactive TUI | Implemented | `src/cli/main.ts:205`, `src/cli/ui/ink-app/thread.ts:74`, `src/cli/ui/ink-app/app.tsx:24` | — |
| `cli/overview.mdx` | `gizzi exec` | Implemented | `src/cli/commands/run.ts:988-998`, `src/cli/main.ts:208` | — |
| `cli/overview.mdx` | Headless / CI mode | Implemented | `src/cli/ci.ts`, `src/cli/main.ts:120-128` | — |
| `cli/overview.mdx` | Built-in tools | Implemented | `src/runtime/tools/builtins/`, `src/cli/commands/run.ts:567-586` | — |
| `cli/overview.mdx` | Code editing | Implemented | `src/runtime/tools/builtins/edit.ts`, `write.ts` | — |
| `cli/overview.mdx` | Browser automation | Implemented (bridge) | `src/runtime/tools/builtins/browser.ts` | Actual driver is external ACU gateway |
| `cli/overview.mdx` | Permission profiles | Implemented | `src/cli/commands/permission-profile/index.ts`, `src/cli/commands/config.ts:33-149` | Multiple competing profile systems exist |
| `cli/overview.mdx` | Credential management | Implemented | `src/cli/commands/auth.ts`, `src/runtime/context/config/auth-profiles.ts` | Keyring is stubbed |
| `cli/installation.mdx` | `npm install -g @allternit/gizzi-code` | Partial | `package.json:5,46-49` | Upgrade/uninstall still reference `@gizzi/tui` / `gizzi-ai` |
| `cli/installation.mdx` | One-line installer | Separate package / Doc-only | — | External installer, not in source |
| `cli/installation.mdx` | Build from source | Partial | `script/build.ts`, `script/build-production.js:415` | Doc says binary at `dist/gizzi`; actual is `dist/gizzi-code` |
| `cli/installation.mdx` | `gizzi --version` | Implemented | `src/cli/main.ts:101-105`, `src/shared/installation/index.ts:198-204` | — |
| `cli/installation.mdx` | Config path `~/.config/gizzi-code/config.toml` | Partial | `src/runtime/context/global/paths.ts:5-14` | Only true on Linux; macOS uses `~/Library/Application Support/gizzi-code/` |
| `cli/installation.mdx` | `gizzi auth login` | Implemented | `src/cli/commands/auth.ts:20-44` | — |
| `cli/installation.mdx` | Homebrew tap | Separate package | — | Not implemented in gizzi-code source |
| `cli/authentication.mdx` | `gizzi auth login --api-key --provider --profile` | Implemented | `src/cli/commands/auth.ts:20-44` | — |
| `cli/authentication.mdx` | Interactive API-key prompt | Implemented | `src/cli/commands/auth.ts:30-36` | — |
| `cli/authentication.mdx` | `gizzi auth login --base-url` | Missing | `src/cli/commands/auth.ts:25-27` | Only `gizzi auth profile add` accepts `--base-url` |
| `cli/authentication.mdx` | `gizzi auth status` output strings | Implemented | `src/cli/commands/auth.ts:46-64`, `src/runtime/context/config/auth-profiles.ts:237-251` | — |
| `cli/authentication.mdx` | Auth profile list/add/set-active/remove | Implemented | `src/cli/commands/auth.ts:102-195` | — |
| `cli/authentication.mdx` | `file`/`keyring`/`auto` credential stores | Partial | `src/runtime/context/config/credential-store.ts:86-144` | `keyring` backend throws |
| `cli/authentication.mdx` | `ALLTERNIT_API_KEY=... gizzi exec ...` | Partial / Misleading | no `ALLTERNIT_API_KEY` reference | Requires pre-existing profile with `api_key_env` |
| `cli/configuration.mdx` | 3-source precedence (flags → env → toml) | Partial | `src/runtime/context/config/config.ts:71-343` | Actual precedence is more complex |
| `cli/configuration.mdx` | `[model]` table | Missing / Doc-only | `src/runtime/context/config/config.ts:1268` | Schema uses top-level `model` string |
| `cli/configuration.mdx` | `[auth].credential_store` | Implemented | `src/runtime/context/config/config.ts:1272` | — |
| `cli/configuration.mdx` | `[auth].default_profile` | Missing | `src/runtime/context/config/config.ts:1271` | Implemented as `auth.active_profile` |
| `cli/configuration.mdx` | `[permissions]` table | Missing / Doc-only | `src/runtime/context/config/config.ts:1287-1315,1442` | Replaced by `permission`, `approval_policy`, `permission_profiles` |
| `cli/configuration.mdx` | `[ui].theme` | Partial | `src/runtime/context/config/config.ts:1230` | Theme is top-level, values not validated |
| `cli/configuration.mdx` | `[ui].syntax_highlighting` | Missing | `src/cli/ui/ink-app/utils/settings/types.ts:711` | Exists as TUI setting, not config.toml key |
| `cli/configuration.mdx` | `[ui].code_wrap` | Missing | — | No such setting |
| `cli/configuration.mdx` | `[telemetry]` table | Missing / Doc-only | `src/runtime/context/config/config.ts:1465-1468` | Only `experimental.openTelemetry` boolean |
| `cli/configuration.mdx` | `gizzi config profile` commands | Implemented | `src/cli/commands/config.ts:33-149` | — |
| `cli/configuration.mdx` | `ALLTERNIT_API_KEY` env | Missing | — | Not referenced |
| `cli/configuration.mdx` | `ALLTERNIT_ENDPOINT` env | Missing | `src/runtime/services/api/allternitApi.ts:30` | Use `ALLTERNIT_API_URL` |
| `cli/configuration.mdx` | `GIZZI_CONFIG_PATH` env | Missing | `src/runtime/context/flag/flag.ts:13,17` | Use `GIZZI_CONFIG` / `GIZZI_CONFIG_DIR` |
| `cli/configuration.mdx` | `GIZZI_CREDENTIAL_STORE` env | Missing | `src/runtime/context/config/auth-profiles.ts:162` | Not implemented |
| `cli/configuration.mdx` | `GIZZI_ENABLE_RAILS_PEER` env | Implemented | `src/runtime/gizzi-core/services/railsPeer.ts:49` | — |
| `cli/configuration.mdx` | `config.toml` created with `0o600` | Partial | `src/runtime/context/config/auth-profiles.ts:77-78` | Only auth writes set mode |
| `cli/headless-execution.mdx` | `gizzi exec` command | Implemented | `src/cli/commands/run.ts:988-998` | — |
| `cli/headless-execution.mdx` | Pipe-safe / non-interactive | Implemented | `src/cli/commands/run.ts:588,769-772,457-480` | — |
| `cli/headless-execution.mdx` | Default plain text output | Implemented | `src/cli/commands/run.ts:996` | — |
| `cli/headless-execution.mdx` | `--output-format json` | Implemented | `src/cli/commands/run.ts:333-337,590,598-608` | — |
| `cli/headless-execution.mdx` | `--output-format stream-json` | Partial | `src/cli/commands/run.ts:589,599-601` | Event type names differ from docs |
| `cli/headless-execution.mdx` | Default `dontAsk` permission mode | Implemented | `src/cli/commands/run.ts:997,338-342,404` | — |
| `cli/headless-execution.mdx` | `--permission-profile` flag | Missing | `src/cli/commands/run.ts:228-401` | Not defined |
| `cli/headless-execution.mdx` | Exit codes `0`/`1`/`2` | Partial | `src/cli/commands/run.ts:430,444,484,489,856`, `src/cli/main.ts:309` | Code `2` not produced |
| `cli/permission-profiles.mdx` | `gizzi config profile add/list/set-active/remove` | Implemented | `src/cli/commands/config.ts:33-149` | — |
| `cli/permission-profiles.mdx` | Permission modes `suggest`/`ask`/`dontAsk` | Partial | `src/runtime/context/config/config.ts:714`, `src/runtime/tools/guard/permission/next.ts:93` | `suggest` missing; `ask` is an action, `dontAsk` is a mode |
| `cli/permission-profiles.mdx` | Rule syntax `tool=mode` | Implemented | `src/cli/commands/config.ts:20-31` | — |
| `cli/permission-profiles.mdx` | Tool patterns (`bash`, `edit`, `delete`, `web_search`, `code_execution`, `*`) | Partial | `src/runtime/tools/builtins/websearch.ts:67`, `bash.ts:164`, `edit.ts:57` | `delete`, `web_search`, `code_execution` are not valid keys |
| `cli/permission-profiles.mdx` | First-matching-rule semantics | Partial | `src/runtime/tools/guard/permission/next.ts:377-381` | Code uses last-match |
| `cli/permission-profiles.mdx` | Config.toml persistence shape | Doc-only mismatch | `src/runtime/context/config/permission-profiles.ts:53-66` | Actual section is `[permission_profiles]`, rules are table not array |
| `cli/permission-profiles.mdx` | `gizzi exec --permission-profile` | Missing | `src/cli/commands/run.ts:338-342` | Not implemented |
| `cli/ci-mode.mdx` | `gizzi exec` in CI | Implemented | `src/cli/commands/run.ts:988-998` | — |
| `cli/ci-mode.mdx` | `ALLTERNIT_API_KEY` env auth | Partial / Misleading | no reference | Requires profile |
| `cli/ci-mode.mdx` | `gizzi config profile add --rule` | Implemented | `src/cli/commands/config.ts:53-69` | — |
| `cli/ci-mode.mdx` | `gizzi exec --permission-profile` | Missing | `src/cli/commands/run.ts:228-401` | Not implemented |
| `cli/ci-mode.mdx` | `--output-format stream-json` | Partial | `src/cli/commands/run.ts:589,599-601` | `jq` example mismatches event schema |
| `cli/ci-mode.mdx` | Exit codes `0`/`1`/`2` | Partial | — | Code `2` not produced |
| `cli/allternit-wrapper.mdx` | `bin/allternit` wrapper | Implemented | `bin/allternit:1-49`, `src/cli/allternit.ts` | — |
| `cli/allternit-wrapper.mdx` | `allternit tui` | Implemented | `src/cli/allternit.ts:42-51` | — |
| `cli/allternit-wrapper.mdx` | `allternit up`/`down` | Implemented | `src/cli/allternit.ts:53-68`, `src/cli/platform/daemon.ts` | — |
| `cli/allternit-wrapper.mdx` | `allternit status` | Partial | `src/cli/allternit.ts:70-80`, `src/cli/platform/daemon.ts:100-114` | Omits gateway service |
| `cli/allternit-wrapper.mdx` | `allternit doctor` | Partial | `src/cli/allternit.ts:82-93`, `src/cli/platform/daemon.ts:119-157` | Checks Cargo/Bun/Platform, not api/gateway/local-models |
| `cli/allternit-wrapper.mdx` | `allternit logs` | Implemented | `src/cli/allternit.ts:95-98` | — |
| `cli/allternit-wrapper.mdx` | Config keys (`api.bind_address`, `kernel.url`, etc.) | Missing | `src/cli/ui/ink-app/utils/settings/types.ts` | Not consumed by gizzi-code; ports hardcoded |
| `cli/commrails-cli.mdx` | `allternit-rails` / `rails` binaries | Separate package | `rails/src/bin/allternit-rails.rs`, `rails/cli/src/main.rs` | Not in gizzi-code |
| `cli/commrails-cli.mdx` | gizzi-code `ListPeers`/`SendMessage` runtime tools | Implemented | `src/runtime/tools/ListPeersTool/ListPeersTool.ts:53`, `src/runtime/tools/SendMessageTool/SendMessageTool.ts:103`, `src/runtime/gizzi-core/services/railsPeer.ts:45` | — |
| `cli/agent-daemon.mdx` | Agent daemon process | Separate package | `cmd/agent-daemon/src/index.ts` | Not in gizzi-code |
| `cli/agent-daemon.mdx` | gizzi-code runtime pairing client | Implemented | `src/runtime/services/pairing/pairing.ts` | Separate from daemon |
| `cli/allternit-mux.mdx` | `allternit-factory pane tty` (mux folded into the Factory pane engine) | Separate package | `factory/pane/src/factory_terminal.rs` | Not in gizzi-code |
| `cli/allternit-mux.mdx` | gizzi-code PTY integration | Implemented | `src/runtime/integrations/pty/index.ts` | Starts the pane engine (`pane tty ensure`) and speaks `factory.terminal.*` |
| `cli/cli-typescript.mdx` | `@allternit/cli-typescript` | Separate package | `cmd/cli-typescript/cli/` | Not in gizzi-code |

### Core concepts (`surfaces/docs/core/*.mdx`)

| Doc page | Feature | Status | Evidence | Gap note |
|----------|---------|--------|----------|----------|
| `core/gizzi-core.mdx` | Package `@allternit/gizzi-core` | Missing / Doc-only | `cmd/gizzi-code/package.json:5` | Package is named `@allternit/gizzi-code`; no `gizzi-core` package |
| `core/gizzi-core.mdx` | Default exports (`Bus`, `Workspace`, `Verification`, `ShimmeringBanner`) | Missing | `cmd/gizzi-code/src/index.ts` | Not exported |
| `core/gizzi-core.mdx` | Subpath exports (`/brand`, `/bus`, `/workspace`, `/continuity`, `/verification`) | Missing | `package.json:50-52` | Wildcard `./*` only; no dedicated entries |
| `core/gizzi-core.mdx` | `GIZZIBrand` / `GIZZICopy` | Implemented | `src/runtime/brand/meta.ts:4`, `src/runtime/brand/copy.ts:27` | — |
| `core/gizzi-core.mdx` | `Bus.publish/subscribe/once/subscribeAll` | Implemented | `src/shared/bus/index.ts:46-113` | — |
| `core/gizzi-core.mdx` | `Bus.stats()` / `Bus.clear()` | Missing | — | Not implemented on typed Bus |
| `core/gizzi-core.mdx` | `WorkspaceInitialized` event | Missing | — | Not found |
| `core/gizzi-core.mdx` | `Workspace` methods (`init`, `readFile`, `writeFile`, `resolvePath`, `exists`) | Implemented | `src/runtime/workspace/workspace.ts` | — |
| `core/gizzi-core.mdx` | `Workspace.localPath()` / `Workspace.listFiles()` | Missing | `src/runtime/workspace/workspace.ts` | Not implemented |
| `core/gizzi-core.mdx` | `Verification` namespace API | Missing | `src/runtime/verification/` | Visual verification subsystem exists, not the documented primitive API |
| `core/gizzi-core.mdx` | `ShimmeringBanner onComplete` prop | Partial | `src/cli/ui/components/gizzi/shimmering-banner.tsx:48` | Component exists but accepts no props |
| `core/gizzi-runtime.mdx` | `Agent` class from `@allternit/core` | Separate package | no `@allternit/core` import | Not in gizzi-code |
| `core/gizzi-runtime.mdx` | Runtime versions (Pro/Fast/Local) | Missing / Doc-only | — | Not implemented |
| `core/gizzi-runtime.mdx` | Agent lifecycle states | Partial | `src/runtime/session/status.ts:7-22` | Only `idle`/`retry`/`busy`; missing `connecting`/`ready`/`suspended`/`disposed` |
| `core/gizzi-runtime.mdx` | Sandboxing levels (`none`/`soft`/`strict`/`container`) | Partial | `src/runtime/integrations/shell/sandbox.ts:30-45` | Uses `SandboxPolicy` object, no named levels or Docker container |
| `core/gizzi-runtime.mdx` | `agent.memory.set/get/clear` API | Missing | `src/runtime/tools/builtins/memory-write.ts`, `memory-recall.ts` | Tools exist but no object API |
| `core/gizzi-runtime.mdx` | Model config `provider`/`model`/`temperature` | Partial | `src/runtime/providers/provider.ts:81-169`, `src/runtime/session/llm.ts:111-116` | Generic config works; `allternit`/`gizzi-pro` not present |
| `core/gizzi-runtime.mdx` | Local providers (Ollama, MLX, etc.) | Implemented | `src/runtime/providers/discovery/local.ts`, `src/runtime/providers/adapters/loaders/local.ts`, `src/cli/commands/provider.ts:84-89` | Bonsai WebGPU is image-gen only, not text LLM |

### Tools & ACI (`surfaces/docs/tools/*.mdx`)

| Doc page | Feature | Status | Evidence | Gap note |
|----------|---------|--------|----------|----------|
| `tools/tool-belt.mdx` | `NativeToolBelt` / `ToolRegistry` | Separate package | `sdk/allternit-sdk/src/ai-runtime/tools/search.ts` | Not in gizzi-code |
| `tools/tool-belt.mdx` | REST `/api/v1/tools`, `/api/tools/execute`, approvals | Separate package | `cmd/allternit-api/src/tool_routes.rs`, `cmd/allternit-api/src/main.rs:391-398` | Not in gizzi-code |
| `tools/tool-belt.mdx` | `tool_search` / `tool_activate` | Separate package | `sdk/allternit-sdk/src/ai-runtime/tools/search.ts` | gizzi-code has unrelated `ToolSearchTool` |
| `tools/tool-belt.mdx` | `web_search` | Partial | `src/runtime/tools/builtins/websearch.ts:40-64` | Different name/schema; calls Exa |
| `tools/tool-belt.mdx` | `web_fetch` | Partial | `src/runtime/tools/builtins/webfetch.ts:11-20` | Different name/schema; 5 MB cap |
| `tools/tool-belt.mdx` | `str_replace_editor` | Partial | `src/runtime/tools/builtins/read.ts`, `write.ts`, `edit.ts` | Split into separate tools |
| `tools/tool-belt.mdx` | `bash` | Partial | `src/runtime/tools/builtins/bash.ts:60-82` | Different timeout units/options |
| `tools/tool-belt.mdx` | `code_execution` | Missing | `src/runtime/tools/REPLTool/constants.ts` | Only stubs exist |
| `tools/tool-belt.mdx` | `memory` | Partial | `src/runtime/tools/builtins/memory-write.ts`, `memory-recall.ts` | Persistent frontmatter tools, not session key/value |
| `tools/tool-belt.mdx` | `computer` | Implemented (bridge) | `src/runtime/tools/builtins/browser.ts` | Uses ACU gateway |
| `tools/mcp.mdx` | `/mcp/connectors`, `/mcp/servers`, `/mcp/test`, `/mcp/oauth/callback` REST | Separate package | `cmd/allternit-api/src/mcp_routes.rs` | Not in gizzi-code |
| `tools/mcp.mdx` | `/mcp/server` JSON-RPC MCP server surface | Separate package | `cmd/allternit-api/src/mcp_server_routes.rs` | Not in gizzi-code |
| `tools/mcp.mdx` | Tunnel auth headers / `-32001` | Separate package | `cmd/allternit-api/src/mcp_tunnel_auth.rs` | Not in gizzi-code |
| `tools/mcp.mdx` | gizzi-code as MCP client (stdio/SSE/HTTP) | Implemented | `src/runtime/tools/mcp/index.ts` | — |
| `tools/mcp.mdx` | Bundled MCP servers | Implemented | `src/runtime/tools/mcp/bundled.ts` | — |
| `tools/strict-tool-use.mdx` | `registry.registerTool(tool, { strict: true })` | Separate package | `sdk/allternit-sdk/src/ai-runtime/tools/registry.ts` | gizzi-code bundled SDK lacks `strict` option |
| `tools/strict-tool-use.mdx` | `NativeToolBelt` strict-by-default | Separate package | `sdk/allternit-sdk/src/ai-runtime/tools/search.ts` | Not in gizzi-code bundled SDK |
| `tools/strict-tool-use.mdx` | Schema normalization / requirements | Separate package | `sdk/allternit-sdk/src/ai-runtime/tools/schema.ts` | Not in gizzi-code |
| `tools/strict-tool-use.mdx` | gizzi-code legacy builtins with `strict: true` | Partial | `src/runtime/tools/builtins/bash/BashTool.tsx:427`, `FileEditTool.ts:91`, etc. | Subset only |

---

## Features implemented by separate packages (out of gizzi-code scope)

These are documented on the CLI/docs site but owned by other packages. They should not be added to `cmd/gizzi-code` during cleanup unless the architecture changes.

| Feature | Owner | Evidence |
|---------|-------|----------|
| `NativeToolBelt`, REST tool execution, approval endpoints | `allternit-sdk` + `allternit-api` | `sdk/allternit-sdk/src/ai-runtime/tools/`, `cmd/allternit-api/src/tool_routes.rs` |
| `/mcp/*` REST connector/server directory and `/mcp/server` JSON-RPC | `allternit-api` | `cmd/allternit-api/src/mcp_routes.rs`, `cmd/allternit-api/src/mcp_server_routes.rs` |
| `allternit-rails` / `rails` CLI | `allternit-agent-system-rails` | `rails/src/bin/allternit-rails.rs`, `rails/cli/src/main.rs` |
| `allternit-agent-daemon` | `cmd/agent-daemon/` | `cmd/agent-daemon/src/index.ts` |
| `allternit-factory` pane engine (was `allternit-mux`) | `factory/pane/` | `factory/pane/src/factory_terminal.rs` |
| `@allternit/cli-typescript` | `cmd/cli-typescript/cli/` | `cmd/cli-typescript/cli/package.json` |

---

## Recommendations

1. **Fix the highest-impact doc/implementation mismatches first** (P0/P1 gaps above). These are user-visible: wrong config schema, missing env vars, missing `--permission-profile`, missing `--base-url`.
2. **Decide on package boundaries** — `gizzi-core.mdx`, `gizzi-runtime.mdx`, `tool-belt.mdx`, `mcp.mdx`, `strict-tool-use.mdx` describe surfaces that are not in `cmd/gizzi-code`. Either move those pages to package-specific sections or create the missing packages/exports.
3. **Consolidate the multiple profile systems** — gizzi-code has `gizzi config profile`, `gizzi permission-profile`, and `gizzi profile`. Pick one, remove the others, and make the docs match.
4. **Remove or implement stubs** — `keyring` credential store and `code_execution` tool are documented or implied but not fully implemented. Either finish them or remove the claims.
5. **Standardize env-var naming** — settle on `ALLTERNIT_API_KEY` vs provider-specific keys, `GIZZI_CONFIG_PATH` vs `GIZZI_CONFIG`, `ALLTERNIT_ENDPOINT` vs `ALLTERNIT_API_URL`.
6. **Update `allternit` wrapper** — make `status`/`doctor` output match docs, or update docs to reflect actual behavior.
7. **Run this audit again after cleanup** — once stubs are implemented and dead code is removed, re-verify every P0/P1 item.
