# Craft editors Phase 2 — image kind (PhotoCraft over craft:1)

Session: `session/craft-image-kind-20261010` · Kimi Code · 2026-10-10 ~05:00
Merged: platform **#1477** (`70ee41ad`, merge commit) · allternit-ai **#497** (`6eb3c3a5`, merge commit)
Continues: `2026-10-10-0341-10650ac0-kimi-code-craft-editors-phase0-1.md` (HANDOFF-craft-editors-2026-10-10 item 1)

## What was done

Image artifacts open in the embedded PhotoCraft WASM editor (office.allternit.com/craft/image/, bundle already live), layered edits survive sessions, and every save flattens a new artifact version.

**App (allternit-ai #497)**
- `src/components/craft/image-io.ts` (new): open-source resolution — stored `.pcraft` → stored `.psd` → flat body URL / data-URI; save classification — the user's own `.pcraft`/`.psd` saves vs host-requested `app.save {path}` flatten exports, told apart by the adapter's save `meta.path`; PSD export waiters for Download PSD.
- `src/components/craft/CraftEditor.tsx`: generalized open/save behind kind callbacks (PdfCraftEditor unchanged); `ImageCraftEditor` — user source saves → artifact runtime storage (`craft/source.pcraft`, `craft/source.psd`, personal scope); requested flatten PNG → `uploadAttachmentToCloud` (R2) → appended `text/uri-list` version body (the URL every viewer renders); PSD exports stored for Download PSD.
- `bridge.ts`: `craft:save-request` passes the app's `meta` through (PhotoCraft puts the save path there).
- `src/lib/artifacts/runtime-storage.ts` (new): client for `/api/v2/artifact-runtime/:id/storage` (base64 values; server caps 5 MB/value, 20 MB/artifact).
- `store.ts`: `declareStorage()` — the editor PATCHes `capabilities.storage: true` onto the record before its first source save (idempotent).
- `kinds.ts`: image `Editor = ImageCraftEditor`; new `psd` export (Download PSD); `ArtifactWindow` exportFormat `.psd` case.
- `export.ts`: Download PSD — stored source, else fresh export from the open editor over the bridge; `safeFileName` moved to `filename.ts` so the export module does not statically import the lazy editor chunk.
- `ArtifactsView`: Upload image (R2 URL body ≥1 MB, data-URI under) creates editable image artifacts with `storage: true`.

**Server (allternit-platform #1477)**
- `artifacts/runtime.rs`: the artifact-runtime storage gate admitted only page/card; it now admits `image` (and `video`, Phase 3 forward-looking) for **storage only** — never page AI or viewer connectors. New `Denied::NotStorageKind` (409 `not_a_storage_kind`) for kinds that neither run apps nor craft-edit.
- `routes/artifact_runtime.rs`: deny() maps the new variant.
- Docs: `artifact-modes.mdx` PhotoCraft paragraph; `artifact-runtime.mdx` storage-route note; dependency-map Phase-2 decision.

## How the save flow works

1. User saves in PhotoCraft → the platform write service routes the save to the bridge as `craft:save-request` (extension = format).
2. `.pcraft`/`.psd` (user's own) → base64 into artifact runtime storage; ack; then the host requests `app.save {path: "<slug>.png"}` and `{path: "<slug>.psd"}`.
3. Those exports come back as save-requests carrying `meta.path` → PNG uploads to R2 and appends the version (the flat image everywhere); PSD stores to `craft/source.psd`.
4. Re-open prefers the stored `.pcraft`, then `.psd`, then the flat URL — so layers survive.

## Verification

- `pnpm typecheck` clean; `pnpm build` green.
- vitest: craft + artifacts suites green — image-io 12, bridge 10 (new: save-meta passthrough), store declareStorage (new), registry 23.
- `cargo test -p allternit-cloud-api --lib artifacts` 77/77 (new: craft kinds store sources but never run apps; NotStorageKind coverage).
- Docs: `check_links.py` 0 problems; dependency-map `--validate` clean.

## Incidents / honest deferrals

- **Deploys NOT done**: ai.allternit.com's GitHub Actions deploy workflow is billing-blocked (last run 2026-09-28, 4–5 s failures), so merging #497 does not deploy. ai.allternit.com needs a manual wrangler deploy, and m.allternit.com PWA needs its separate manual deploy + SW cache bump — both await Eoj's go-ahead per the deploy runbook. Until the platform gate deploys, editor source saves 409 `not_a_storage_kind` and the editor surface shows the failure; the app keeps the document dirty (retry once deployed) — no data-loss path.
- **Storage caps**: a layered source over 5 MB (server value cap) fails that save with a visible error; the 20 MB/artifact quota is shared with page runtime storage. Acceptable for v1; large-document strategy (R2 for sources too) is a Phase-4 candidate.
- The shared checkout (`allternit/`) could not be fast-forwarded (other sessions' dirty files overlap the pull); the ledger commit lands via a detached worktree instead. `git-discipline-check.sh` will not PASS on the shared checkout until those sessions finish — expected.
