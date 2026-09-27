# Parity Agent B — competing Desktop UIs stopped, Cowork session parity (Claude Code, 2026-09-27)

## Done
- **Competing installed UIs traced and stopped.** Kimi session a9bfece2 rsync'd uncommitted UIs into the running /Applications app 5× (~21:10–22:36) and bypassed a chflags lock. Stopped at Eoj's direction (pid 21052); its WIP is left in allternit-ai-wt-coworkfix2. A Codex session Eoj started patched twice more (22:48/22:49), then ended.
- **Built and installed Desktop 1.1.3 b3886** from main (platform f91adbad3, UI b5da652c); CFBundleVersion is correct; quit-reaping verified, including after SIGKILL.
- **Merged:**
  - platform #784: build number + in-place-patch detection in build-state.sh.
  - ai #99: removed the rsync-into-app ship recipes from the plan doc.
  - ai #91 (2c21aea5a): Cowork top deck + Output mode contract; session header + title menu; Progress/Outputs/Context panel; `text-primary` → text color (the red mode switcher / tan typed text); 528px launch composer; ACI mark + toolbar; two tsc breaks on main.

## Verification
- tsc clean.
- vitest: 475 files / 3815 tests pass.
- vite build ok.
- b3886: served bundle, skill reload 200, build-state clean.

## Deferred
- #91 not built or seen live yet. Next session: full Desktop build from main, then a visual check against the Cowork/ACI references.
- The installed app's platform/ currently holds Codex's 22:49 patch (matches no build).
- The Cowork-view fix Codex was working on never reached a PR.
