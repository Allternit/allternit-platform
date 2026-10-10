# Vendor audit — craft/image (vendored PhotoCraft)

Vendored copy of `github.com/storytold/photocraft` (pure-Rust Photoshop-class image editor, dual MIT/Apache-2.0),
embedded in the Allternit platform as the image artifact editor (WASM web build). Paths below are relative to
`vendor/craft/image/` unless noted. Audit date: 2026-10-09. No `cargo build/check/test` was run (machine load
capped; central build happens later) — edits were made string/asset-level and cross-checked by grep and by a
script that verifies every non-test `tl!()` literal and every `UI_COMMANDS` label against all 16 i18n catalogs
(0 missing).

## a. REBRAND SUMMARY

Naming rule applied: user-visible "PhotoCraft" → "Allternit Image Editor"; ArtCraft names/wordmarks/logos stripped
(their `LICENSE-brand.txt` requires forks to remove the marks and allows a plain-text "based on" note). Machine
names untouched: crate names (`photocraft-*`), binaries (`photocraft`, `photocraft-cli`, `photocraft-web`),
reverse-DNS ids (`ai.storyteller.photocraft`), the `.pcraft` format, `PhotocraftApp`/`PhotocraftMcp` type names,
command ids, env vars (`PHOTOCRAFT_*`), the `Photocraft-dev` fetch User-Agent, and on-disk names
(`PhotoCraftData`, `PhotoCraft.portable`, `%APPDATA%\Photocraft`, `Application Support/Photocraft`).

What changed, by class:

- **Deleted** `docs/brand/` entirely (ArtCraft logos/marks + `LICENSE-brand.txt`). No code referenced those paths
  (only README/NOTICE/ATTRIBUTION/docs/book prose, all updated). The only runtime logo was the app icon
  (`crates/ui-egui/src/brand.rs` includes `assets/app-icon/hicolor/128x128/apps/ai.storyteller.photocraft.png`
  for the title-bar mark, web build included). Neutralized by replacing that one PNG with a generated plain
  rounded blue tile (128×128, transparent corners, opaque center — exactly the properties the existing unit test
  asserts, verified by parsing the PNG back). Code path untouched; `assets/app-icon/README.md` documents the swap.
- **App name, UI**: `native_menu.rs` `APP_NAME` (drives macOS app menu About/Hide/Quit via `{app}` templates),
  `panels.rs` `window_title()` + in-bar title, `canvas.rs` start-screen splash (`tl!("Allternit Image Editor")`),
  `dialogs.rs` About blurb/title, `screen_picker.rs` window title, `tool_cursor.rs` debug name, `gpu_status.rs`
  (system-info + CPU-compositor notice), `prefs_ui.rs` (3 strings), `new_doc_ui.rs` preset "Default … Size",
  `rasterize_prompt.rs`/`file_ui.rs` dialog `__label`s, `kys_import.rs` message, `notices.rs` Wayland hints,
  `chrome_ui.rs`/`theme.rs`/`kys_import_tests.rs`/`credits_tests.rs`/`file_dialog/tests.rs` test literals,
  `apps/photocraft/src/main.rs` (window title ×2, log line), `crash_guard.rs`, `screen_color.rs` (macOS error),
  `services.rs` file-dialog filters, both Windows `build.rs` (ProductName/FileDescription; LegalCopyright kept as
  "the PhotoCraft authors"), `cms/src/builtin.rs` built-in ICC profile labels, engine user-facing strings
  (`print_cmds.rs` `%%Creator`, `file_cmds.rs` `.cube` header, `automate_cmds.rs` droplet script+error,
  `preset_store.rs`, `smart_cmds.rs`, `preset_import_cmds.rs`, `swatch_cmds.rs`, `layer_menu_cmds.rs` params doc,
  `codecs/src/codecs/heif.rs` NOT_IN_BUILD), `io/src/abr_map.rs` warning.
- **Upstream marketing links removed from the UI**: Help menu lost `help.discord`, `help.website`,
  `help.artcraftWebsite` (menus.rs); `links.rs` rewritten (kept only GitHub + Report-an-Issue → upstream repo;
  `discord_button()` deleted; About/start-screen/title-bar call sites removed; `panels.rs` Discord button +
  `DISCORD_ROOM`, `titlebar.rs` responsive test updated). `links.rs`/`menus.rs` tests updated to match.
- **i18n catalogs** (16 `*.tsv`): scripted, verified transformation — token-replaced `PhotoCraft`→new name in
  source keys and translations; removed dead keys ("Join the ArtCraft Discord…", "ArtCraft Website", "ArtCraft",
  "PhotoCraft Website", "PhotoCraft website", "Join the ArtCraft Discord", "Join us on Discord", "Discord");
  renamed the leftover key to the new menu label ("Project on GitHub"); fixed the header guidance lines.
- **Cargo.toml descriptions** (7 files: apps ×3, heif, plugins, automation, example plugin) use the new name;
  package `name` fields unchanged.
- **README.md**: top-of-file vendored note added ("Based on PhotoCraft by the ArtCraft team (MIT/Apache-2.0)",
  marks removed, machine names kept); ArtCraft logo header, Discord badges/community blocks and the Crafting-Apps
  marketing table replaced with a short "Upstream" attribution section; getartcraft/Discord links removed;
  trademark footer rewritten; copyright/Adobe disclaimer kept.
- **Docs/prose** (62 files): `docs/**`, `book/**` (+book.toml), `packaging/**`, SECURITY.md, scorecard TOMLs,
  `xtask/src/scorecard.rs`, crate READMEs token-replaced; `AGENTS.md` naming rule rewritten for the fork;
  `docs/contributing.md`, `book/src/project/licenses.md` updated to say the marks were removed.
- **Packaging metadata**: user-visible names token-replaced (WiX `Name`/`Title`/Description, plist
  CFBundleName/DisplayName/doc-type strings, `.desktop` `Name=`, metainfo name, DMG volume name, portable.txt).
  **WiX/macOS identifiers, ProgIds and registry keys were restored to the original `PhotoCraft*` machine ids**
  after the bulk pass (spaces are invalid in WiX ids); `[PhotoCraftVersion]` in the .wxl matches the restored
  Property id; `PhotoCraftData`/`%APPDATA%\Photocraft` in portable.txt restored to match `app_dirs.rs`.
  Upstream marketing URLs in packaging (nfpm homepage, WiX ARPURL*/ARPHELPLINK, metainfo homepage/contact)
  repointed to the upstream GitHub repo/issues.

Deliberately left (with reasons):

- **LICENSE-MIT, LICENSE-APACHE, NOTICE** — copyright/trademark legal text; untouched on purpose (attribution).
- **ATTRIBUTION.md** — asset attribution (factual authors/sources, incl. "drawn in ArtCraft" provenance); only the
  `docs/brand/` row was removed (dir deleted). i18n `LICENSE-translations.txt` likewise.
- **Code comments/docstrings** (`//!`/`///`/`//`) mentioning PhotoCraft across crates — developer docs, not
  user-visible; kept to keep the vendored diff reviewable.
- **`.github/workflows/*`** — upstream CI/release names; not our build path.
- **Desktop-only upstream icons** (`photocraft.icns`, `photocraft.ico`, 1024/256/… PNGs) — used only by the
  desktop app/packaging we don't ship; the one in-app/web copy (128px) was replaced. Replace the rest before any
  branded desktop build.
- **Machine/test data**: `app_dirs.rs` constants, `codecs/tests/orientation.rs` EXIF marker, `format/src/zip.rs`
  test bytes, `text` font-alias prefix `.PhotoCraft-face-`, `raw/testgen.rs`, type-tool test strings,
  `photocraft-cli` test font name, `crates/plugins/tests/fixtures/invert.wasm` (compiled example).
- **docs/images/*.jpg** — upstream screenshots still show the old name/logo (regenerating needs a build).

References I could not fully neutralize: none blocking; the caveats above (desktop icons, screenshots, DS_Store
below) are the residual branding surfaces.

Known broken-if-used leftover: `packaging/macos/dmg/dmg-layout.DS_Store` is a binary Finder file whose alias
encodes the old `PhotoCraft.app` volume layout — regenerate it (per `packaging/macos/dmg/README.md`) before
cutting a macOS DMG.

## b. TELEMETRY/NETWORK INVENTORY

**There is no telemetry, analytics, crash-reporting, or auto-update phone-home anywhere in the tree.**
`grep` for reqwest/ureq/hyper/isahc/surf/curl/sentry/posthog/segment/mixpanel/amplitude/analytics/telemetry/
tracking/minidump/crashpad finds no shipped code or dependency (`Cargo.lock` has no HTTP client crate). What
exists, by surface:

Runtime (shipped):
- `apps/photocraft-web/src/web.rs:200-218` — `fetch_bytes()`: browser `fetch` of `fonts/manifest.txt` and font
  files, **same-origin only, relative URLs**, optional Subresource Integrity check (`RequestInit.integrity`).
  Missing manifest is the normal case. This is the only network egress in the embedded web build.
- `apps/photocraft-web/src/web.rs:393-417` — save/export via Blob + object URL + `<a download>` (no network).
- `apps/photocraft-web/src/web.rs:354-372` — File Open via browser picker (rfd async), Save = suggested filename
  only. `apps/photocraft-web/src/indexed_presets.rs:99-100` — IndexedDB for brush presets (local).
  `web.rs:380-389` — preferences in `localStorage`. No OPFS.
- `crates/ui-egui/src/links.rs:9-13` — UI opens upstream GitHub + issues URLs in the **system browser**
  (`open_url` service / `ctx.open_url`); not an embedded client.
- `crates/ui-egui/src/credits.rs:326` — About-dialog contributor names hyperlink to `https://github.com/<login>`
  (system browser; upstream contributors, attribution).
- `apps/photocraft/src/control_server.rs:19-48` — desktop JSON control server bound to **127.0.0.1 only**,
  token-authenticated (opt-in via `--control` / `PHOTOCRAFT_CONTROL_PORT`; token via
  `--control-token-file`/`PHOTOCRAFT_CONTROL_TOKEN[_FILE]`, 256-bit, mode 0600 on Unix). Not compiled into wasm.
- `crates/automation/src/rpc.rs:276` — MCP server TCP bind (stdio default; `--serve 127.0.0.1:…`);
  `crates/automation/src/bridge.rs:34-54` — TcpStream connect with a **loopback-only host guard**
  (127.0.0.1/localhost/[::1]/::1, else error); `crates/automation/src/security.rs:228` stream hardening.
  `crates/automation/tests/mcp.rs` uses loopback only.
- Local process spawns (no network): `apps/photocraft/src/appearance.rs:47` (gsettings), `monitor_profile.rs:56,68`
  (osascript helper), `linux_libs.rs:248` (ldconfig-ish probe), `crates/ui-egui/src/gpu_canvas.rs:1080,1093`
  (sysctl RAM size).
- `crates/ui-egui/src/screen_picker.rs` — the macOS screen-color picker uses ScreenCaptureKit locally; the web
  build uses the browser's EyeDropper API (`web.rs:431-498`) — browser-mediated, no endpoint.

Build/CI-time only (not shipped in the wasm app):
- `Dockerfile:26-29` — curl downloads the pinned trunk binary from `github.com/trunk-rs/trunk/releases`
  (User-Agent `Photocraft-dev`); `:68` wget healthcheck against localhost.
- `xtask/src/pinned.rs:174` — corpus/test-asset downloads from `https://codeload.github.com/…` (dev xtask);
  `xtask/src/corpus_pins.rs` documents upstream corpus sources (psd-tools, ag-psd, PngSuite, heic-rs,
  pillow_heif, vector-art…) and `raw.pixls.us` (line 246).
- `.github/workflows/*` — fetch nfpm, actionlint, trunk, rustup, and the optional `storytold/craft-fonts` pin.
- `packaging/linux/package.sh:126-129` — AppImage embeds `gh-releases-zsync|<GITHUB_REPOSITORY>|latest|…`
  update metadata (AppImageUpdate delta updates; fork-aware via GITHUB_REPOSITORY; only in upstream-style
  release builds).
- Test URLs are all `https://example.org` placeholders.

## c. UNSAFE CODE INVENTORY

- Workspace root `Cargo.toml:58` — `[workspace.lints.rust] unsafe_code = "forbid"` (everything inherits it).
- **Only exception**: `crates/tablet/Cargo.toml:12` — `unsafe_code = "deny"` crate-wide, and the only unsafe
  blocks are in `crates/tablet/src/macos.rs:43,59,69` (3 blocks: NSEvent reference, AppKit
  `addLocalMonitorForEventsMatchingMask_handler`, `removeMonitor` — each with a `SAFETY:` comment per AGENTS.md).
  The X11 path and all mapping code in that crate are safe.
- No other `unsafe` blocks found in any crate/app (grep over `crates/`, `apps/`, `xtask/`, `examples/`).

## d. WEB BUILD + CONTROL SURFACE SUMMARY (for the host-page bridge)

Web build (`apps/photocraft-web`): `index.html` is the trunk entry (`<canvas id="photocraft_canvas">`, loading
screen; title now "Allternit Image Editor"). `src/main.rs` → `web::start()` (`src/web.rs`): boots the same
`PhotocraftApp` (ui-egui) via eframe's WebRunner (WebGPU, WebGL2 fallback, `?cpu`/`?webgl` query flags), wires
`Services`: `import`/`export` through `photocraft_io`, file picker → `Services.inbox` (async bytes), saves →
browser download (`download()`), drag-and-drop read async into the inbox, pen pressure via Pointer Events,
beforeunload guard, preferences in localStorage (`photocraft.preferences`), brush presets in IndexedDB
(`indexed_presets.rs`, hydrated before the session). Documents therefore load only from user picks/drops (or
host-seeded inbox bytes); saves leave the page as downloads — there is no OPFS/document filesystem on wasm.
Optional same-origin `fonts/manifest.txt` serves CJK fonts (`served_fonts.rs`, SRI-checked).

Control surface (docs/control-protocol.md; desktop transport `apps/photocraft/src/control_server.rs`, handlers
`crates/ui-egui/src/control.rs`, MCP bridge `crates/automation`): the app is **transport-agnostic by design** —
transports deliver `ControlRequest {method, params, reply}` over an mpsc channel and the UI thread answers
between frames (`control.rs:36-56,220`). Desktop uses token-auth'd loopback TCP (one JSON line per request/
reply: `{"id":1,"method":"engine.execute","params":{…}}` → `{"id":1,"ok":true,"result":{…}}`). Methods:
`engine.execute {command,params}` / `engine.commands` (the 500+ command registry is the whole surface);
`ui.inspect`, `ui.set`, `ui.menu.invoke/list`, `ui.dialog.open/set/confirm/cancel/apply`, `ui.window.open/close`,
`ui.pointer` (pen-synthetic strokes), `ui.click/move/key/type`, `ui.resize`, `ui.screenshot` (base64 PNG),
`app.open {path}`/`app.save {path?}` (scoped to granted automation read/write roots; `file.*` path commands are
refused), `app.quit`, `jobs.list/cancel`. File-shunning rules live in `crates/automation/src/workspace.rs`.

Host-page bridge implication: the wasm build has **no control transport today** (no TCP in browsers), but the
handler layer needs no redesign — a thin wasm-side transport (e.g. `postMessage` → construct `ControlRequest` →
`control::handle()` between frames → reply back) gives the host page the full command surface. For documents,
the host must additionally plumb bytes in (inbox/`Services.import`) and capture saves (wrap the `write` service,
which currently triggers a download) — see `web.rs:335-384`.

## e. FONT FILES

**Found 4 committed font files** (against upstream's own "never commit fonts" rule — an upstream exception, not
something introduced here): `assets/fonts/Inter-Regular.ttf`, `Inter-Medium.ttf`, `Inter-SemiBold.ttf`,
`JetBrainsMono-Regular.ttf`, each with its OFL license next to it (`OFL-Inter.txt`, `OFL-JetBrainsMono.txt`) and
rows in ATTRIBUTION.md. `crates/text/build.rs` deflates them into `OUT_DIR` (`fonts::BUNDLED`, the UI faces).
`crates/text/build.rs` also reads the **optional** `CRAFT_FONTS_DIR` env (external `storytold/craft-fonts`
checkout): fonts listed in its `fonts/manifest.txt` are embedded as `photocraft_text::CRAFT_FONTS`; unset/empty →
empty (everything still works); error → warning (or build error with `CRAFT_FONTS_REQUIRED=1`). On wasm32 nothing
is embedded regardless (24 MiB size gate); the web build fetches host-served fonts instead. No other font files
exist in the tree.
