# A:// Artifacts API — Design

Status: design for review (Phase 2 program, session `artifactsapi-0911`)
Mapping doc: §2 row 16, Eoj amendment 2026-09-11
Decisions amendment 2026-09-12 (Eoj, session `artdecisions-0912`): §6 sharing
tiers (publish + relay) and §8 version retention all DECIDED — see §6, §7,
§8.

## What this is

The artifact library is the platform's best output type — sandboxed HTML
documents the design agent produces and every surface can render. Today it is a
design-mode detail: artifacts live in two IndexedDB stores
(`allternit-design-gallery`, `allternit-design-files`) that only the web app
can see, and the gateway has a *different* artifact API
(`cmd/allternit-api/src/artifact_routes.rs`) that models sectioned text
documents, not rendered artifacts.

This design turns artifacts into the platform's shared content type: one
artifact system, many surfaces. Every artifact gets an address
(`a://artifact/<id>`), a canonical home in the local gateway on :8013, and a
defined way for chat, cowork, design, code, and desktop to create, read, and
version it.

The market context: Anthropic's artifacts have no public API (verified
2026-08 — no create/read/list/delete surface, no export, no addressing).
That wall is their lock-in. Our position is the opposite: artifacts are
first-class, addressable, and open. This doc is about the mechanics, not the
marketing.

Every claim below is grounded in code that exists today; paths are cited.

## 1. Goals / non-goals

**Goals (DECIDED)**

- Artifacts are first-class addressable objects: `a://artifact/<id>`, served
  over the local gateway (`cmd/allternit-api`, default port :8013,
  `main.rs:995`).
- One artifact system, many surfaces: chat, cowork, design, code, and desktop
  all consume the same records through the same API. Surfaces stop owning
  private artifact stores.
- The gateway is canonical. Client-side stores (IndexedDB) become sync caches,
  not sources of truth.
- Non-destructive versioning from day one, aligned with the upcoming
  file-versions work (same version-row shape, same retention questions).
- Provenance is a first-class field: which session/project produced the
  artifact, from what prompt, with which design system and skill.

**Non-goals (DECIDED)**

- **Viewer-pays AI artifacts: OUT.** Deferred per Eoj (2026-09-11 amendment).
  No billing, metering, or marketplace mechanics in this program.
- **Public-by-default anything: OUT.** Sharing is local unless the user takes
  an explicit publish action (§6).
- **Replacing the in-session `<artifact>` streaming format: OUT.** The
  `<artifact type identifier title>` tag format parsed by
  `src/lib/openui/artifact-parser.ts` (`splitOnArtifacts`) stays the
  session-level wire format. The API stores what sessions produce; it does not
  change how streams are emitted.
- **Multi-device sync: not solved here.** Local-first means one gateway, one
  machine (§8).
- **General document store.** The existing `artifacts`/`artifact_sections`/
  `artifact_revisions` tables (V1 baseline, `migrations/V1__baseline_schema.sql`)
  serve the Next.js-style sectioned-document model and stay as-is. The A://
  Artifacts API is a *content-artifact* model (renderable HTML-first objects).
  Same gateway, adjacent tables, no migration of existing rows.

## 2. Data model (DECIDED)

New tables next to the existing document-artifact tables, in the same
`allternit.db` (`main.rs:212`, data dir). Migrations follow the existing
convention (`migrations/V<nnn>__<name>.sql`; latest is V145
`beta_memory_entries`, so these land at **V146**).

### `content_artifacts` — one row per artifact

| Column | Type | Notes |
|--------|------|-------|
| `id` | TEXT PK | `art_<ulid>`; the `<id>` in `a://artifact/<id>` |
| `user_id` | TEXT NOT NULL | from `AuthUser` (same convention as `artifact_routes.rs`) |
| `title` | TEXT NOT NULL | display name |
| `type` | TEXT NOT NULL | MIME-style; html-first (§2.1) |
| `project_id` | TEXT | design project, when produced in one (gallery-store `projectId`) |
| `source_session_id` | TEXT | producing session, when known |
| `prompt` | TEXT | first user message of the producing run (gallery-store `prompt`) |
| `design_system_id` | TEXT | gallery-store `designSystemId` |
| `skill_id` / `skill_name` | TEXT | gallery-store `skillId` / `skillName` |
| `sandbox_policy` | TEXT NOT NULL DEFAULT 'standard' | reference to the execution policy (§8.2) |
| `thumbnail` | TEXT | JPEG dataURL, best-effort (same capture as `artifact-thumbnail.ts`) |
| `current_version` | INTEGER NOT NULL DEFAULT 1 | denormalized pointer to latest version |
| `created_at` / `updated_at` | DATETIME | RFC3339, `chrono::Utc::now().to_rfc3339()` as elsewhere |
| `deleted_at` | DATETIME NULL | soft delete; rows purge later per retention (§8.3) |

The provenance fields deliberately reuse the `GalleryEntry` field names
(`surfaces/ai.allternit.com/src/lib/design/gallery-store.ts:13`) so the
IndexedDB → gateway mapping is mechanical.

### `content_artifact_versions` — one row per immutable version

| Column | Type | Notes |
|--------|------|-------|
| `id` | TEXT PK | `ver_<ulid>` |
| `artifact_id` | TEXT NOT NULL | FK → `content_artifacts.id`, cascade delete |
| `version` | INTEGER NOT NULL | 1-based, monotonic per artifact; unique(artifact_id, version) |
| `body` | TEXT NOT NULL | full artifact content (HTML/SVG/MD source) |
| `body_sha256` | TEXT NOT NULL | content-hash dedupe, same idea as the P0 gallery capture |
| `storage` | TEXT NOT NULL DEFAULT 'inline' | `inline` (row body) or `file` (body on disk, §4) |
| `file_path` | TEXT NULL | set when storage = 'file' |
| `created_at` | DATETIME | |

Versions are append-only. Editing an artifact never rewrites a version row;
it inserts the next one and bumps `current_version`. This is the same shape
the file-versions work is converging on, so the two can share retention and
diff tooling later.

Indexes: `(artifact_id, version)` unique; `(user_id, created_at)` for list;
`(type)`; `(project_id)`.

### `content_artifact_files` — per-artifact project file tree (V159, 2026-09-12)

Mirrors a design project's whole file tree per artifact so the web store's
read-through cache works for every file, not just `/index.html` (§3 file
tree). Upsert-keyed on `(artifact_id, path)`; bodies inline.

| Column | Type | Notes |
|--------|------|-------|
| `artifact_id` | TEXT NOT NULL | FK → `content_artifacts.id`, cascade delete; PK part |
| `path` | TEXT NOT NULL | canonical `/segment/…` project path; PK part |
| `body` / `body_sha256` | TEXT NOT NULL | full file content + content hash |
| `created_at` / `updated_at` | DATETIME | updated bumps on every upsert |

Index: `(artifact_id)`.

### 2.1 Type system (DECIDED shape, typed renderers DECIDED 2026-09-12)

`type` is a MIME-style string. Html-first, per row 1:

- `text/html` — default; rendered in the sandboxed srcDoc iframe
  (`ArtifactRenderer.tsx:58-70`).
- `image/svg+xml` — inline SVG renderer (`ArtifactRenderer.tsx:73-84`).
- `text/markdown` — markdown renderer (as today).
- `application/vnd.allternit.deck`, `…prototype`, `…mobile` — typed
  renderers. Deck already has a real export path
  (`artifact-export.ts` extracts slides from `<deck-stage>` / `.slide`
  patterns for PPTX).

**Typed renderers — DECIDED (2026-09-12, session `artphase2-0912`).** A typed
renderer is a *thin presentation-chrome layer over the same sandboxed srcdoc
iframe* every artifact renders in — not a separate execution environment:

- **Storage does not care.** One body column, one sandbox policy; the MIME
  type selects presentation only. No per-type tables, policies, or CSP
  variants.
- **deck** = the deck-stage skeleton already ships in-document nav (click
  zones, keyboard, hash routes). The typed renderer adds a chrome bar with a
  live slide counter fed by deck-stage's `slideIndexChanged` postMessage
  (validated like the aio-targeting channel: opaque origin, own iframe only)
  and prev/next buttons that navigate through the iframe's `#slide-N` hash.
- **mobile** = the artifact renders inside a 390px device frame (viewport
  metadata made visible).
- **prototype** = the standard sandboxed iframe unchanged. Hotspot linking is
  in-document anchor navigation, which the sandbox already supports; the
  typed case exists so the MIME type maps to a defined renderer rather than
  the default fallback.

Reasoning: the §2.1 open question asked what a typed renderer adds beyond
`text/html` + viewport metadata. The answer after looking at the real deck
skeleton: navigation chrome the host must own (counter, parent-driven nav) —
everything else (hotspots, slide markup) already lives inside the artifact
document. Execution policy stays byte-for-byte identical (no
`allow-same-origin`, CSP untouched) so "typed" can never widen the sandbox.

## 3. API shape (DECIDED, served at :8013 under `/api/v1`)

New route module `cmd/allternit-api/src/content_artifact_routes.rs`, mounted
exactly like `artifact_router()` (`.merge(...)` in `main.rs:783`, inside the
protected `/api/v1` nest — same `Extension<AuthUser>` auth, same
`AppState.db` handle, same `spawn_blocking` + rusqlite transaction pattern as
`artifact_routes.rs`).

Error convention matches the existing API: HTTP status + `{"error": "<message>"}`
(see `create_artifact` in `artifact_routes.rs:397`). List endpoints return
camelCase aliases for web-surface params (`#[serde(alias = "projectId")]`), as
`ListQuery` already does.

### `POST /api/v1/content-artifacts` — create (also creates version 1)

```json
// request
{
  "title": "Q3 deck — launch narrative",
  "type": "application/vnd.allternit.deck",
  "body": "<!DOCTYPE html>…",
  "projectId": "proj_01J…",
  "sourceSessionId": "ses_01J…",
  "prompt": "Build a 6-slide deck for the Q3 launch…",
  "designSystemId": "amber-law",
  "skillId": "od.deck", "skillName": "Deck Builder",
  "sandboxPolicy": "standard",
  "idempotencyKey": "01J…-run-3"
}

// response 201
{
  "artifact": {
    "id": "art_01J8ZK…",
    "address": "a://artifact/art_01J8ZK…",
    "title": "Q3 deck — launch narrative",
    "type": "application/vnd.allternit.deck",
    "version": 1,
    "projectId": "proj_01J…",
    "provenance": { "prompt": "…", "designSystemId": "amber-law",
                    "skillId": "od.deck", "skillName": "Deck Builder",
                    "sourceSessionId": "ses_01J…" },
    "sandboxPolicy": "standard",
    "createdAt": "2026-09-11T19:30:00Z",
    "updatedAt": "2026-09-11T19:30:00Z"
  }
}
```

### `GET /api/v1/content-artifacts/:id` — read (current version body inline)

200 returns the artifact with `body`; 404 `{"error":"not found"}`; rows are
scoped to `user_id` like every other route in the crate.

### `PUT /api/v1/content-artifacts/:id/versions` — append a version

```json
// request
{ "body": "<!DOCTYPE html>…v2…", "idempotencyKey": "…" }
// response 201 — { "version": 2, "artifactId": "art_01J8ZK…" }
```

Also exposed as `PATCH /api/v1/content-artifacts/:id` with `{ "body": … }`
convenience (update = append version + bump pointer), matching the
`PATCH` convention the document-artifact routes already use.

### `GET /api/v1/content-artifacts/:id/versions` — list versions (no bodies)

### `GET /api/v1/content-artifacts/:id/versions/:n` — read one version (body inline)

### `GET /api/v1/content-artifacts?type=&project=&q=&limit=&cursor=` — list

Filterable by `type`, `project`, free-text `q` over title/prompt. Cursor
pagination (limit + `created_at` cursor), the pattern used by the console
memory-store work — no OFFSET paging.

### `DELETE /api/v1/content-artifacts/:id` — soft delete

Sets `deleted_at`; list/get exclude it. Hard purge is a retention concern
(§8.3), not an API concern.

### File tree (Phase 2 multi-file sync, IMPLEMENTED 2026-09-12 — session `artpolish-0912`)

Design-mode projects are a flat per-project tree of files; until this session
only `/index.html` synced to the gateway (as the artifact version body).
`content_artifact_file_routes.rs` (migration V159, table
`content_artifact_files`) mirrors the whole tree per artifact:

- `GET /api/v1/content-artifacts/:id/files` — index: `[{path, sha256, updatedAt}]` (no bodies)
- `GET /api/v1/content-artifacts/:id/files/*path` — read one file `{path, body, sha256, updatedAt}`
- `PUT /api/v1/content-artifacts/:id/files/*path` — upsert write-through `{body}` → `{path, sha256, updatedAt}` (200; natural idempotency via the PK upsert)
- `DELETE /api/v1/content-artifacts/:id/files/*path` — remove one file

Paths are canonical `/segment/…` (no `..`, empty, or trailing-slash segments;
404 cross-user like every route, soft-deleted artifacts unreadable). Bodies are
inline — design project files are small source files; the disk-spill machinery
stays reserved for version bodies. The web store
(`project-file-store.ts`) writes through every save/delete/rename and
read-through-fills a cache-cold browser's whole tree from the index.

### Publish tier (Phase 3, IMPLEMENTED 2026-09-12 — session `artphase3-0912`)

Served by `content_artifact_publish.rs`; state in V150
(`content_artifact_publishes` + `content_artifact_publish_routes`).

### `POST /api/v1/content-artifacts/:id/publish` — publish a version snapshot

```json
// request (version optional; defaults to current_version)
{ "version": 2 }
// response 201 — { "published": true, "artifactId", "version": 2,
//                  "routePath": "u-<hash12>/art_…", "deploymentId",
//                  "deploymentUrl", "url", "publisher", "publishedAt" }
// response 200 — idempotent replay when the same version is already live
```

Implements the §6 publish decisions: (4) artifacts whose `sandbox_policy`
requests network access are rejected with **422** and an error naming the
policy; (2) the publish snapshots the resolved version — later appends do
not change what is live; (1) the deploy lands in the shared Cloudflare Pages
project under a deterministic per-user route prefix (`u-<sha12(user_id)>`,
stored idempotently in `content_artifact_publish_routes`).

The version body is exported as a static site (`index.html`; full documents
pass through, fragments get a standards-mode shell) and deployed through a
narrow publisher interface, `ALLTERNIT_ARTIFACT_PUBLISHER`:
- `wrangler` — real `npx wrangler pages deploy` against the shared project
  (`ALLTERNIT_ARTIFACT_PAGES_PROJECT`, default `allternit-artifacts`).
- `fs` (dev/test default) — immutable deployments under
  `<data_dir>/artifact-publish/deployments/<id>/` with routes as directories
  under `routes/<user_prefix>/<artifact_id>/`; servable by any static file
  server. Not a fake — the same per-user-route-over-immutable-deployment
  layout the shared project uses.

### `GET /api/v1/content-artifacts/:id/publish` — publish status

`{"published": false}` when never published or after unpublish (with
`unpublishedAt` history); full snapshot fields when live.

### `DELETE /api/v1/content-artifacts/:id/publish` — unpublish

Implements decision (3): removes the route only — the Pages deployment
stays immutable (wrangler: the tree is redeployed without the route and the
previous deployment keeps its own `pages.dev` URL; fs: the route directory
is removed, the deployment file stays). 404 when not published.

### Relay tier (org relay, IMPLEMENTED 2026-09-12 — session `relay-0912`)

Served by `content_artifact_relay.rs`; state in V159
(`content_artifact_relay_receipts` + `content_artifact_relay_provenance`).

### `POST /api/v1/content-artifacts/:id/relay` — relay to a peer gateway

```json
// request
{ "target": "gateway-b" }
// response 200 — { "ok": true, "target", "bundleHash", "received":
//                  { "artifactId": "art_…", "version": 1, "bundleHash" } }
// response 200 + "replayed": true — send dedupe: this exact bundle was
//                  already delivered to this target
// response 404 — unknown relay peer (configure ALLTERNIT_RELAY_PEERS)
// response 502 — peer unreachable / rejected the bundle
```

### `POST /api/v1/content-artifacts/relay/inbox` — receive a relay bundle

Public router, `internal_auth::require_internal_token` per-handler (peer
gateways carry the internal service token, not a Clerk JWT; the localhost
local-dev bypass applies). Unpacks, verifies the deterministic bundle hash
+ body sha256, mints a NEW LOCAL id (§6 decision 5), stores provenance.
**Idempotent by bundle hash** — a replayed bundle returns the existing local
id with 200 instead of 201.

### `GET /api/v1/content-artifacts/:id` — relay addressing

Accepts `a://artifact/<id>@<gateway>`-style ids: local hit on the prefix
wins; local miss + `@peer` proxies the read from the named peer (read-through,
nothing persisted; annotated `resolvedVia`/`resolvedFrom`). Relayed reads
attach `provenance.relay`. Exact semantics documented in §6.

### Idempotency (DECIDED)

`Idempotency-Key` header (preferred) or `idempotencyKey` body field. Keyed on
`(user_id, key)`: a repeated create with the same key returns the original
artifact (status 200, not 201) instead of duplicating. Stored in a small
`content_artifact_idempotency` table (key, user_id, artifact_id, created_at,
24h TTL sweep). This fills a real gap — the existing document-artifact routes
have no idempotency today (`create_artifact`, `artifact_routes.rs:397`), and
agent-produced creates are exactly where retries happen. The convention is new
to this crate; the error/status shape stays identical.

## 4. Storage (DECIDED)

- **DB:** `allternit.db` (data dir, `main.rs:212`), migrations V146+
  (`migrations/V146__content_artifacts.sql` and forward), same `DbHandle`
  connection pool as everything else.
- **Inline vs file:** bodies ≤ 256 KB store inline in
  `content_artifact_versions.body`; larger bodies store on disk under
  `<data_dir>/content-artifacts/<artifact_id>/<version>.<ext>` with
  `storage='file'` and the row holding the path + sha256. Thumbnails (already
  JPEG dataURLs from `artifact-thumbnail.ts`) stay inline on the artifact row.
- **IndexedDB relationship:** the gateway is canonical. The web surface keeps
  `allternit-design-gallery` / `allternit-design-files` as offline/read-through
  caches in Phase 1 (sync-on-save; reconcile on launch). Phase 2 moves surfaces
  to fetch-through. The gallery UI does not change — only where its data comes
  from.

## 5. Surfaces (bounded)

This doc does not prescribe per-surface UI. Each surface change is a thin
client of the API; the per-surface build plans belong to their own sessions.

- **Design (web, `surfaces/ai.allternit.com`):** Phase 1 adopter. The P0
  gallery capture (`DesignModeView` → `upsertGalleryEntry`) writes through to
  the gateway instead of only IndexedDB. Gallery list reads gateway-first.
- **Chat:** `splitOnArtifacts` segments gain an optional persist step —
  rendered artifacts can be saved as `content_artifacts` with
  `sourceSessionId` set. Chat rendering itself is unchanged.
- **Cowork:** artifacts produced in cowork runs are created through the API
  with provenance; the cowork rail links `a://artifact/<id>` instead of
  embedding bodies in messages.
- **Code (gizzi-code):** read/write client for CLI-produced artifacts
  (`gizzi artifact save/list/show`), thin adapter over the HTTP API.
- **Desktop:** consumes the same gateway it already bundles (hard-codes
  :8013, `surfaces/allternit-desktop/src/main/unified-main.ts`); no new
  transport.

## 6. Sharing tiers (row 10 — all tiers decided 2026-09-12)

| Tier | Mechanism | Status |
|------|-----------|--------|
| **Local (default)** | `a://artifact/<id>` on the local gateway; only this machine, only authenticated local users | DECIDED |
| **Static export** | Existing client-side pipelines: HTML / PDF / ZIP / PPTX / MP4 (`artifact-export.ts`) | DECIDED — exists, unchanged; export stays client-side in Phase 1 |
| **Hosted publish (Cloudflare Pages)** | Infra exists (the Ops gateway already deploys Pages projects). Publish = explicit user action that exports a version and deploys it to a Pages project under the user's account. | IMPLEMENTED 2026-09-12 (`artphase3-0912`) — `POST/GET/DELETE /api/v1/content-artifacts/:id/publish`; shared project + per-user routes, version-snapshot publish, immutable deployments, sandbox-policy gate |
| **Org relay (A:// mesh)** | Artifact travels between gateways over the mesh (Factory substrate, `factory/`). | IMPLEMENTED 2026-09-12 (session `relay-0912`) — `POST /api/v1/content-artifacts/:id/relay` + public inbox `POST /api/v1/content-artifacts/relay/inbox`; `a://artifact/<id>@<gateway>` read resolution; see "Relay tier — implementation" below |

**Publish tier — decisions (2026-09-12, decided by Eoj; the former OPEN
questions, answered):**
1. **URL shape: shared Pages project with per-user routes** — NOT per-user
   projects. One project, routes namespaced per user
   (`artifacts.allternit.com/<user>/<id>`-style), keeping deploy/ops surface
   flat.
2. **Publish snapshots a version — immutable.** "What you reviewed is what is
   live." A publish points at a specific version row; it does not track
   `current_version`.
3. **Deployments stay immutable.** Unpublish/takedown removes the route only;
   it does not delete the Pages deployment.
4. **Publish IS gated on the sandbox policy for v1.** Artifacts whose
   `sandbox_policy` requests network access are rejected at publish time with
   a clear error.

**Relay tier — decisions (2026-09-12, decided by Eoj; the former OPEN
questions, answered):**
5. **Addressing: mint a new local id on receive; the origin id is carried in
   provenance.** Local ids stay local (the design's lean confirmed); no
   cross-gateway registry lookup.
6. **Trust: NO stricter received sandbox.** Received artifacts render under
   the standard policy; provenance is displayed to the user. There is no
   `sandbox_policy='received'` promotion flow in v1.

**Relay tier — implementation (2026-09-12, session `relay-0912`).** The
decisions above are implemented as decided, in
`cmd/allternit-api/src/content_artifact_relay.rs` (+ migration V159):

- **Send:** `POST /api/v1/content-artifacts/:id/relay` `{target}` packages the
  current version (HTML body + metadata + relay/provenance chain) as a
  portable bundle (`allternit.content-artifact.relay/v1`) and POSTs it to the
  target gateway's inbox. Idempotent per bundle: a retried send with the same
  artifact+version+target replays the original receipt instead of
  re-delivering.
- **Receive:** `POST /api/v1/content-artifacts/relay/inbox` (public router;
  per-handler `internal_auth::require_internal_token`, so the same localhost
  local-dev bypass applies) unpacks the bundle, verifies the deterministic
  bundle hash and body sha256, then mints `art_<uuid4>`, stores the artifact
  (sandbox_policy forced to `standard`; the origin policy is kept in
  provenance) + version 1 + a `content_artifact_relay_provenance` row, and
  records a receipt. **Idempotent by bundle hash:** a replayed bundle returns
  the already-minted local id (200, not 201) — no duplicates.
- **Provenance on read:** `GET /content-artifacts[/:id]` attaches
  `provenance.relay` (`originGateway`, `originArtifactId`, `originVersion`,
  `originSandboxPolicy`, `relayPath[]`, `bundleHash`, `receivedAt`) whenever
  the artifact was received over the relay. The web gallery detail surface
  renders it as a small "Relayed from \<gateway\>" line.
- **`a://artifact/<id>@<gateway>` read resolution:** `GET
  /content-artifacts/:id` accepts an `@peer` suffix. A local hit on the
  prefix always wins. On a local miss with a suffix, the gateway proxies the
  read from the named peer (forwarding the caller's `Authorization` when
  present) and annotates the response `resolvedVia: "relay"` +
  `resolvedFrom`. This is read-through only — nothing is persisted locally;
  import happens only via the explicit relay send/inbox flow. Unknown peer →
  404 naming the peer.
- **Wire transport:** HTTP inbox between gateway base URLs. The Factory UDS
  envelope path needs a listener the gateway does not run, and the Factory
  Bus inbox is keyed per data_dir (`.allternit/bus/queue.db`), so neither can
  span two gateway instances with separate data dirs — the org-mesh topology.
  The bundle is a Factory-shaped envelope and every relayed bundle is
  recorded as a `ContentArtifactRelayed` event in the local Factory ledger
  on both the sending and receiving gateways.
- **Configuration:** `ALLTERNIT_GATEWAY_NAME` (this gateway's name in
  provenance, default `local`) and `ALLTERNIT_RELAY_PEERS` (comma-separated
  `name=http(s)://host:port` peer map, read per request). The receiving user
  of a relayed artifact is the origin user id — the org mesh assumes shared
  user identity across org gateways; per-user cross-org attribution is future
  work.

## 7. Phasing

Each phase is one build session of roughly the size of the sessions that
landed today (PR #378, P0 gallery, ~700 lines).

- **Phase 1 — Gateway CRUD + design-session persistence.** V146 migration,
  `content_artifact_routes.rs` (create/read/list/version/delete + idempotency),
  design-session save writes through the API, gallery reads gateway-first.
  Verification: `cargo test -p allternit-api`, live `curl` smoke against a
  running gateway, typecheck + design vitest on the web surface.
- **Phase 2 — Cross-surface consumption.** Chat persist step, cowork
  `a://artifact/<id>` links, gizzi-code `artifact` client commands, typed
  renderers for deck/prototype/mobile (decision in §2.1), IndexedDB stores
  demoted to read-through cache. Version retention is DECIDED (§8: cap 50,
  admin-configurable, prune oldest) and implemented at append time in
  Phase 1, so Phase 2 has no retention item.
- **Phase 3 — Publish tiers.** Static-export polish plus hosted publish via
  Cloudflare Pages. **IMPLEMENTED 2026-09-12 (session `artphase3-0912`)**:
  the §6 publish routes (`POST/GET/DELETE …/publish`), the V150 publish-state
  migration, the publisher interface (wrangler-backed for the shared Pages
  project, filesystem publisher as the dev/test default), and the
  sandbox-policy publish gate all landed; gallery cards gained
  publish/unpublish actions. The §6 publish-tier answers are implemented as
  decided: shared Pages project with per-user routes, version-snapshot
  publish, immutable deployments (unpublish removes the route only), and the
  publish gate rejecting artifacts whose `sandbox_policy` requests network
  access. Static-export polish (client-side `artifact-export.ts`) remains
  client-side, unchanged. Org relay stays out of Phase 3; its §6 relay-tier
  answers are also decided (new local id on receive with origin id in
  provenance; standard sandbox for received artifacts, provenance displayed).

## 8. Risks / honesty

- **Local-first limits.** One gateway = one machine. Artifacts created on the
  laptop are not on the desktop until the relay tier exists, and there is no
  conflict-resolution story for the same id on two machines. We are not
  solving multi-device sync in this program; the doc says so instead of
  pretending otherwise.
- **Sandbox execution policy.** Today rendering is a srcDoc iframe with
  `sandbox="allow-scripts allow-forms allow-modals"` and a storage shim that
  strips persistence (`ArtifactRenderer.tsx:16-71`) — no `allow-same-origin`,
  no network permission beyond what the sandbox allows by default. The
  `sandbox_policy` column codifies this (row 9's codification work): the
  stored policy string maps to the iframe attribute set at render time, so a
  future policy change is a data change, not a renderer rewrite. The API does
  not execute artifacts; it stores and serves them. Execution stays in the
  surfaces, under the same sandbox as today.
- **Version retention.** Append-only versions grow without bound. DECIDED
  2026-09-12 (Eoj): industry-standard cap of **50 versions per artifact**,
  admin-configurable, pruning the oldest beyond the cap — the same pattern as
  the memory-store cap. Phase 1 implements the cap at version-append time so
  the append-only store never grows unbounded from day one (the Phase 1
  session owns this); the admin-configurable knob lands with it (env or
  config, default 50).
- **Name collision.** "Artifact" already means the sectioned-document model in
  this codebase (`artifact_routes.rs`). The new tables and routes use the
  `content-artifact`/`content_artifacts` prefix to keep the two apart; the
  user-facing name stays "artifact" (nobody sees the table names).

## Appendix — what exists today (the grounding)

| Piece | Path | Role |
|-------|------|------|
| Artifact renderer (sandboxed iframe) | `surfaces/ai.allternit.com/src/components/artifact/ArtifactRenderer.tsx` | Renders html/svg/markdown/mermaid/react artifacts |
| In-session parser | `surfaces/ai.allternit.com/src/lib/openui/artifact-parser.ts` | `splitOnArtifacts` on `<artifact>` tags — unchanged by this design |
| Templates | `surfaces/ai.allternit.com/src/lib/ai/tools/templates/artifact-templates.ts` | Self-contained HTML templates (kind: html/svg/mermaid/jsx) |
| Export pipelines | `surfaces/ai.allternit.com/src/lib/design/artifact-export.ts` | Client-side html/pdf/zip/pptx/mp4 export — the static tier |
| Gallery store (IndexedDB) | `surfaces/ai.allternit.com/src/lib/design/gallery-store.ts` | `GalleryEntry` — provenance field source for this design |
| Project file store (IndexedDB) | `surfaces/ai.allternit.com/src/lib/design/project-file-store.ts` | Per-project file trees — becomes a cache |
| Document-artifact API | `cmd/allternit-api/src/artifact_routes.rs` | Existing `/api/v1/artifacts` (sections/revisions model) — untouched |
| Mount + DB | `cmd/allternit-api/src/main.rs:212,783` | `allternit.db`, router merge pattern this design follows |
| Migration convention | `cmd/allternit-api/migrations/V1…V145` | Next: V146 |
