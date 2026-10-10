# Vendored craft editors (PhotoCraft / PdfCraft / FilmCraft)

Vendored from `github.com/storytold` (the ArtCraft "Crafting Apps") into the Allternit
platform. Same pattern as `vendor/oss` in `allternit-ai` (GenOffice): these copies are
ours to skin, rebrand, and patch; upstream improvements are cherry-picked on a refresh
ritual. Upstream is **~9 days old and ~100% AI-written** — treat every refresh as
untrusted input until re-audited (see `AUDIT.md`, refreshed per bump).

| dir | upstream | pinned rev | date | vendored app |
|---|---|---|---|---|
| `image/` | `storytold/photocraft` | `0c72d95425dece90ef9a1cceb49e3315c96e22d5` | 2026-10-09 | image artifact editor (Allternit Design surface) |
| `pdf/` | `storytold/pdfcraft` | `68e91d4815ed43212895589411aa3d21b2a84bc3` | 2026-10-09 | PDF artifact editor (Allternit Office surface) |
| `video/` | `storytold/filmcraft` | `7bd76212629a067a9d1c5426d751de9faf4664b7` | 2026-10-09 | video artifact editor (Allternit Design surface) |

## License

All three are dual **MIT OR Apache-2.0** (LICENSE-MIT + LICENSE-APACHE kept verbatim in
each tree, plus NOTICE/ATTRIBUTION). The trademark clause requires forks/modified
versions to strip ArtCraft names, wordmarks, and logos — done in this vendored copy:

- User-facing product names in UI strings, window titles, About, docs prose renamed to
  descriptive labels ("Allternit Image Editor" / "Allternit PDF Editor" /
  "Allternit Video Editor"). No new brand names — the artifact kinds are Image/PDF/Video.
- `docs/brand/` removed (ArtCraft logos are **not** open source — upstream
  `docs/brand/LICENSE-brand.txt`).
- Crate names, binary names, and reverse-DNS machine ids (`ai.storyteller.*`) are
  machine names, not marks, and are left unchanged on purpose.

**Never touch:** `storytold/artcraft`, `artcraft-services`, `artcraftx` — custom
fair-source license forbidding use in competing products. Not vendored, never will be.

## Rebrand rules (keep the trees mergeable-ish with upstream)

1. Only user-facing strings, docs prose, and brand assets change. Never rename crates,
   binaries, ids, or file formats (`.pcraft`, `.fcproj`, document type strings).
2. Keep upstream attribution in README/docs ("Based on PhotoCraft by the ArtCraft team,
   MIT/Apache-2.0").
3. `.github/` (upstream CI) is stripped from the vendored trees — merged workflows would
   execute in this repo. CI for the craft trees is rebuilt in `.github/workflows/` at the
   repo root when the WASM build pipeline lands.
3. Each refresh re-applies the rebrand delta — keep rebrand edits small and localized;
   record them in `REBRANDED.md` per tree.

## Refresh ritual (run per quarter or when upstream lands something we want)

```sh
# mirror clones live outside the repo (machine-local):
#   ~/Desktop/allternit-workspace/craft-mirrors/{photocraft,pdfcraft,filmcraft}.git
./refresh-from-upstream.sh            # shows what changed; add --apply to copy
# then: re-apply rebrand delta, re-run the audit steps in AUDIT.md, bump the table above.
```

## Embedding

The WASM frontends (`apps/*-web`) are built in CI and published as static bundles to
`surfaces/office.allternit.com/public/craft/<app>/` (served at
`office.allternit.com/craft/<app>/`) and mirrored into `allternit-ai/public/craft/`.
The app embeds them sandboxed cross-origin. The host-page bridge lives in
`craft-host/` (shared crate).

## Building the bundles (manual until CI lands)

Requirements: `wasm-bindgen-cli` at the pinned `wasm-bindgen` version (all three trees
currently pin 0.2.129 — `cargo install wasm-bindgen-cli --version 0.2.129 --locked`),
`trunk` (`cargo install trunk --locked`), and `CARGO_TARGET_DIR` pointed at the
workspace shared target.

**The bundles MUST be built with the `embed` feature** — a stock bundle does not speak
the craft:1 protocol and the host handshake will time out.

The video xtask reads the feature from the **`CRAFT_FEATURES`** env var
(`vendor/craft/video/xtask/src/main.rs` forwards it to cargo `--features`), so the
FilmCraft build is `CRAFT_FEATURES=embed cargo xtask web`, never a bare `cargo xtask web`.
Future CI for the craft trees (rebuilt under `.github/workflows/` once the WASM build
pipeline lands; builds are manual until then) must set `CRAFT_FEATURES=embed` on the
video job the same way the image/pdf trunk jobs pass `--features embed`, and fail the
job when the bundle does not speak craft:1 (the `strings … | grep craft:1` check below).

```sh
# NB: this shell exports NO_COLOR=1, which trunk's CLI rejects — override it:
export NO_COLOR=true
export CARGO_TARGET_DIR="$HOME/Desktop/allternit-workspace/.shared-target"
(cd vendor/craft/image/apps/photocraft-web && trunk build --release --features embed)  # → vendor/craft/image/dist/web/
(cd vendor/craft/pdf/apps/pdfcraft-web    && trunk build --release --features embed)  # → vendor/craft/pdf/dist/web/
(cd vendor/craft/video && cargo xtask web)  # CRAFT_FEATURES=embed → target/web/dist
cp -R vendor/craft/image/dist/web/. surfaces/office.allternit.com/public/craft/image/
cp -R vendor/craft/pdf/dist/web/.    surfaces/office.allternit.com/public/craft/pdf/
cp -R vendor/craft/video/target/web/dist/. surfaces/office.allternit.com/public/craft/video/
```

Constraints: every file < 25 MiB (Pages per-file limit); video's xtask runs wasm-opt.
Verify a bundle speaks craft:1 with `strings <name>_bg.wasm | grep craft:1`.

