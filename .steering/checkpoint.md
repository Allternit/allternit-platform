# Steering checkpoint

**Session:** `session/craft-video-kind-20261010` · worktree `allternit-wt-craft-video` (+ ai worktree `allternit-ai-wt-craft-video` at 5d26d443) · plan dag_724767 (gizzi `workspace node add|claim|close` all error with yargs "unexpected argument" — noted, TodoList tracking instead)
**Date:** 2026-10-10

## Goal
Craft editors item 2, app-independent slice (HANDOFF-craft-editors-2026-10-10): `video` kind in both registries, runtime.rs gate verify, VENDOR.md note, clean FilmCraft embed rebuild, publish to office surface, dep-map + docs, PRs both repos (NO MERGE — Eoj). OUT of scope: footage refs, OPFS, media_generate, phone view+trim, host save/open plumbing, agent co-editing, Phase-4 cleanup.

## Just did
- **Platform** (all verified):
  - kinds.rs `video` KindSpec + contract test list → `cargo check -p allternit-cloud-api` ✅ 1m33s; kinds tests 5/5 ✅.
  - runtime.rs gate VERIFIED already admitting `"image" | "video"` storage-only (line 187 + test 371-388, from PR #1477) — no change.
  - VENDOR.md CRAFT_FEATURES=embed note added.
  - features.json: Phase 3 app-independent description + 2026-10-10 decision; fixed pre-existing #1477 validate break (touches `src/routes/artifact_runtime.rs` → `src/routes/artifact_*`). `--validate` ✅ "Valid: 33 products, 28 features, 9 journeys, 1190 components, 5701 relationships, 17299 files."
  - Docs: pdf+video rows in both kind tables (pdf was missing — gap closed), prose lists, minimal true-today video paragraph in artifact-modes.mdx. check_links ✅ 0 problems (525 nav / 520 pages).
  - **FilmCraft bundle rebuilt cleanly**: `CRAFT_FEATURES=embed cargo xtask web` → 4m41s release build (craft-host compiled = embed active), wasm-bindgen 0.2.129, wasm-opt v133 accepted the xtask's existing flags — NO patch needed. Output: 23.1 MB wasm / 23.3 MB total. craft:1 strings ✅ (7 matches: "craft:1 embed mode: … authenticated bridge is the only control surface"). Size gate ✅ 0 files ≥25 MiB. Published to `surfaces/office.allternit.com/public/craft/video/` (wasm+index.html changed; js/audio-worklet byte-identical).
- **ai worktree** (all verified):
  - types.ts ARTIFACT_KINDS += 'video'; kinds.ts video entry (Viewer V.code interim, Editor null, SOURCE+COPY, ALL_PLANS, comment marks the host-wiring slice); ArtifactKindIcon KIND_ICON += video.
  - `pnpm install --ignore-scripts` ✅; `pnpm typecheck` (tsc --noEmit) ✅ exit 0; vitest registry+api-store+craft suites 57/57 ✅; `check-sw-cache-bump.mjs` → "No Fabric Session asset changes." ✅.
  - Impact query recorded (Craft editors/Artifacts/Motion/Dashboards/Office [building]; surfaces ai.allternit.com, Desktop, m.allternit.com PWA; 280 components).
  - **No public/craft mirror**: image/pdf bundles are not mirrored in allternit-ai (bridge loads cross-origin from office.allternit.com via CRAFT_BASE); mirroring video alone would diverge — reported in PR body instead.

## Next
1. Platform commits (5 logical) → push → PR with evidence. DO NOT MERGE.
2. ai commits → push → PR with evidence. DO NOT MERGE.
3. Final report: PRs, SHAs, evidence, deferrals.

## Open questions
- Whether Eoj wants craft bundles mirrored into allternit-ai public/ at all (image/pdf never were) — flagged in PR bodies.
- wasm-opt needed no patch; the handoff's feared flag error did not occur with v133 on this module.
