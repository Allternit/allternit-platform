# Vendor audit — craft/video (FilmCraft → Allternit Video Editor)

Vendored tree: `vendor/craft/video/` (github.com/storytold/filmcraft, pinned rev). License: MIT OR Apache-2.0;
ArtCraft marks removed per the upstream trademark license (`docs/brand/LICENSE-brand.txt`, since deleted with the directory).
Audit date: 2026-10-09. No `cargo build/check/test` was run (machine load cap) — edits were made by careful targeted replacement and verified by grep.

## a. REBRAND SUMMARY

**Changed (user-facing → "Allternit Video Editor"):**
- `docs/brand/` **deleted entirely** (logos, marks, LICENSE-brand.txt, sidecars). Code that embedded it deleted: `crates/ui-egui/src/brand.rs` (was `include_bytes!` of the wordmark PNGs), `pub mod brand;` in `crates/ui-egui/src/lib.rs:33`, wordmark paint calls in `crates/ui-egui/src/panels/dialogs.rs` (About tab) and `crates/ui-egui/src/panels/import_mode.rs` (Home screen). The now-orphaned `image` dependency removed from `crates/ui-egui/Cargo.toml`.
- **ArtCraft community surfaces removed**: header Discord button (`crates/ui-egui/src/header.rs`), About-dialog Discord/website/app-page link rows (`panels/dialogs.rs`), Home-screen community row reworked to project links (`panels/import_mode.rs`), Help-menu items `help.discord`/`help.website`/`help.appPage` (`crates/ui-egui/src/menus.rs:125-127`), macOS app-menu Discord item (`apps/filmcraft/src/native_menu.rs:42`). `crates/ui-egui/src/links.rs` reduced to GitHub + issues (upstream repo, kept for attribution). Test `crates/ui-egui/tests/links_ui.rs` updated to match.
- **UI strings**: window title + dialogs (`apps/filmcraft/src/main.rs:70,95,117,140,214,240,261,264`), About heading/tab title, error-window title (`ui-egui/src/lib.rs:1561`), export/save-dialog file-type labels (`lib.rs:215`, `panels/menu_dialogs.rs:214`, engine `project_tools.rs:126`), graphics-template strings (`panels/graphics_templates.rs:592,672`), crash-recovery messages (`panels/file_dialogs.rs:56,58`), settings notes (`panels/settings.rs:398`, engine `settings.rs:788,943,969,978`), shortcuts preset name `FilmCraft Default` → `Allternit Video Editor Default` (engine `shortcuts.rs:59` + tests), menu labels About/Help/Quit (`menus.rs:132`, `panels/keyboard.rs:131-132`), decoder display names (`crates/codecs/src/video.rs` ×8 + `crates/platform/tests/setting.rs`), renderer string (`crates/project/src/lib.rs:1158`, engine `project_tools.rs:842`), file-provenance strings in exported MCC/STL/LUT/CUBE files (`crates/captions/src/mcc.rs:530,543`, `stl.rs:367`, `crates/color/src/lut.rs:410`, `crates/render/src/luts.rs:78`), OMF/AAF producer strings (`interchange/src/omf/write.rs:349-352`, `aaf/write.rs:336-341`), user-facing error messages (`codecs/src/mxf.rs:281`, `mpeg.rs:263,497,543`, `project/src/gtemplate.rs:213,216,218`, `engine/graphic_templates.rs:99`, `interchange/src/comp.rs:832`, `fcp7.rs:1012`, engine `shortcuts.rs:1040`, gtemplate author + end-credit body `project/src/gtemplate.rs:462,599`), Windows version-info resources (`apps/filmcraft/build.rs`), web HTML (`apps/filmcraft-web/web/index.html` title/loading/fatal/error strings), web API error (`apps/filmcraft-web/src/api.rs:81`), packaging display names (`packaging/macos/Info.plist.in`, `packaging/linux/ai.storyteller.filmcraft.metainfo.xml.in`, `packaging/windows/filmcraft.wxs`, packaging scripts' echo strings, DMG volume name).
- **i18n catalogs**: `es.tsv` — 19 rows re-keyed/re-valued, 6 ArtCraft/community rows deleted; `pt-br.tsv` — 4 rows re-keyed, 3 deleted (its "every entry must be a current menu label" test is kept consistent); `ja.tsv` had no occurrences. All new `tl!` strings have Spanish entries (required by `spanish_translates_every_tl_literal`).
- **Cargo.toml descriptions** (13 files): `FilmCraft …` → `Allternit Video Editor …`. Package names untouched.
- **README.md**: top note added — *"Vendored, rebranded copy … Based on FilmCraft by the ArtCraft team (MIT/Apache-2.0)"*; ArtCraft logo header, Discord badges, getartcraft link rows, "Crafting Apps" promo table, star-history widget removed (replaced by an "The upstream project" attribution section); prose rebranded. `NOTICE`, `ATTRIBUTION.md` (8 brand rows + paragraph removed), `AGENTS.md` §1.8 (brand exception struck, removal documented) updated. All `*.md` docs (37 files) swept `FilmCraft` → `Allternit Video Editor` in prose.

**Deliberately left (machine names / file-format identifiers, per the rules):**
- Crate names (`filmcraft-*`), binary names (`filmcraft`, `filmcraft-cli`), reverse-DNS ids (`ai.storyteller.filmcraft`), `.fcproj`/`.fcgt` formats, MCP resource URIs `filmcraft://document`/`filmcraft://commands`, the `window.filmcraft` JS object name, env vars (`FILMCRAFT_CONTROL_PORT`, …), data/log directories (`~/Library/Application Support/FilmCraft`, `%APPDATA%\FilmCraft`, `FilmCraft Previews`, `FilmCraft Logs`), and the `.fcproj` generator tag (`crates/format/src/lib.rs:105,284` — files we write still say `generator: "FilmCraft <ver>"`; historical fixtures in `crates/format/tests/fixtures/` intentionally unchanged).
- **Interchange round-trip keys** (changing them would break file interop with upstream-written AAF/OMF/OTIO): `FilmCraft:SCLP:ClipName`, `FilmCraft:EFFE:EffectID` (OMF), tagged values `FilmCraft Effect` / `FilmCraft Frame Size` / `FilmCraft Audio Sample Rate` (AAF), OTIO `generator_kind: "FilmCraftGenerator"` and `metadata.filmcraft`.
- Code doc comments still say "FilmCraft" (internal prose, not user-visible); upstream GitHub/issue links kept as attribution.

**Could not fully neutralize / judgment calls:**
- The macOS app bundles/CI still build from `packaging/` with binary name `FilmCraft` inside `MacOS/` (machine name, kept per rule).
- es/ja translations of long settings notes were re-pointed to the new English keys with "Allternit Video Editor" substituted into the Spanish text (not a professional re-translation).
- Desktop Linux/Windows packaging metadata files (desktop file `Name=`, etc.) still carry some upstream strings in `packaging/linux/*.desktop` templates — display names there come from the metainfo/plist which were updated; low-value for the WASM-only embedding.
- Not compiled (build cap): consistency verified by grep; the i18n sync tests and `links_ui.rs` were reasoned through but not executed.

## b. TELEMETRY / NETWORK INVENTORY

**No telemetry, analytics, crash-reporting, Sentry/PostHog/segment SDKs exist anywhere in the tree** (grep for `telemetry|analytics|sentry|posthog|bugsnag|mixpanel|crashreport` — zero hits in code). Crash logs are local files only (`crates/ui-egui/src/crash.rs` → `<data dir>/Logs/crash-*.log`). The web build explicitly states "Nothing is uploaded anywhere" (`docs/web.md:6`).

Every network-capable site:

| file:line | What it does |
|---|---|
| `crates/speech/Cargo.toml:17,28` | Optional `download` feature pulling `ureq 3` (+ rustls, rustls-rustcrypto, sha2). The **only HTTP client dependency in the workspace** (no reqwest/minreq/attohttpc/hyper/isahc anywhere). |
| `crates/speech/src/models.rs:53,68,108,148` | Pinned model catalogue: `https://huggingface.co/openai/whisper-{tiny,base,small}/resolve/<rev>/<file>` (OpenAI Whisper weights for the optional Transcript/speech-to-text feature; off by default). |
| `crates/speech/src/models.rs:224-271` | `download()`: streams each missing file via `agent.get(f.url)` into `*.part`, SHA-256 + size verified against pinned catalogue, then renamed into place. Cancelable. |
| `crates/speech/src/models.rs:274-287` | `agent()`: ureq with pure-Rust TLS (rustls + RustCrypto provider, `RootCerts::PlatformVerifier`), 30 s connect timeout. |
| `apps/filmcraft/src/control_server.rs:27-34` | Desktop **control channel**: `TcpListener::bind(("127.0.0.1", port))`, opt-in via `--control <port>`/`FILMCRAFT_CONTROL_PORT`. JSON-lines request/response. Deliberately rejects HTTP request lines (anti-smuggling, tested at `control_server.rs:197`); loopback only, no auth — documented "only enable it while you use it" (`docs/control-protocol.md:16-17`). |
| `crates/automation/src/bridge.rs:25-29` | MCP bridge client: connects **out** to `127.0.0.1|localhost|[::1]` only; any other host refused. |
| `crates/automation/src/server.rs:181` | MCP server is **stdio only** (`filmcraft-cli mcp`), no sockets. |
| `xtask/src/main.rs:340-375` | `cargo xtask web --serve`: dev-only localhost static file server (binds 127.0.0.1), sends COOP/COEP headers. Not part of the app. |
| `apps/filmcraft-web/src/api.rs:106-116, 193-200` | `importUrl(url, name?)` / `openProject(url)`: browser `fetch()` of a URL **by the host page's origin** (same-origin in practice; test helper per `docs/web.md:120`). No arbitrary server-side fetch — this is client-side JS fetch from the embedding page. |
| `apps/filmcraft-web/tests/smoke.mjs:55` | Test-only: headless Chrome DevTools protocol on `http://127.0.0.1:<port>/json`. |
| `crates/ui-egui/src/links.rs:13-14` | `links::open` → `egui::OpenUrl` opens the **system browser** (new tab) at the upstream GitHub repo / issues — only on explicit user click (About dialog, Help menu, Home screen). The upstream getartcraft.com/Discord links were removed in the rebrand. |
| `build.rs` files (×3: `apps/filmcraft`, `crates/ui-egui`, `crates/text`) | No network. Read local files only (icon, contributor credits, optional `CRAFT_FONTS_DIR`). |
| `.github/workflows/*` | CI only: standard actions, crates.io `wasm-bindgen-cli` install, release uploads to GitHub Releases. Not shipped code. |

Web runtime surfaces that are **not** network: OPFS (Origin Private File System), WebCodecs, AudioWorklet, Blob/object-URL downloads — all same-origin browser APIs.

## c. UNSAFE CODE INVENTORY

Workspace policy (`Cargo.toml:74`): `unsafe_code = "forbid"` for every crate. The single exception is `crates/platform` (`crates/platform/Cargo.toml:10-13`): `unsafe_code = "deny"` + `#[allow(unsafe_code)]` only on FFI modules, `// SAFETY:` comment on every block, safe `Result`-returning API (ADR `docs/adr/0001-platform-ffi.md`).

Grep confirms **zero `unsafe` outside `crates/platform`** (all other crates advertise "no unsafe" in their crate docs, e.g. `crates/cfb/src/lib.rs:16`, `crates/matroska/src/lib.rs:11`).

| file | `unsafe` sites | why |
|---|---|---|
| `crates/platform/src/videotoolbox.rs` | 22 | macOS VideoToolbox hardware **decode** FFI (CMBlockBuffer/CVPixelBuffer bridging to the engine's frame type). |
| `crates/platform/src/videotoolbox_encode.rs` | 40 | macOS VideoToolbox hardware **encode** FFI (H.264/H.265 export). |
| `crates/platform/src/media_foundation/mft.rs` | 36 | Windows Media Foundation transform FFI (hardware decode; graceful Media-Foundation-missing fallback). |
| `crates/platform/src/media_foundation/gpu.rs` | 16 | Windows D3D11 texture/ shared-handle import of hardware-decoded frames. |
| `crates/platform/src/nvenc/ffi.rs` | 18 | NVENC driver entry points (`unsafe extern "C" fn` signatures, dynamically loaded). |
| `crates/platform/src/nvenc/device.rs` | 7 | NVENC device open/close lifetime management. |
| `crates/platform/src/nvenc/session.rs` | 17 | NVENC encode session calls (Windows/Linux hardware export). |
| `crates/platform/src/vaapi/ffi.rs` | 27 | Linux VA-API entry points (`unsafe extern "C" fn`, dynamically loaded). |
| `crates/platform/src/vaapi/va.rs` | 33 | VA-API decode session/config/surface management (H.264, HEVC). |
| `crates/platform/src/lib.rs`, `media_foundation/mod.rs`, `nvenc/mod.rs`, `vaapi/mod.rs` | `#[allow(unsafe_code)]` scoping only | module-level opt-ins that confine unsafe to the FFI files above. |

All hardware paths are opt-in/preference-gated with the pure-Rust software decoder as the tested fallback. The WASM web build compiles none of this (no OS FFI on wasm32); it uses WebCodecs instead.

## d. `window.filmcraft` HOST API (web build JS surface — reference shape)

Object installed unconditionally at startup on the page's `window` (`apps/filmcraft-web/src/api.rs:215-216`, called from `src/lib.rs:143`). **There is no origin/allowlist check**: any script running on the embedding page can call it; security must come from the host page (it is the same trust domain as the page itself). The desktop equivalent is the loopback TCP control channel; the JS object is the exact same method table exposed as promises (`docs/web.md:107-133`).

**Plumbing.** Each call builds a `ControlRequest { method, params, reply: Sender }` (`crates/ui-egui/src/control.rs:34-47`) and sends it over an `mpsc` channel into the running `FilmcraftApp` (wired at `apps/filmcraft-web/src/lib.rs:191-193` via `with_control`). Replies are pumped once per UI frame (`api.rs:51-70`, called from `WebApp::logic`, `lib.rs:102`). Resolution contract: reply `{"ok": true, "result": …}` → resolve with `result`; otherwise reject with a real JS `Error` whose message is the control error (`api.rs:55-59, 81, 225-234`). Single-threaded: requests execute between frames on the UI thread.

**Methods** (all defined in `api.rs:119-218`):

| JS call | control method | notes |
|---|---|---|
| `filmcraft.execute(command, params?)` | `engine.execute` | any of the 650+ engine command ids (`docs/control-protocol.md`). Promise → command result. |
| `filmcraft.request(method, params?)` | any control method | direct escape hatch: `ui.inspect`, `ui.click`, `ui.key`, `ui.playback`, `ui.menu.invoke`, `perf.stats`, … |
| `filmcraft.commands()` | `engine.commands` | full command registry. |
| `filmcraft.inspect()` | `ui.inspect` | UI state. |
| `filmcraft.screenshot(params?)` | `ui.screenshot` | resolves `{pngBase64, width, height}` (base64 PNG of the canvas or a panel). |
| `filmcraft.importFiles(File\|FileList\|File[])` | — | registers each under `/files/<name>` (dedupe: `name (2).ext`, `fs.rs:75-99`), imports into the current bin; resolves `{items, errors, paths}` (`import.rs:107-134`). |
| `filmcraft.importUrl(url, name?)` | — | `fetch()` + import (tests). |
| `filmcraft.openProject(fileOrUrl)` | — | `File` or URL → `/projects/<name>` → `file.open` (`import.rs:136-142`). |
| `filmcraft.files()` | — | synchronous: virtual file table `[{path, size, kind}]`, kind ∈ `blob`/`memory` (`fs.rs:107-120`). |
| `filmcraft.readFile(path)` | — | promise → `Uint8Array` of an in-memory/virtual file (e.g. an export) (`api.rs:169-180`). |
| `filmcraft.info()` | — | synchronous environment JSON: `backend`, `compositor` (`gpu`/`cpu`), `crossOriginIsolated`, `threads: false`, `frameWorkers: "cooperative"`, `opfs`, `audio`, `restoredMedia`, `startupMs`, `webcodecsStats`, `fetchesInFlight` (`lib.rs:130-136, 197-220`, `api.rs:150-160`). |

**Documents open/save model.** There is no direct "save()" call: the host page drives the engine's own commands (`file.save`, `file.exportMedia`, …) via `execute`. All file I/O goes through a virtual file table (`fs.rs`): user media become `Blob` entries served to demuxers by 1 MiB cached range reads; anything the app *writes* (saved projects, exports, captions) lands as an in-memory entry and is simultaneously offered to the user as a browser download via a hidden `<a download>` object-URL click (`fs.rs:409-425`); `/opfs/` paths instead go to the Origin Private File System (auto-save snapshot every 5 s + media copies, restored on next visit; `src/recovery.rs`, `src/opfs.rs`). `readFile` is how a host fetches export bytes back out.

**Startup handshake.** `window.filmcraftLoad = {wasmMs, readyMs, fatal?}` is set by `index.html:45-65, 77-79`; a Rust panic flips `filmcraftLoad.fatal` and shows a "stopped working" overlay with Reload. Readiness = `readyMs` present.

**Design notes for the Allternit unified host bridge:** promise-per-request with `result`-or-`Error` semantics; method string table shared with the desktop TCP channel and MCP; synchronous getters (`files`, `info`) alongside async calls; import by host-provided `File` handles (no upload); save = engine command + host-side download hook; load/error surface via a small `*Load` state object. Automation-id element targeting (`ui.elements`/`ui.click`) gives hosts a second, layout-independent drive path.

## e. WEB BUILD SUMMARY (`apps/filmcraft-web`)

Entry: `src/lib.rs:124` `#[wasm_bindgen] start(canvas_id)` (canvas id `filmcraft_canvas`), bootstrapped by `web/index.html`'s module script (cache-busting `?v=<build-hash>` on the wasm-bindgen glue + `.wasm`; `.wasm` served as `application/wasm`). Build: `cargo xtask web [--dev] [--serve PORT]` → `target/web/dist` (index.html, `filmcraft_web.js`, `filmcraft_web_bg.wasm`, `audio-worklet.js`, favicon); wasm-bindgen CLI must match crate 0.2.129; optional wasm-opt.

Same engine + egui UI as desktop under eframe's WebRunner; **WebGPU with WebGL2 fallback** (`?webgl` forces; auto-reload retry on WebGPU start failure). URL flags: `?empty ?norecover ?fresh ?cpu ?webgl ?nowebcodecs`.

**Single-threaded by design** (docs/web.md:55-62): no atomics wasm build; frame jobs, decode, mix and export run cooperatively on the UI thread — frame server pumped between frames (24 ms budget playing / 40 ms idle), exporter stepped 30 ms per frame, `rayon` runs inline. `filmcraft.info()` reports `crossOriginIsolated`/`threads` so a future threaded build can choose at startup. COOP/COEP isolation is what `--serve` sends and what a threaded build would require; the current build runs fine without it.

**Media under wasm** (docs/web.md:64-99): containers read via `BlobReader` — 1 MiB chunks, 384 MiB LRU, async `Blob.slice().arrayBuffer()` with read-ahead; uncached reads fail `WouldBlock` and the frame server/import retries when bytes arrive. WebCodecs `VideoDecoder` is registered ahead of the built-in decoders for H.264/HEVC/VP9/AV1 in MP4/MOV (`?nowebcodecs` disables; per-source fallback to FilmCraft's own pure-Rust decoders on any decoder error; support probed with `VideoDecoder.isConfigSupported`). Audio: WebAudio `AudioWorklet` (`web/audio-worklet.js`), UI thread mixes 250 ms ahead in 2048-frame blocks, played-frame clock is the master. GPU compositor on WebGPU; CPU compositing on WebGL2 (`info().compositor`).

**Persistence**: crash-recovery journal = OPFS `recovery/snapshot.fcproj` every 5 s while dirty + background copies of imported media (≤ 4 GiB) under `media/`; next visit reopens the snapshot (`?fresh`/`?norecover` disable). Files the app writes are offered as downloads (see §d).

**Tests**: `tests/smoke.mjs` (headless Chrome over DevTools protocol, no npm deps): load → demo playback → generated-MP4 import → playback → H.264 export download; plus `import-bins.mjs`, `import-collision.mjs`, `tests/recovery_policy.rs`. Panics surface via the `window.filmcraftLoad.fatal` overlay instead of a dead canvas. Web-only restrictions: no `std::env::temp_dir`, `Instant/SystemTime::now`, `thread::sleep/spawn`, `process::id` (panics on wasm32 — use `web_time`/engine helpers); thread-needing commands (render previews, proxies, Project Manager, mask tracking) return errors.
