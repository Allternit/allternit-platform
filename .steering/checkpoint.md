# Steering checkpoint

**Session:** `session/craft-video-kind-20261010` · worktree `allternit-wt-craft-video` (+ ai worktree `allternit-ai-wt-craft-video`) · plan dag_724767 (gizzi node commands error — noted below)
**Date:** 2026-10-10 · **State: PRs OPEN, awaiting Eoj's review/merge (human gate — do not merge)**

## Goal
Craft editors item 2, app-independent slice — COMPLETE. Footage refs, OPFS, media_generate, phone view+trim, host save/open plumbing, agent co-editing, Phase-4 cleanup remain for later slices per the handoff.

## Just did (final state)
- **Platform PR #1480** (branch `session/craft-video-kind-20261010`, head 4d76cd1f46): video KindSpec in kinds.rs (cargo check ✅, kinds tests 5/5 ✅); runtime.rs gate verified already-open (no change); VENDOR.md CRAFT_FEATURES note; clean CRAFT_FEATURES=embed rebuild published to office surface (craft:1 ✅ ×7 strings, 0 files ≥25 MiB, wasm-opt v133 needed NO patch); docs (pdf+video rows, gap closed); features.json decision + pre-existing #1477 validate break fixed (`--validate` clean); check_links 0 problems.
- **ai PR #500** (branch `session/craft-video-kind-20261010`, head 9fbd3b67): ARTIFACT_KINDS + KIND_REGISTRY video entry (interim V.code rendering, Editor null) + KIND_ICON; typecheck ✅; vitest 57/57 ✅; SW guard "No Fabric Session asset changes." ✅; impact query recorded. No public/craft mirror (image/pdf never were; bridge is cross-origin to office.allternit.com) — flagged in PR body.
- Lifecycle steps 5–9 (merge, sync, attest, desktop rebuild, worktree cleanup) deliberately NOT run: Eoj merges, per the handoff's DO-NOT-MERGE instruction. git-discipline-check on the shared checkout is expected-red from other sessions (pre-existing, not mine to fix).

## Next (after Eoj merges both PRs)
1. Deploys per the runbook (Eoj gate): cloud-api → office.allternit.com Pages (craft/video bundle) → ai.allternit.com manual wrangler → m.allternit.com PWA + SW bump (only when user-facing video work ships).
2. Next slice: FilmCraft host open/save plumbing (VideoCraftEditor), footage refs, OPFS, media_generate, phone view+trim.
3. Ledger attestation for this session when merging (lifecycle step 7).

## Open questions
- Should craft bundles ever be mirrored into allternit-ai public/ (Desktop offline concern)? Image/pdf never were; needs Eoj's call, not this session's invention.
- gizzi `workspace node add|claim|close` all fail with yargs "unexpected argument" on gizzi 2.1.9 — plan dag_724767 exists but node-level tracking was impossible; used the session TodoList instead. Worth a tooling issue.
