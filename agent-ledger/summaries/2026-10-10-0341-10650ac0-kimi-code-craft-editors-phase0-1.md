# Craft editors Phase 0+1 — vendored PDF/Image/Video artifact editors (session 10650ac0)

## What landed
- Vendored PhotoCraft/PdfCraft/FilmCraft (storytold, MIT/Apache-2.0) into `vendor/craft/{image,pdf,video}` at pinned revs 2026-10-09, rebranded per the trademark clause; `artcraft` engine repos excluded (fair-source). Tier-C audit: `vendor/craft/AUDIT.md` PASS-with-conditions (cargo audit findings not reachable in the embed posture; zero telemetry in any tree).
- `vendor/craft/craft-host`: craft:1 host-page bridge (protocol, auth state machine, wasm transport; 8/8 tests). Embed adapters in all three web apps behind `embed` cargo features (video gates window.filmcraft; pdf adds the save write-back; image feeds the control channel).
- Office surface serves release WASM bundles at `/craft/{pdf,image,video}/` (22.7/24.1/22.0 MiB, craft:1 verified, CSP/COOP/COEP headers — moved to the Pages root `_headers` in #1475 after the first deploy silently ignored the subdirectory copy).
- `pdf` artifact kind end-to-end: cloud-api allow-list (platform #1472), app registry + TS craft:1 bridge client + CraftEditor with save→versions (allternit-ai #495, merged c9c23112).

## PRs
allternit-ai #495 (merged) · platform #1472, #1473, #1475 (merged). Foundation branch session/craft-editors-20261009.

## Deferred
image/video artifact kinds, `studio_command` agent co-editing, video xtask feature plumbing in CI, PWA cache/deploy, README screenshot regen, pre-existing gitleaks findings in agent-ledger/ (rotate/scrub — surfaced, not ours).

## Incidents/notes
GitHub Actions intermittently blocked (billing/spending limit) — used local verification + `--admin` after conflict resolution. Machine load discipline: serial cargo after a load-124 parallel-build mistake.
