# Production release: gizzi-code v2.1.2 (DONE) + desktop v1.1.2 (CI IN FLIGHT)

Session: gizzi-tui-parity closeout. Context/quota stop — desktop install steps
staged below for the next session.

## DONE
- CLI v2.1.2 published via CI (quality gates + 5 platform binaries + GitHub
  release + npm all green, run 36270290512).
- homebrew-tap Formula/gizzi-code.rb bumped to 2.1.2 with CI checksums
  (commit aaf76b0c59 in Gizziio/homebrew-tap); `brew upgrade` installed it
  locally; verified 2.1.2 running with P0–P11 code (strings grep).
- Release blockers fixed on main: welcomeArt.ts stray @ts-nocheck (burn-down
  guard 500>499, ef54fff08), lume x64 404 tolerated in prepare-lume.cjs +
  hosted-ui rm-guard (0777306df), Windows NTFS junction in build-hosted-ui
  action (a2ff240f8 — git-bash ln -s cannot create dir links on Windows
  runners; mklink /J needs no privilege).
- Six duplicate Allternit Desktop instances killed (~5GB RAM freed); user
  confirmed duplicates were a bug. Desktop app currently NOT running.
- Tags: gizzi-code/v2.1.2 = 0777306df (published). desktop-v1.1.2 = a2ff240f8.

## IN FLIGHT — finish me
- Desktop release run **36272297013** (tag desktop-v1.1.2 @ a2ff240f8).
  At handoff: unit tests/preflight/Linux green; Windows past the old failure
  point; macOS compiling allternit-api. Check: `gh run view 36272297013
  --repo Gizziio/allternit-platform`.
- WHEN GREEN:
  1. `gh release download desktop-v1.1.2 --repo Gizziio/allternit-platform
     --pattern "*arm64.dmg" --dir /tmp` (or fetch asset URL from the run).
  2. Replace local preview: delete repo-local
     surfaces/allternit-desktop/release/Allternit-Desktop-1.1.1-b3704-arm64.dmg
     + .blockmap (unsigned local build, superseded).
  3. hdiutil attach DMG; cp -R "Allternit Desktop.app" /Applications/
     (unsigned?? NO — CI build is signed/notarized via Apple secrets).
  4. DELETE DUPLICATES: rm -rf ~/Applications/"Allternit Desktop.app"
     (and any other copies outside /Applications) — multi-instance bug source.
  5. Relaunch: open -a "Allternit Desktop". Verify new version + that the
     bundled sidecar contains P7-P11 code (strings grep for
     getRemoteMemoryDirOverride / "quota n/a").
  6. If the run FAILED: diagnose via `gh run view 36272297013 --log-failed`;
     tags can be moved again (desktop-v1.1.2 is unpublished until release
     assets land; desktop-v1.1.1 lock is untouched).
- Ledger: append one-line entry to agent-ledger/LEDGER.md when install lands.

## Incidents
- Release workflow had been red on main since post-v1.1.1 merges (2a7d739a1
  lume staging, b345b3140 hosted-ui action) — rule-6 breakage found only when
  cutting this release. All fixed; workflow green as of this run (pending
  macOS/Windows completion).
- brew caveat: Cellar 2.1.2 is now the CI build; `brew upgrade` is safe again.
