# Steering checkpoint

**Session:** `session/craft-editors-20261009` · worktree `allternit-session-craft-1009` · plan dag_387644
**Date:** 2026-10-09

## Goal
Adopt the ArtCraft craft suite (PhotoCraft/PdfCraft/FilmCraft — pure-Rust, MIT/Apache-2.0) as the PDF, Image, and Video artifact editors. Approved plan (Eoj, 2026-10-09): local vendoring (NO GitHub forks), existing names/surfaces (Allternit Office for PDF, Allternit Design for image/video), WASM bundles served from the existing office.allternit.com surface, sandboxed iframe embedding with a craft-host bridge, Tier-C audit before any product wiring. Plan file: session plans dir (`nightcrawler-drax-black-panther.md`).

## Just did
- Deep research on `github.com/storytold` (7 apps, licenses verified MIT/Apache; `artcraft` engine repos legally blocked — excluded) and on our artifact system (v2 kinds registry `src/lib/artifacts/kinds.ts` on origin/main; local checkout 17 behind — fast-forward before Phase 1).
- Session worktree + DAG plan (`dag_387644`) created.
- Mirror-cloned the three repos to `~/Desktop/allternit-workspace/craft-mirrors/` (machine-local, not committed). Pinned revs: photocraft `0c72d9542…` (2026-10-09), pdfcraft `68e91d481…`, filmcraft `7bd762126…`.
- Vendored trees into `vendor/craft/{image,pdf,video}/` (~34/29/34 MB) at those revs. Wrote `vendor/craft/VENDOR.md` (provenance, license/trademark handling, refresh ritual) + `refresh-from-upstream.sh` (dry-run/apply).
- cargo-audit installing in background (not previously installed).
- All three rebrand+audit agents DONE: rebrands complete (docs/brand removed, community UI stripped, i18n verified by script — 1,285 literals × 16 catalogs for image; deep PDF-embedded strings and export-format provenance strings for pdf/video), per-app audit inventories written.
- AUDIT.md verdict: PASS with documented conditions (rsa/rustls-webpki not reachable in embed posture; zero analytics in all three trees).
- Committed + pushed: 5edf5f6afb on session/craft-editors-20261009 (vendored trees, VENDOR.md, refresh script, AUDIT.md, audit reports, craft-host/PROTOCOL.md). Upstream .github CI stripped (would have executed in our repo).

## Next
1. Implement the craft-host bridge: PROTOCOL.md + per-app adapter mapping are done and
   committed (5edf5f6afb); write the crate + the three web adapters (video wraps the
   existing `window.filmcraft` behind the token/origin gate; pdf adds the save-bytes
   write-back; image feeds postMessage into its transport-agnostic `control::handle()`).
2. WASM build pipeline (GHA: wasm32 builds → bundles into
   `surfaces/office.allternit.com/public/craft/<app>/` + allternit-ai mirror), bare
   open-edit-save test page; Eoj checkpoint before product wiring.
3. Phase 1 (separate session, after ai checkout fast-forwards): `pdf` kind in cloud-api
   kinds.rs + KIND_REGISTRY; CraftEditor component; save→versions; docs.

## Open questions
- ~~COOP/COEP~~ RESOLVED: all three current web builds are single-threaded; COOP/COEP only needed for a future threaded build. No header changes required for v1.
- Editor canvas theming inside egui (accept themed params, not full design tokens) — Eoj judges at the Phase-0 checkpoint.
- Plans/entitlements gating for the new kinds (Eoj's call at review, Phase 4).
- `gizzi workspace node add` CLI shape (help is unclear; DAG plan + node n_8583 exist, node breakdown tracked here meanwhile).
