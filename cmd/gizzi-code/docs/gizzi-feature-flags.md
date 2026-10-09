# gizzi-code compile-time feature flags

gizzi-code inherited 74 `feature('X')` flags (`import { feature } from 'bun:bundle'`). A flag that is not passed to the bundler evaluates to `false`, and its gated code is removed from the binary. Until 2026-09-26 no build passed any flags, so every gated feature was off.

- **Source of truth:** `script/features.mjs`, which is read by `script/build-production.js`, `bun run dev`/`start` (`script/dev.mjs`), and `script/ci-smoke-test.sh`.
- **Runtime gates:** many flags also check a `tengu_*` GrowthBook gate. gizzi has no flag server, so a gate falls back to its call-site default, usually `false`. `src/constants/gizziGates.ts` holds gizzi's own defaults for the gates behind enabled flags. A remote value or an override still wins.
- **Experiments:** `GIZZI_FEATURES_EXTRA=FOO,BAR` adds flags to one build or dev run without editing the list.

## Enabled (20)

| Flag | What it turns on | Notes |
|---|---|---|
| AWAY_SUMMARY | "While you were away" recap after 5 minutes of terminal blur | Gate `tengu_sedge_lantern` defaults to true. Needs focus reporting (DECSET 1004). |
| MESSAGE_ACTIONS | shift+↑ message cursor | Fullscreen layout only. |
| HISTORY_PICKER | ctrl+r opens the prompt-history picker | |
| QUICK_SEARCH | ctrl+shift+p quick open, ctrl+shift+f global search | cmd+ bindings need a kitty-protocol terminal. |
| AUTO_THEME | "Auto (match terminal)" theme | |
| ULTRATHINK | `ultrathink` keyword | |
| TOKEN_BUDGET | `+500k` per-turn token budget syntax | |
| MCP_RICH_OUTPUT | Formatted MCP tool output | |
| NATIVE_CLIPBOARD_IMAGE | Fast macOS clipboard image check | Falls back to osascript. |
| HOOK_PROMPTS | Hooks can ask the user a question | |
| SHOT_STATS | Shot distribution in /stats | |
| COMPACTION_REMINDERS | Compaction reminder attachments | |
| BUILTIN_EXPLORE_PLAN_AGENTS | Explore and Plan subagents | |
| FORK_SUBAGENT | Fork subagent, `/fork <directive>` | `/fork` was ported 2026-09-28: it starts a background fork through the Agent tool's fork path, so the fork shares the prompt cache and reports back with a task notification. `/branch` copies the conversation into a new session. |
| AGENT_TRIGGERS | CronCreate / CronDelete / CronList | The tool wrappers were missing and were written 2026-09-26 on top of the existing scheduler. |
| EXTRACT_MEMORIES | Background memory extraction after turns | Gate `tengu_passport_quail` defaults to true. Skips turns where the agent already wrote memory. |
| TRANSCRIPT_CLASSIFIER | Auto mode | The classifier prompts were missing and were authored in `utils/permissions/yolo-classifier-prompts/`. Works with whichever model is chosen: the classifier runs on the main model, through the Anthropic API or the model's provider (OpenRouter, local servers) with reasoning switched off. First entry shows a consent dialog. |
| PET | Terminal pet sprite, `/pet` | `/pet` and the per-turn observer were missing and were written 2026-09-26. Uses the small model. |
| TERMINAL_PANEL | meta+j opens a persistent shell; meta+j inside it returns | Gate `tengu_terminal_panel` defaults to true. tmux-backed, one server per session on its own `gizzi-panel-*` socket, killed on exit; without tmux it runs a one-off shell. Works when gizzi itself runs inside tmux. Checked live 2026-09-28. |
| TREE_SITTER_BASH | AST-based bash permission checks | The parser is pure TypeScript (`utils/bash/bashParser.ts`), no wasm. Commands it can't model statically (e.g. `$(...)`, `eval`, loops) fail closed to a permission prompt; read-only commands still run without one. Checked live 2026-09-28. |

## Off, needing Anthropic services

BRIDGE_MODE, CCR_AUTO_CONNECT, CCR_MIRROR, CCR_REMOTE_SETUP, DAEMON (claude.ai remote control / cloud sessions) · ULTRAPLAN (cloud planning) · TEAMMEM (Anthropic team-memory sync) · UPLOAD_USER_SETTINGS, DOWNLOAD_USER_SETTINGS (settings sync) · KAIROS, KAIROS_BRIEF, KAIROS_CHANNELS, KAIROS_GITHUB_WEBHOOKS, KAIROS_PUSH_NOTIFICATION, PROACTIVE (assistant mode, which also has missing modules) · AGENT_TRIGGERS_REMOTE (RemoteTrigger tool, missing) · ANTI_DISTILLATION_CC, NATIVE_CLIENT_ATTESTATION (Anthropic API protections) · CONNECTOR_TEXT (Anthropic API beta) · LODESTONE (claude-cli:// deep links) · CHICAGO_MCP (Anthropic computer-use MCP; code removed 2026-10-09, Allternit uses its own computer toolset).

## Off, internal, debug, or telemetry

ALLOW_TEST_VERSIONS, BREAK_CACHE_COMMAND, HARD_FAIL, FILE_LOGGING, SLOW_OPERATION_LOGGING, PERFETTO_TRACING, PROMPT_CACHE_BREAK_DETECTION, UNATTENDED_RETRY, SKIP_DETECTION_WHEN_AUTOUPDATES_DISABLED, COWORKER_TYPE_TELEMETRY, ENHANCED_TELEMETRY_BETA, MEMORY_SHAPE_TELEMETRY, TORCH, TEMPLATES (job classifier), SKILL_IMPROVEMENT, AGENT_MEMORY_SNAPSHOT, VERIFICATION_AGENT, NEW_INIT (gizzi has its own /init direction).

## Off, incomplete or superseded here

- BASH_CLASSIFIER, POWERSHELL_AUTO_MODE: `bashClassifier.ts` is an internal-only stub, and auto mode runs on TRANSCRIPT_CLASSIFIER alone.
- COORDINATOR_MODE, WEB_BROWSER_TOOL, HISTORY_SNIP, MCP_SKILLS, EXPERIMENTAL_SKILL_SEARCH: their tool or command modules were never ported.
- REACTIVE_COMPACT, CONTEXT_COLLAPSE, BG_SESSIONS: context-management and background-session experiments.
- TREE_SITTER_BASH_SHADOW: telemetry-only comparison of the AST and legacy checks; TREE_SITTER_BASH is on instead.
- VOICE_MODE: Claude Code voice uses Anthropic's voice endpoint. Allternit voice has its own architecture.
- IS_LIBC_GLIBC, IS_LIBC_MUSL: per-target build facts, not features. No musl target is built.

## Turning a flag on

1. Check that the gated modules exist and load through static `require('./literal/path.js')`. `safeRequire(path)` hides the path from the bundler, so the module comes back `null` in compiled binaries.
2. Give any `tengu_*` gate it depends on a default in `src/constants/gizziGates.ts`.
3. Add it to `script/features.mjs`, run `bun run dev`, and check it live in a real session.
