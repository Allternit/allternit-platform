# Attestation — gizzi TUI parity (Claude Code): feature flags, 2.1.4 release PR, bubble/layout + display fixes

**Date:** 2026-09-26 → 2026-09-27 · **Agent:** Claude Code (Opus 5.5) · **Branches:** session/restore-850a31905, session/gizzi-tui-flags, session/gizzi-2.1.4-release, session/gizzi-tui-display

## What was done

Continues the owner-requested 1:1 TUI parity audit of gizzi-code against Claude Code (see the 2.1.3 attestation). Handoff: `allternit-workspace/HANDOFF-gizzi-tui-parity-2026-09-26.md`.

| PR | Merge | What |
|----|-------|------|
| #787 | ed7c6ebb1 | Restored 76 files reverted by ledger commit 850a31905 (made from the shared checkout's stale index) |
| #788 | b01818954 | 18 compile-time feature flags on (`script/features.mjs`, gate defaults in `constants/gizziGates.ts`, all 74 triaged in `docs/gizzi-feature-flags.md`); cron tools, auto-mode classifier prompts, `/buddy` observer/soul written; TUI never called `setIsInteractive(true)`; `@ts-nocheck`-hidden undefined names in bash permissions/toolSearch/api/sessionStorage; side calls routed through the provider path; `small_model` honored; static requires for compiled binaries |
| #790 | 5b32532ff | 2.1.4 version + CHANGELOG (**not tagged** — owner deferred release steps); `bunfig.toml` restored on every build exit path; CI smoke red on main since #788 fixed (test reassigned `process.env`, detaching it — snapshot suite's `GIT_CONFIG_GLOBAL` never reached git); one malformed Claude Code plugin frontmatter (`argument-hint: [system] [--source <path>]`) no longer fails every plugin; buddy bubble overlap fixed (below) |
| #792 | open | Diffs/highlighted files follow terminal color depth (color-diff port hard-coded 24-bit); Claude Code diff gutter (`2 -`); wrapped user prompt aligns under text |

**Buddy bubble root cause (#790):** not a one-frame glitch — footer text painted under/through the bubble for ~5 s. The yoga TS port's multi-entry layout cache (w/h only) was consulted on layout passes, so a hit skipped child positioning and left the footer at its pre-bubble positions. Restricted to measure passes (the `_hasL` slot still covers same-input layout hits). Also: prompt column flex-basis 0 in row mode (auto basis came from `width="100%"` children resolving against the previous width); cwd badge no longer shares shrink.

## Verification evidence
- Compiled darwin-arm64 binary (CI build steps, sha 1046ba101, isolated `GIZZI_CONFIG_DIR`): `/buddy` hatch, Ctrl+R picker, CronCreate one-shot fired on schedule.
- bunfig: forced worker-bundle failure (hid `packages/sdk/dist`) → exit 1, `bunfig.toml` present.
- Smoke: bisected the failing test to `tui-feature-flags.test.ts`; after fix 1354/0 locally, CI green. #792: 1372/0 over 114 entries; tsc clean.
- Bubble: 50 ms tmux frame captures at 140 and 90 cols plus fullscreen; `YOGA_NOCACHE` A/B isolated the layoutNode cache; caught and fixed a self-introduced regression (flex-basis 0 collapsed the prompt in the narrow column layout).
- Display: side-by-side with Claude Code 2.1.283 on one prompt under an Apple Terminal env; after #792 the gizzi capture has 0 truecolor escapes.

## Known gaps / remaining work
- **2.1.4 not released** (tag, npm/GitHub, Homebrew tap, packaging hashes, Desktop sidecar, prune) — owner said not now.
- Owner decisions pending: boxed tool card vs Claude Code's `⏺ Update(file)`; per-turn telemetry line vs Claude Code's always-on `✻ Churned for Ns · done <time>` (gizzi shows its done line only >30 s); dimmed streaming reveal.
- Mascot redesign (options first), auto mode live test (no Anthropic key), remaining flags, Aliyun key for `small_model`.
- `@ts-nocheck` sweep (593 files): 9 undefined names left, all unreachable (ant-only / ULTRAPLAN off); Spinner's is kept verbatim per the codemod hand-patch rule — left as is.
