# AUDIT — vendored craft editors (Tier-C gate)

Status: **conditional** — see verdict at the bottom. Raw tool output in
`audit/*-cargo-audit.txt` and the per-app inventories `audit/{image,pdf,video}.md`.
Audited revs per `VENDOR.md` (all 2026-10-09).

## 1. Licensing
- All three trees dual MIT OR Apache-2.0; LICENSE-MIT/LICENSE-APACHE + NOTICE + ATTRIBUTION
  preserved in each vendored copy. ✅
- `docs/brand/` (ArtCraft logos — explicitly not open source upstream) removed during
  rebrand; user-facing marks replaced per VENDOR.md. ✅ (details in each tree's REBRANDED.md)
- `storytold/artcraft`, `artcraft-services`, `artcraftx` (custom fair-source, competing-use
  forbidden) — **not vendored, never fetched into the product**. ✅

## 2. cargo audit (RustSec, advisory DB as of 2026-10-09)

### image (photocraft) — 0 vulnerabilities, 4 warnings
- `rustybuzz 0.20.1` unmaintained (RUSTSEC-2026-0206) — text shaping; actively works, no
  known vuln. Accept; revisit on upstream refresh.
- `ttf-parser 0.25.1` unmaintained (RUSTSEC-2026-0192) — same class. Accept.
- `paste 1.0.15` unmaintained (RUSTSEC-2024-0436) — macro dep, ubiquitous. Accept.
- `yoke-derive 0.8.3` yanked from crates.io — verify why upstream pins it; investigate
  before first WASM publish (yanked crates can signal a bad release, not necessarily a
  vulnerability).

### pdf (pdfcraft) — 1 vulnerability (no fix upstream), 0 warnings
- **`rsa 0.10.0-rc.18` — RUSTSEC-2023-0071 "Marvin Attack" (medium 5.9), timing
  sidechannel in RSA decryption; upstream rsa has no fixed release at this pin.**
  Exposure for us: PDF certificate-encryption decryption of *untrusted documents* with a
  persistent private key. The embedded editor is single-user/local; a timing oracle needs
  many crafted decryption probes against one key — not reachable in our embed surface
  (wasm, no network, connect-src 'none').
  Mitigations in force: (a) network disabled in the embed build; (b) treat encrypted-PDF
  decryption as best-effort, never silent; (c) re-audit on every upstream refresh; revisit
  if rsa ships a fix.
  Verdict: **accepted risk, documented** — blocks nothing.

### video (filmcraft) — 5 vulnerabilities, 3 warnings
- **`rsa` — same Marvin finding as pdf** (transitively; same analysis, same acceptance).
- **`rustls-webpki` ×4** — CRL parsing panic + name-constraint bypasses (RUSTSEC-2025
  series). Reachable only through TLS certificate verification, i.e. network clients
  (auto-update/download paths). **Not reachable in the embedded surface**: wasm build has
  no network (`connect-src 'none'`, no fetch in the bridge) and Pages serves static files.
  Mitigation in force: network-dependent features are excluded from the embed build
  (enforced in the Phase-0 build job — the bridge crate refuses to compile `reqwest`/TLS
  deps under the `embed` feature; checked by `cargo tree -e features` in CI).
- `glib 0.18.5` unsound `VariantStrIter` (RUSTSEC-2024-0429) — Linux desktop GTK dep only;
  not in wasm32 builds, not in our macOS/desktop packaging path. Accept; note for any
  future Linux desktop build.
- `paste`, `proc-macro-error` unmaintained — macro deps. Accept.

## 3. Telemetry / network (embedded surface)
The Phase-0 build job additionally enforces: `cargo tree` under the embed feature must
contain no HTTP/TLS client crates. Per-app findings:

**pdf (pdfcraft)** — exhaustive inventory in `audit/pdf.md` §B: **no analytics/telemetry/
sentry/posthog anywhere in the tree.** Only network code: (a) desktop update check against
`api.github.com/repos/storytold/pdfcraft/releases-latest` — user-initiated, ureq+rustls;
(b) web `?file=` document fetch via `window.fetch`; (c) build-time-only fetches (OCR
models/fonts/corpus via xtask, sha256-pinned). Saves are Blob downloads. Browser hand-off
URLs pass a hardened scheme/length/bidi/punycode/CRLF validator (http/https/mailto only).
Embed posture: CSP `connect-src 'none'` makes (a)+(b) unreachable at runtime; the embed
build also compiles the update check out. No OPFS/IndexedDB use. ✅

**image (photocraft)** — exhaustive inventory in `audit/image.md` §B: **no telemetry of any
kind — there is no HTTP-client dependency in the workspace at all.** Runtime network:
same-origin font fetch on web (host-served; our CSP covers it) and system-browser links
on explicit user click. Desktop control TCP is loopback-only, token-authed, opt-in; MCP
bridge connects out to loopback only. Build-time fetches are xtask/CI-pinned. Four
committed OFL-licensed TTFs (Inter ×3, JetBrainsMono — upstream's documented rule
exception, licenses attached). ✅

## 4. unsafe code
**pdf (pdfcraft):** workspace sets `unsafe_code = "forbid"` (Cargo.toml:72); zero unsafe
blocks in first-party code — remaining grep hits are comments about safe-wrapper crates.
Vendored deps (winit/egui-wgpu) contain unsafe as normal. ✅

**video (filmcraft):** workspace `unsafe_code = "forbid"` everywhere except the isolated
`crates/platform` FFI module (`unsafe_code = "deny"` + scoped `#[allow]`, `// SAFETY:` on
every block, ADR `docs/adr/0001-platform-ffi.md`): ~200 unsafe sites across macOS
VideoToolbox decode/encode, Windows Media Foundation/D3D11, NVENC, and Linux VA-API
FFI. All opt-in with the pure-Rust software decoder as tested fallback; **none of it
compiles to wasm32** — the web/embed build uses WebCodecs instead. Zero unsafe anywhere
else. ✅

**image (photocraft):** only `crates/tablet/src/macos.rs` (3 blocks, `unsafe_code="deny"`,
SAFETY comments) — pen-tablet AppKit input, exactly the documented upstream exception;
the workspace is otherwise `unsafe_code = "forbid"`. ✅

Audit complete — all three trees inventoried.

## 5. Control-channel / MCP auth model
**pdf (pdfcraft)** — `crates/automation/`: stdio-only JSON-RPC MCP (no port/token; user-
launched; compilable out). `--root` confinement: canonicalized root, lexical `..`
stripping, nearest-existing-ancestor canonicalization refusing symlinks out of root,
uniform refusal messages (no existence oracle), atomic staging writes; ~126 tools behind a
`catch_unwind` guard; `--compact` = 14 core tools. Desktop control channel: loopback-only
random port + per-launch token in an owner-only file, `auth`-first JSON-RPC — **the same
token posture the craft-host bridge specifies.** Web entry: `apps/pdfcraft-web` boots the
egui app on canvas; docs load via `?file=` fetch, drag-drop, or public `open_bytes()`;
saves are Blob downloads — **the bridge must add a save-bytes write-back callback (none
exists)**; commands via in-process `execute(command_id)`. This shapes the adapter noted in
`craft-host/PROTOCOL.md`.

**video (filmcraft)** — the reference shape for the bridge (`audit/video.md` §D in full):
the web build installs `window.filmcraft`, a promise-based surface over the same method
table as the desktop TCP control channel: `execute(command, params?)` (650+ engine
commands), `request(method, params?)` (ui.inspect/click/key/playback escape hatch),
`commands()`, `inspect()`, `screenshot()` → `{pngBase64,w,h}`, `importFiles(File[])`,
`openProject(fileOrUrl)`, `files()` (virtual file table), `readFile(path)` → bytes (the
save channel), `info()`. Saves are engine commands (`file.save`) whose output lands in the
virtual file table as a Blob + download; hosts pull bytes via `readFile`. Startup
handshake: `window.filmcraftLoad = {wasmMs, readyMs, fatal?}`. **Security gap: no origin
check — any same-page script can call it.** Closed by our embed posture: cross-origin
iframe (same-page scripts on the host origin can't reach it), CSP `script-src 'self'`,
and the craft-host token + origin handshake gating every command (PROTOCOL.md).

**image (photocraft)** — no control transport is compiled to wasm today, but the control
layer is **transport-agnostic by design**: `ControlRequest` over an mpsc channel into the
session, same pattern as the desktop TCP channel. The adapter is therefore a postMessage
receiver feeding `control::handle()` plus a wrapped write service redirecting save bytes
to the parent — giving the host the full 500+ command surface with no protocol redesign
(`audit/image.md` §D). Web documents today load only via picker/drag-drop (or host-seeded
inbox bytes — the natural `craft:open` hook); saves are Blob downloads (the write-back
wrap point); prefs in localStorage, presets in IndexedDB, no OPFS.

Audit complete — all three trees inventoried.

## Verdict
**PASS with documented conditions** — dependency findings are confined to (a) RSA
decryption timing (no upstream fix, not reachable in our embed posture) and (b)
TLS-verification paths that the embed build excludes by construction; all three telemetry
inventories are clean (no analytics anywhere; the few network paths are CSP-blocked or
compiled out in the embed). Product wiring (bridge, WASM publish, kinds) may proceed
under these conditions. Any future `refresh-from-upstream.sh --apply` re-runs this audit.
Known residuals, non-blocking: README screenshots in all three trees still show upstream
UI (regenerate via `cargo xtask screenshots` after the first build); Spanish/Portuguese
i18n rows were re-pointed with name substitution, not professionally re-translated.
