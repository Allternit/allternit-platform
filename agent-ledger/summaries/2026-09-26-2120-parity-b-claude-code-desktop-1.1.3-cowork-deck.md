# Parity Agent B — Desktop 1.1.3 install + Cowork top deck (Claude Code, 2026-09-26)

## Done
- **Desktop 1.1.3 b3705** (local unsigned, from main + #781) installed to `/Applications/Allternit Desktop.app` from the DMG. `~/Applications/Allternit Desktop Preview.app` deleted after owner OK. b3704 artifacts pruned with `build-state.sh --prune`. Installed UI checked to be clean main (no Cowork WIP strings).
- **PR allternit-platform #784**: explicit `ALLTERNIT_BUILD_SUFFIX=-b<N>` now sets CFBundleVersion `<ver>.<N>` (was `Date.now()`); `build-state.sh` also reads the bundled `build-info.json` suffix. Verdict went from a false "install it" to clean.
- **PR allternit-ai #91**: Cowork top deck: plain text triggers + Radix menus; Output (Document/Presentation/Design/Artifact) + design system become the session's real mode contract. Also fixes a TS2367 in ModelPickerPopover that broke `pnpm typecheck` on main.

## Verification
- `POST /v1/skill/reload` on the installed sidecar → 200 `{"count":57}`.
- Skill catalog install via the app (CDP, `/api/v1/skills/catalog/install`, `codex-clone-website`) → `agentReloaded:true`, gizzi lists it; uninstall → `agentReloaded:true`, gone. Test skill removed.
- Bundled `Resources/bin/vendor/ripgrep/arm64-darwin/rg` present (joe-61's ask).
- allternit-ai: typecheck clean; vitest 460 files / 3719 tests pass; `vite build` ok.

## Incidents
- Quitting Preview.app (1.1.1 build 3703, pre-#770) left 8 sidecars orphaned under launchd; killed by hand. The installed 1.1.3 has the #770 lifeline watchers on every sidecar. I did not re-test quit-reaping on 1.1.3 because another session was using the app.
- Another session relaunched the installed app on CDP 9223 mid-verification. The first probe of `/v1/skill/reload` hit the earlier instance and 404'd; the probe against the current instance returned 200.

- **My attestation commit 850a31905 also committed the shared checkout's stale staged index**: `git add <ledger files>` + `git commit` took everything already staged, reverting 86 files of merged work. joe-33 restored them in platform #787 (ed7c6ebb1); I checked each file: 0 still reverted, and my LEDGER.md change was only my own line. Rule going forward: never commit in the shared checkout; make direct-on-main ledger commits from a clean worktree on origin/main, and check `git diff --cached --stat` first.

## Deferred
- #91 not verified live in Desktop — needs merge + next full Desktop build.
- Rest of W6: session title menu, right panel, tool toggles, Settings → Cowork; W5 Bot Schedule entry; W8 ACI icon; coordinator audit spec; ACI specs.
