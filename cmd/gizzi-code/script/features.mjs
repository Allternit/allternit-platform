/**
 * Compile-time `feature()` flags (`import { feature } from 'bun:bundle'`).
 *
 * A flag not listed here evaluates to false and its gated code is
 * dead-code-eliminated from the bundle. This list is the single source for
 * every build (script/build-production.js) and for `bun run dev`
 * (script/dev.mjs), so the dev TUI and shipped binaries match.
 *
 * Flags that also check a runtime gate (`tengu_*`) need that gate's local
 * default in src/constants/gizziGates.ts — gizzi has no remote flag server,
 * so the gate's hardcoded default is what users get.
 *
 * Triage of all 74 flags (2026-09-26): docs/gizzi-feature-flags.md.
 */
export const FEATURES = [
  // TUI parity with Claude Code
  "AWAY_SUMMARY", // "while you were away" recap after 5 min unfocused
  "MESSAGE_ACTIONS", // shift+↑ message cursor (fullscreen layout)
  "HISTORY_PICKER", // ctrl+r opens the prompt-history picker dialog
  "QUICK_SEARCH", // ctrl+shift+p quick open, ctrl+shift+f global search
  "AUTO_THEME", // "Auto (match terminal)" theme
  "ULTRATHINK", // "ultrathink" keyword
  "TOKEN_BUDGET", // "+500k" per-turn token budget syntax
  "MCP_RICH_OUTPUT", // formatted MCP tool output
  "NATIVE_CLIPBOARD_IMAGE", // fast macOS clipboard image check (falls back to osascript)
  "HOOK_PROMPTS", // hooks can ask the user a question
  "SHOT_STATS", // shot distribution in /stats
  "COMPACTION_REMINDERS",
  "BUILTIN_EXPLORE_PLAN_AGENTS", // Explore + Plan subagents
  "FORK_SUBAGENT", // fork subagent (inherits parent context)
  "AGENT_TRIGGERS", // CronCreate / CronDelete / CronList tools
  // Owner-approved beyond the parity set (2026-09-26)
  "EXTRACT_MEMORIES", // background auto-memory extraction
  "TRANSCRIPT_CLASSIFIER", // auto mode
  "PET", // terminal pet (/pet)
  // Checked live 2026-09-28
  "TERMINAL_PANEL", // meta+j persistent shell panel (tmux-backed)
  "TREE_SITTER_BASH", // AST-based bash permission checks (pure-TS parser)
];

/** Extra flags for one-off experiments: GIZZI_FEATURES_EXTRA=FOO,BAR */
export function resolveFeatures(env = process.env) {
  const extra = (env.GIZZI_FEATURES_EXTRA ?? "")
    .split(",")
    .map((f) => f.trim())
    .filter(Boolean);
  return [...new Set([...FEATURES, ...extra])];
}
