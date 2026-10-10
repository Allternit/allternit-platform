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
- Three parallel agents rebranding the trees (strip ArtCraft marks → "Allternit Image/PDF/Video Editor" descriptive labels; docs/brand removed) + writing per-app audit inventories to `vendor/craft/audit/{image,pdf,video}.md`.

## Next
1. Collect rebrand/audit agent reports → consolidate `vendor/craft/AUDIT.md` (cargo audit results + telemetry inventory + unsafe inventory; product wiring waits on this gate).
2. Design + implement `vendor/craft/craft-host` bridge crate (postMessage protocol: init/open/save/command; FilmCraft's `window.filmcraft` is the reference shape).
3. WASM build pipeline (GHA: wasm32 builds → bundles into `surfaces/office.allternit.com/public/craft/<app>/` + allternit-ai mirror), bare open-edit-save test page; Eoj checkpoint before product wiring.
4. Phase 1 (separate session, after ai checkout fast-forwards): `pdf` kind in cloud-api kinds.rs + KIND_REGISTRY; CraftEditor component; save→versions; docs.

## Open questions
- COOP/COEP header support on the office.allternit.com Pages surface (needed for wasm threads; single-threaded fallback exists) — verify in Phase 0 step 5.
- Editor canvas theming inside egui (accept themed params, not full design tokens) — Eoj judges at the Phase-0 checkpoint.
- Plans/entitlements gating for the new kinds (Eoj's call at review, Phase 4).
- `gizzi workspace node add` CLI shape (help is unclear; DAG plan + node n_8583 exist, node breakdown tracked here meanwhile).
