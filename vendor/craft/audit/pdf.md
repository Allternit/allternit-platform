# Vendored pdfcraft — rebrand & security audit

Vendored tree: `vendor/craft/pdf/` (github.com/storytold/pdfcraft, pinned rev, dual MIT/Apache-2.0).
Audit date: 2026-10-09. The WASM web build is embedded in the Allternit platform as the PDF
artifact editor (Allternit Office surface). Per the license trademark clause, ArtCraft marks were
stripped; user-facing names are now "Allternit PDF Editor".

---

## A. Rebrand summary

### Changed

**Brand assets — removed**
- `docs/brand/` deleted entirely (8 ArtCraft logo/mark files + `LICENSE-brand.txt`).
- Their 8 entries removed from `ATTRIBUTION.toml` and `ATTRIBUTION.md` (kept in sync by hand, since
  `cargo xtask assets --write` could not be run); the screenshot-licence text in
  `xtask/src/screenshots.rs` no longer claims screenshots show the mark.
- Code references neutralized:
  - `crates/ui-egui/src/widgets.rs` — `artcraft_logo()` and `artcraft_mark()` deleted (they
    `include_bytes!`'d the SVGs); `community_links()` simplified to render every link uniformly.
  - `crates/ui-egui/src/home.rs` — the "Join the ArtCraft community" card removed from the home screen.
  - `crates/ui-egui/src/dialogs.rs` — About dialog: mark, "Part of" row and wordmark removed; the app
    name label now reads "Allternit PDF Editor".
  - `crates/ui-egui/src/chrome.rs` — the Discord button in the tab-strip removed.
  - `crates/engine/src/links.rs` — the link table collapsed to a single upstream-source link
    (`GITHUB = https://github.com/storytold/pdfcraft`, label "Source code on GitHub");
    `DISCORD`/`WEBSITE`/`APP_PAGE`/`APP` constants removed.
  - `crates/engine/src/commands.rs` — `help.discord`, `help.app_page`, `help.website` commands
    removed; `help.github` reworded; `help.about` → "About Allternit PDF Editor".
  - `crates/automation/src/mcp.rs` — MCP `serverInfo.title` → "Allternit PDF Editor",
    `websiteUrl` → the GitHub link; `INSTRUCTIONS` and the `tool_search`/`tool_call` titles and
    descriptions rebranded.
  - `apps/pdfcraft-cli/src/main.rs` — `pdfcraft-cli --version` no longer prints Discord/Web lines.
  - Packaging URLs that pointed at getartcraft/discord now point at the GitHub repo/issues
    (`packaging/windows/pdfcraft.wxs` ARP properties, `packaging/linux/…metainfo.xml.in`,
    `packaging/linux/nfpm.yaml`).
  - `AGENTS.md` §1.2 rewritten to state the brand was removed in this fork; `NOTICE` brand
    paragraph replaced with a "vendored, rebranded copy" statement (copyright line kept).

**User-visible names — "PdfCraft"/"PDFCraft" → "Allternit PDF Editor"** (case-sensitive sweep over
first-party code, docs, packaging):
- Window titles (`apps/pdfcraft/src/main.rs` `run_native`/`with_title`, `crates/ui-egui/src/lib.rs:1913`).
- Home screen, update dialog, recovery dialog, OCR/signing/XFA/form-notice strings, blocked-link
  notices (`crates/ui-egui/src/{home,updates,dialogs,ocr_ui,sign_ui,js_ui,canvas,lib}.rs`).
- All 14 i18n catalogs (`crates/ui-egui/src/i18n/*.tsv`): changed-string keys renamed in every
  locale, name mentions inside translations replaced (product name stays Latin per locale
  conventions), ArtCraft/community rows deleted, new "Source code on GitHub" row added in all 14.
- Windows VERSIONINFO (`apps/pdfcraft/build.rs`, `apps/pdfcraft-cli/build.rs` + test
  `tests/version_resource.rs`) — ProductName/FileDescription rebranded; LegalCopyright keeps
  "the PdfCraft contributors".
- Web build: `apps/pdfcraft-web/index.html` `<title>`.
- PDF-embedded output: Producer (`crates/create/src/lib.rs:53`), signature app `/Name`
  (`crates/sign/src/pdf.rs:1364,1527`), signature-check messages, ICC profile desc
  (`crates/preflight/src/icc.rs:51`), print job title (`crates/print/src/spool.rs:38`), default
  comment author (`crates/automation/src/comments.rs:15`), OCR error/install-dir strings
  (`crates/ocr/src/lib.rs`).
- Update-check User-Agent: `PdfCraft/<ver>` → `Allternit-PDF-Editor/<ver>` (`apps/pdfcraft/src/updates.rs:20`).
- xtask demo/showcase content (title "Allternit PDF Editor Showcase", authors, keywords,
  `AllternitPdfEditorShowcase` XMP marker, `assets/demo/showcase.html/.csv`), xtask report strings,
  `assets/app-icon/README.md`/`LICENSE.txt` prose.
- Data-dir names that carry the wordmark: settings dir `eframe::storage_dir("Allternit PDF Editor")`
  (`apps/pdfcraft/src/main.rs:53`), Windows legacy-migration target, recovery dirs
  (`crates/ui-egui/src/recovery.rs:53-55`), portable data dir `PdfCraftData` → `AllternitPdfData`
  (`crates/ui-egui/src/portable.rs:16`), `packaging/windows/portable.txt`.
- Cargo.toml `description` fields (10 crates/apps; package names untouched), docs prose
  (`docs/*.md`, `ROADMAP.md`, `CLAUDE.md`, `parity/acrobat-features.toml`, `ATTRIBUTION.*` prose),
  README (see below).
- Tests asserting any of the above were updated in lockstep (window-title, home, updates, signing,
  editing Producer, fonts glyph, links, ui tab-strip layout tests that anchored on the removed
  Discord button — now anchored on the Keyboard-shortcuts button).

**README** — top note added: vendored, rebranded copy, **"based on PdfCraft by the ArtCraft team
(MIT/Apache-2.0)"**; ArtCraft logo header, Discord badges, getartcraft link rows, the community
note, the "Made by the ArtCraft team" footer and the Crafting-Apps promo table replaced with a
neutral upstream-pointer section; license section's brand paragraph rewritten; footer attribution
keeps the required wording.

### Deliberately left unchanged (machine names / non-user-visible)
- Crate names (`pdfcraft-*`), binary names (`pdfcraft`, `pdfcraft-cli`), reverse-DNS ids
  (`ai.storyteller.pdfcraft`), `pdfcraft://` MCP resource URIs, MCP `serverInfo.name: "pdfcraft"`.
- `PdfCraftApp` struct/type name and other identifier names; code comments (only stale brand-asset
  comments were updated).
- Persistence keys `"pdfcraft"`/`"printcraft"` (eframe storage), env vars `PDFCRAFT_*`, XDG data
  dir `pdfcraft/`.
- File-format fingerprint markers: `%PdfCraft` content-stream wrap markers (`crates/edit/src/lib.rs:299-300`),
  `/PCAdded`, `/PCFillSign`, dict keys `PdfCraftFuzz`/`PdfCraftTest`, `PdfCraft.portable` portable
  marker (still recognized for upstream portable copies).
- Upstream release-package layout names: `PdfCraft.app` bundle + executable, DMG volume "PdfCraft",
  `PdfCraft.AppDir`, Windows registry ProgID/verb keys (`PdfCraft.Document`, `PdfCraft.CreatePdf`,
  `Software\PdfCraft\…`), MSI install dir — the OCR model-probe layouts and committed
  `dmg-layout.DS_Store` depend on these names, and this fork ships none of those installers.
- Factual provenance: `LICENSE-MIT`/`NOTICE` copyright, ATTRIBUTION author fields ("drawn by the
  project owner in ArtCraft"), app-icon origin notes.
- `vendor/` third-party code (patch comments mention PdfCraft); `.github/workflows` CI display names
  (not product surfaces).

### Could not safely neutralize / residuals
- `docs/images/*.png` README screenshots still show the old UI (community card, "PdfCraft" strings,
  and in home-screenshot shots the ArtCraft mark). They are contributor-original MIT assets, but
  regenerating them needs `cargo xtask screenshots` (a full build + Chrome). **Recommend
  regenerating or replacing before any public release of docs from this fork.**
- `crates/ui-egui/src/portable.rs:14` keeps recognizing the `PdfCraft.portable` marker filename
  (intentional, invisible).
- The update check and Help ▸ About still point at the upstream GitHub repo/releases (intentional
  attribution; there is no Allternit update feed wired in).

### Verification done (no cargo, per instructions)
- Grep sweeps for `PdfCraft|PDFCraft|ArtCraft|artcraft|getartcraft|discord.gg` across the tree;
  every remaining hit classified (list above) — no user-visible occurrence remains outside
  `vendor/`, workflows, and deliberate exceptions.
- i18n catalog gates checked statically: all changed `tl!("…")` literals (11) exist in the 9
  full-catalog languages (ja, it, pt-br, zh-hans, fr, de, ru, bg, hu); the two changed command
  labels exist in all 13 command-catalog languages; the OCR error key exists in the 11 locales
  required by `missing_ocr_models_messages_are_translated`; Czech has no source==translation rows.
- No dangling code references (`artcraft_logo/mark`, removed `help.*` commands, removed
  `links::{APP,DISCORD,WEBSITE,APP_PAGE}`) — the one miss (`pdfcraft-cli --version`) was found and
  fixed.
- **Not verified by building** (machine load cap): `cargo check`/tests were not run. Risk areas for
  the central build: the hand-kept ATTRIBUTION.toml/md sync, TSV edits (tabs/escapes), and the
  removed UI elements' test updates.

---

## B. Telemetry / network inventory (security input)

**No analytics, telemetry, crash-reporting, sentry/posthog/segment/matomo anywhere** (exhaustive
grep; only false-positive "segments" in DER/geometry code). The README badge "no account, no
telemetry" matches the code. Every actual network touchpoint in first-party code:

| Endpoint / use | file:line | What it does |
|---|---|---|
| `https://api.github.com/repos/storytold/pdfcraft/releases/latest` | `apps/pdfcraft/src/updates.rs:7` | Desktop "Help ▸ Check for updates" only, user-initiated, no check at startup. ureq 3.4 (only HTTP dep in the workspace, `apps/pdfcraft/Cargo.toml:14`) with rustls + OS native roots; custom UA `Allternit-PDF-Editor/<ver>`. Response parsed defensively; the release URL is validated to stay under `github.com/storytold/pdfcraft/releases*` (updates.rs:56-58, tested). Never downloads/installs — opens the release page in the browser. |
| `https://github.com/storytold/pdfcraft/releases` | `crates/ui-egui/src/updates.rs:15` | Constant `RELEASES_PAGE`; opened in the system browser when no update source is wired (web build, tests). |
| `webbrowser` crate (OS browser hand-off) | `crates/ui-egui/src/lib.rs` (`open_url`), document links via `request_document_url` | Opens user-confirmed http/https/mailto only; `crates/engine/src/links.rs:39-129` (`document_url`) refuses every other scheme (`file:`, `javascript:`, `data:`, `smb:`, app handlers), 2048-char cap, control/bidi-char refusal, WHATWG-URL re-validation, and blocks `mailto` parameters that make email clients attach/insert local files. Host display goes through punycode/mixed-script detection (`display_host`, links.rs:265-338). |
| `window.fetch(url)` for `?file=<url>` | `apps/pdfcraft-web/src/main.rs:40-52, 71-81` | **Web build only**: fetches a PDF from the URL in the page's `?file=` query param (browser fetch → CORS applies), pushes bytes into `startup_inbox`. Errors surface in-app and in the console. |
| Browser downloads (Blob + anchor click) | `crates/ui-egui/src/editing.rs:659-675`; callers `crates/ui-egui/src/files.rs:637-638, 701-702, 748-751` | **Web build only**: every save/export leaves the app as a browser download (`application/pdf` Blob, object URL, `<a download>`). No upload path. |
| eframe WebStorage (localStorage) | `crates/ui-egui/src/lib.rs:1768-1769` | UI preferences persisted under key `"pdfcraft"` (localStorage on web; RON file on desktop). |
| `https://ocrs-models.s3-accelerate.amazonaws.com/{text-detection,text-recognition}.rten` | `ATTRIBUTION.toml:3100,3113`; fetched by `xtask/src/assets.rs:319-330` (`curl -sSfL`, sha256-verified) | **Build-time, developer machines only**: `cargo xtask models` fetches OCR models; not part of `cargo build`. |
| `https://raw.githubusercontent.com/google/fonts/<pinned commit>/…` (18 font URLs) | `ATTRIBUTION.toml:2377-2624`; `xtask/src/assets.rs:306` | **Build-time only**: `cargo xtask demo-pdf` fetches demo fonts, sha256-verified; fails if the built PDF embeds any font not allowlisted. |
| `git clone https://github.com/mozilla/pdf.js.git` | `xtask/src/corpus.rs:146` | **Dev tool only**: fetches the pdf.js test corpus (pinned commit + sha256 in corpus metadata). |
| JS engine network denial | `crates/js/src/tests.rs:531` | Acrobat-JS `Get()`/submitForm are not executed against the network — `submitForm` is explicitly refused in the UI ("doesn't send form data", `crates/ui-egui/src/js_ui.rs:47-50`, `canvas.rs:2742-2745`). |

Notes: `vendor/` crates (winit, egui-wgpu, hayro, lopdf) contain no telemetry either; winit/hayro
reference http only for license comments/CRL parsing in tests (`crates/sign/src/x509.rs:524` parses
a CRL distribution-point URI from a fixture — no fetch). The MCP server and automation crate open
**no socket** (stdio only). No WebSocket/WebRTC/UDP anywhere. There is **no OPFS/IndexedDB** use —
web documents live in memory, enter via `?file=` fetch or drag/drop/file-picker, and leave via
download.

---

## C. Unsafe code inventory

- Workspace-wide ban: `[workspace.lints.rust] unsafe_code = "forbid"` (`Cargo.toml:72`); crates add
  `#![forbid(unsafe_code)]` (e.g. `crates/filters/src/lib.rs:13`). AGENTS.md §4 requires it.
- **First-party unsafe blocks/functions/impls: zero** (regex-verified `unsafe\s*\{`, `unsafe fn`,
  `unsafe impl` over `crates/`, `apps/`, `xtask/`).
- All first-party "unsafe" hits are prose: comments documenting *why* a safe wrapper crate is used
  (`apps/pdfcraft/src/apple_events.rs:7` — `fmv-macos-events` wraps Objective-C event handling;
  `apps/pdfcraft-cli/src/main.rs:819` — `rustix::process::geteuid`; `apps/pdfcraft/src/main.rs:331`
  — glow renderer info), or unrelated words (`xtask/src/demo_pdf/mod.rs:97` "percent-encoding
  unsafe bytes").
- `vendor/` (third-party, patched): 70 files contain unsafe — normal for winit/egui-wgpu/hayro
  platform bindings. Vendored copies are pinned in `Cargo.lock` with patches guarded by tests
  (`apps/pdfcraft/src/main.rs:798-908` asserts the egui-wgpu/winit patches are present).

---

## D. MCP confinement

Code: `crates/automation/` (server `src/mcp.rs`, tool table `src/tools.rs`, session wrapper
`src/lib.rs`), entry `pdfcraft-cli mcp` (`apps/pdfcraft-cli/src/main.rs:706-717`).

- **Transport / auth model**: newline-delimited JSON-RPC 2.0 over **stdin/stdout only** — no port,
  no socket, no token, no other auth; the process is started by the user/agent and exits when stdin
  closes (`mcp.rs:86-98`). It cannot be enabled programmatically by the app; builds can remove it
  entirely (`cargo build -p pdfcraft-cli --no-default-features`, feature `mcp`).
- **`--root` confinement** (`crates/automation/src/lib.rs:104-110, 1486-1568`):
  - `Automation::with_root` canonicalizes the root dir at construction (must exist and be a dir).
  - Every user-supplied path goes through `resolve(path, for_write)`: lexical join into the root
    (`..` can't escape — `lexical()` strips them, `None` → refused), Windows share/device-namespace
    check (`foreign_share` refuses another UNC share than the root's), then **canonicalization of
    the nearest existing ancestor** — a symlink or junction pointing outside the root is refused
    uniformly ("outside the allowed directory"), including broken links (which would leak existence).
    For writes to new files, the nearest existing ancestor must be inside the root.
  - Uniform error message for everything outside (no oracle leaking what exists beyond the root),
    and paths below the root report the OS error.
  - `--out` PNG/CSV outputs from tools are confined the same way (`lib.rs:125-131`).
  - Saves are atomic: staging files via `pdfcraft_platform::staging` (same-filesystem temp + rename).
- **Tool list**: ~126 tools (`t(...)` registrations in `src/tools.rs`) — doc_open/doc_save/doc_close,
  page_render, text_extract/text_find, page_* (rotate/delete/move/insert/crop/number), bookmarks,
  comments, forms, protect, redact, measure, print, links, doc_combine/doc_split, edit_undo/redo,
  command_list/command_run/command_batch, doc_inspect, render_preview, … Each has a JSON Schema;
  failures return `isError` text an agent can act on. `tools/call` runs behind a `catch_unwind`
  guard (`mcp.rs:119, 327-329`, engine `guard`) so a panic in a tool becomes an error, not a crash.
- **`--compact`**: `tools/list` returns the 14 `COMPACT_CORE_TOOLS` (`mcp.rs:41-57`: doc_open,
  doc_info, doc_save, doc_close, page_render, text_extract, text_find, doc_combine, doc_split,
  edit_undo, command_list, command_run, command_batch, doc_inspect, render_preview) plus two meta
  tools, `tool_search` and `tool_call`, which search/run **every** other tool by name (`mcp.rs:202-239`).
- **Resources (read-only)**: `pdfcraft://doc/{doc}/info|text|page/{n}/text|page/{n}/image(?dpi=1-600)`
  and `pdfcraft://document`, `pdfcraft://commands` (`mcp.rs:349-469`) — thin wrappers over the same
  tools; `serverInfo.title` is now "Allternit PDF Editor", `name` stays `pdfcraft`.
- No authentication exists by design: the security boundary is (a) the user launching the process,
  (b) stdio-only transport, (c) optional `--root`, (d) per-tool input validation + unknown-key
  rejection (`check_keys`, `mcp.rs:261-267`).

## D-bis. UI control channel (desktop only, relevant as the JSON-command precedent)

`crates/ui-egui/src/control.rs` (opt-in via `pdfcraft --control <file>`; loopback TCP
`127.0.0.1:<random>` + per-launch random token, written 0o600/owner-only to `<file>`
(`apps/pdfcraft/src/main.rs:344-405`); refuses symlinked/foreign-owned control files). Protocol:
newline JSON-RPC; first request must be `auth {token}`. Methods (`control.rs:14-32`): `ui.state`,
`ui.inspect`, `ui.click`, `ui.move`, `ui.drag`, `ui.type`, `ui.key`, `ui.command`/`ui.commands`,
`ui.set` (view options + preferences), `ui.open {path}`, `ui.screenshot`. Drives the real UI via an
egui plugin that keeps the AccessKit tree and injects input events (`control.rs:106-134`). **Not
compiled for wasm** (`serve` is `#[cfg(not(target_arch = "wasm32"))]`).

---

## E. Web build + control surface (feeds the host-page bridge design)

- **Entry**: `apps/pdfcraft-web/src/main.rs` (+ `apps/pdfcraft-web/index.html`, trunk-built;
  canvas `#pdfcraft`, `<title>Allternit PDF Editor</title>`). It boots `PdfCraftApp::new()` from
  `crates/ui-egui` via `eframe::WebRunner` (Glow/WebGL renderer). All real functionality lives in
  the `pdfcraft-ui-egui` library crate — **the host page can embed `PdfCraftApp` in-process** and
  drive it directly.
- **Loading documents under wasm**:
  1. `?file=<url>` query param → `window.fetch` (CORS-bound) → `startup_inbox`
     (`main.rs:38-52`; consumed at `crates/ui-egui/src/lib.rs:1831`).
  2. Drag-drop and the file picker go through `egui::DroppedFileHandle`/`rfd` wasm reads into
     in-memory bytes (`crates/ui-egui/src/files.rs:176-205, 255-275`). **No OPFS/IndexedDB** —
     documents never persist to disk in the browser.
  3. Programmatic: `PdfCraftApp::open_bytes(name, path, bytes)` (`lib.rs:838`) is public — the
     natural host-bridge entry; the failed-fetch queue is `failed_inbox` (`lib.rs:426, 1839`).
- **Saving**: `file.save`/`save_as` on wasm produce a browser **download** (Blob +
  `<a download>`, `editing.rs:659-675`, dispatched from `files.rs:637-638`). There is no write-back
  API — a host bridge that autosaves artifacts will need to add one (e.g. expose the save bytes via
  a callback instead of `download()`).
- **Preferences** persist via eframe WebStorage (`localStorage`, key `"pdfcraft"`, `lib.rs:1768`).
- **Command surface available to a host**: the full engine is reachable in-process —
  - `PdfCraftApp::execute(command_id)` / `run_command` — the same ~400-entry command registry the
    menus use (`crates/engine/src/commands.rs`), incl. `file.*`, `edit.*`, `page.*`, `help.*`;
  - `app.session` (`pdfcraft_engine::Session`): `apply(doc, Edit)` / `undo` / `save` — the same API
    the MCP/CLI tool table wraps (`crates/automation/src/lib.rs`);
  - the desktop JSON-RPC control channel (`control.rs`) is the schema precedent for a host bridge
    but is not wasm-compiled; a wasm bridge would most simply call `open_bytes`/`execute`/
    `session` directly, or a new postMessage-style inbox alongside `startup_inbox`.
- Key files for the bridge: `apps/pdfcraft-web/src/main.rs`, `apps/pdfcraft-web/index.html`,
  `crates/ui-egui/src/lib.rs` (app struct, inboxes, persistence), `crates/ui-egui/src/files.rs`
  (load/save/dispatch), `crates/ui-egui/src/editing.rs:659` (download), `crates/ui-egui/src/control.rs`
  (JSON method shapes to mirror), `crates/automation/src/tools.rs` (headless tool schemas).
