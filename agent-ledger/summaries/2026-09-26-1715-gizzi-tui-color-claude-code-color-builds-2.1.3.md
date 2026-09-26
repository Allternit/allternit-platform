# Attestation — session/gizzi-tui-color (Claude Code): TUI color, own config, build hygiene, gizzi-code 2.1.3

**Date:** 2026-09-26 · **Agent:** Claude Code (Opus 5.5) · **Branches:** session/gizzi-tui-color, session/gizzi-release-2.1.3, session/gizzi-213-followup, session/build-state-unreleased

## What was done

Started from an owner-requested audit of gizzi-code's TUI against Claude Code, then the owner's "one current build" rule.

| PR | Merge | What |
|----|-------|------|
| #762 | e79d9a639 | Color detection follows supports-color (Apple Terminal < macOS 26 got 24-bit SGR it can't render → gray TUI); welcome box theme keys instead of hardcoded sand; footer keeps the permission-mode hint whole; "Not logged in" hidden for provider-prefixed models |
| #763 | 46c7e5fc2 | gizzi's global config → `<GIZZI_CONFIG_DIR ?? ~/.gizzi>/.config.json` (was Claude Code's `~/.claude.json`); owner chose "start fresh", no import |
| #773 | 620100917 | `scripts/build-state.sh` (report + `--prune`), `.steering/bin/guard-build.sh` PreToolUse guard (all CLIs), `GIZZI_BUILD_SHA` embedded in binaries, AGENTS.md "one current build per product" commandment |
| #774 | 31c440e99 | build-state sees processes/apps with spaces in paths (Preview app sidecars) |
| #776 | 467752a5d | Glob/Grep fixed in shipped binaries (ripgrep resolver re-spawned gizzi itself); vendored rg shipped in Desktop release + npm packages; preflight requires rg; version 2.1.3 + CHANGELOG |
| #780 | 9b74d9a16 | `update-packaging-hashes.sh` moves versions (not just hashes) for homebrew/scoop/arch/rpm/deb/winget; manifests at 2.1.3 with ripgrep deps; `-dirty` flag counts tracked source only |
| #782 | 463dcd8bc | build-state counts only binary-changing unreleased commits |

**Release gizzi-code 2.1.3:** tag `gizzi-code/v2.1.3` @ 467752a5d; publish run 36275021410 green (first attempt failed one flaky test, see Incidents); npm `@allternit/gizzi-code@2.1.3` latest (platform packages carry vendor/ripgrep); GitHub Release with 5 assets + checksums; `Gizziio/homebrew-tap` 607714f (`gizzi-code 2.1.3`, 4 sha256 from checksums.txt, `depends_on "ripgrep"`); owner machine `brew upgrade` 2.1.2 → 2.1.3 (2.1.2 keg removed), `gizzi --version` 2.1.3, embedded sha 467752a5d.

**Prune:** 16 stale gizzi binaries deleted from the shared checkout (`dist/*.pre-*` snapshots, Jul/Sep cross builds, cli-package test builds, a sidecar backup binary). Data backups in `.backup-sidecars-2026-09-26/` untouched.

**Hooks installed machine-wide** via `.steering/bin/steer-install.sh` (kimi build guard; codex had no steering hooks registered — the installer added its full standard set; gizzi plugin refreshed).

## Verification evidence
- #762: 77/77 component/color/TUI tests; live tmux TUI under an Apple Terminal env: all colors `38;5;N`, zero `38;2`.
- #763: path test 2/2; live launch creates `.config.json` in the gizzi home, `~/.claude.json` mtime unchanged.
- #773/#774: guard blocks/permits matrix (9 commands), concurrent-build block with a fake build process, `--prune` refusal while active; production build shows the embedded sha.
- #776: compiled probe (embeddedFiles > 0): rg-on-PATH and vendored layouts return files; old embedded path times out. Typecheck clean; preflight 52/0.
- #780: manifests validated (ruby -c, JSON, YAML, bash -n); script idempotent.
- Released tarball hash verified by download against checksums.txt.

## Incidents
- Orphaned test TUIs: `tmux kill-session` did not stop my `bun`/`gizzi` test processes (4 found, incl. one on the old 2.1.1 keg); all killed. The machine's tmux server has `NO_COLOR=1` in its global env and tmux sets `COLORTERM=truecolor` — both skew color checks.
- Publish run attempt 1: `test/server/mesh.test.ts` "(c) sidecar present but join fails" failed once in CI; passes 25/25 locally and in PR CI. A speculative ETXTBSY fix (symlinked fake) measured worse locally (4/58) and was dropped; job re-run green.
- Shared checkout could not fast-forward: another session's uncommitted `surfaces/allternit-desktop/src/main/unified-main.ts` conflicts with incoming main. Left untouched; this attestation lands via PR.

## Honest deferrals
- Desktop is not on 2.1.3's sidecar yet: installed `/Applications/Allternit Desktop.app` is 1.1.1 b3666 (release is desktop-v1.1.2, local b3704 DMG unverified/uninstalled) and `~/Applications/Allternit Desktop Preview.app` is a second installed copy another session patches in place. Owner decision pending.
- `mesh.test.ts` (c) flake root cause not found.
- TUI parity items still open: 73 compile-time `feature()` flags and 56 GrowthBook gates all off (auto recap etc.), mascot redesign.
