# Artifacts v2 — contract (Phase 1 foundation)

Status: **accepted 2026-10-08** (Eoj). Plan and phases: `allternit-workspace/PLAN-artifacts-v2-2026-10-08.md`. Product spec this copies: `allternit-workspace/reports/Claude artifacts replication spec.md` (Anthropic's Claude artifacts as of 2026-10-08).

This document is the contract every Phase 1 workstream builds against: the cloud store (allternit-cloud-api), the app (allternit-ai), and the model tools (gizzi-code). Change it only by PR, and update all three sides.

## 1. Model

One account-level store of **typed artifacts** in **allternit-cloud-api** (Postgres). Every surface (Desktop, ai.allternit.com, the phone layout, the m.allternit.com PWA, cloud computers, gizzi) reads and writes the same records. An artifact is something the assistant (or a person) made that you'd show someone: a doc, deck, sheet, design, dashboard, motion, page, card, diagram, image or code file.

Every record has a `kind` and a `runtime_version`:

- `runtime_version = 2` — current artifacts (this contract).
- `runtime_version = 1` — **legacy**: imported from the pre-v2 local stores (content-artifacts, sectioned artifacts, canvases, Cowork's browser store). They open, can be shared, and keep the behaviour they had; they don't get v2 runtime features (storage, AI, connectors) until re-saved as v2 by an editor.

### Kinds

| kind | body_format (MIME) | Editor (app) | Phase |
|---|---|---|---|
| `doc` | `application/vnd.allternit.doc+json` (or `text/markdown` for imports) | Docs editor | 1 shell, 4 editor |
| `sheet` | `application/vnd.allternit.sheet+json` | Sheets editor | 1 shell, 4 |
| `slides` | `application/vnd.allternit.slides+json` | Slides editor | 1 shell, 4 |
| `design` | `application/vnd.allternit.design+json` or `text/html` | Design canvas | 1 shell, 4 |
| `dashboard` | `application/vnd.allternit.openui` (OpenUI Lang, dashboard library) | Dashboard | 2 |
| `motion` | `application/vnd.allternit.motion+json` (scenes + props) | Motion editor | 3 |
| `page` | `text/html` or `text/markdown` | Page viewer (sandboxed) | 1 view, 4 runtime |
| `card` | `application/vnd.allternit.openui` (answers library) | Interactive card | 1 |
| `diagram` | `text/vnd.mermaid` or `image/svg+xml` | Diagram viewer | 1 |
| `image` | `text/uri-list` (URL to the file store) | Image viewer | 1 |
| `code` | `text/plain` + `language` in `meta` | Code viewer | 1 |

Unknown kinds render as `page` if HTML, otherwise as code. The kind list lives in one shared file per side: `cmd/allternit-cloud-api/src/artifacts/kinds.rs` and `allternit-ai/src/lib/artifacts/kinds.ts`.

## 2. Postgres schema (`migrations_pg/085_artifacts_v2.sql`, idempotent)

```sql
CREATE TABLE IF NOT EXISTS artifacts (
  id               TEXT PRIMARY KEY,            -- 'art_' + ULID; clients may supply it (idempotent create)
  owner_id         TEXT NOT NULL,               -- Clerk user id
  org_id           TEXT,                        -- owner's Clerk org at creation (sharing follows it)
  kind             TEXT NOT NULL,
  runtime_version  INTEGER NOT NULL DEFAULT 2,
  title            TEXT NOT NULL,
  icon             TEXT,                        -- one generic word, e.g. 'chart'
  template_id      TEXT,
  origin           JSONB NOT NULL DEFAULT '{}', -- {surface, session_id, message_id, computer_id, legacy_source, legacy_id}
  capabilities     JSONB NOT NULL DEFAULT '{}', -- {storage:bool, ai:bool, connectors:[{connector, tools:[...]}]}
  current_version  INTEGER NOT NULL DEFAULT 1,
  shared_version   INTEGER,                     -- NULL = viewers always see the latest
  visibility       TEXT NOT NULL DEFAULT 'private', -- private | people | org | link
  link_level       TEXT NOT NULL DEFAULT 'view',    -- what 'anyone with the link' may do (view only in v1)
  thumbnail_url    TEXT,
  created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS artifacts_owner_updated_idx ON artifacts(owner_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS artifacts_org_visibility_idx ON artifacts(org_id, visibility);

CREATE TABLE IF NOT EXISTS artifact_versions (
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  version      INTEGER NOT NULL,
  body         TEXT NOT NULL,                   -- ≤ 16 MiB
  body_format  TEXT NOT NULL,
  meta         JSONB NOT NULL DEFAULT '{}',      -- {language, filename, note, ...}
  size_bytes   INTEGER NOT NULL,
  sha256       TEXT NOT NULL,
  author_id    TEXT NOT NULL,                    -- user id, or 'assistant'
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (artifact_id, version)
);

CREATE TABLE IF NOT EXISTS artifact_shares (
  artifact_id     TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  principal_type  TEXT NOT NULL,                -- user | email | group
  principal_id    TEXT NOT NULL,                -- user id, lower-cased email, or Clerk group id
  level           TEXT NOT NULL,                -- view | comment | edit
  invited_by      TEXT NOT NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at      TIMESTAMPTZ,                  -- outside email invites: +30 days until accepted
  accepted_at     TIMESTAMPTZ,
  PRIMARY KEY (artifact_id, principal_type, principal_id)
);

CREATE TABLE IF NOT EXISTS artifact_storage (       -- Phase 4 runtime; schema now
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  scope        TEXT NOT NULL,                  -- personal | shared
  user_id      TEXT NOT NULL DEFAULT '',       -- '' for shared
  key          TEXT NOT NULL,
  value        TEXT NOT NULL,
  bytes        INTEGER NOT NULL,
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (artifact_id, scope, user_id, key)
);

CREATE TABLE IF NOT EXISTS artifact_consents (
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  user_id      TEXT NOT NULL,
  capability   TEXT NOT NULL,                  -- storage_shared | ai | connectors
  granted      BOOLEAN NOT NULL,
  denied_tools JSONB NOT NULL DEFAULT '[]',
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (artifact_id, user_id, capability)
);

CREATE TABLE IF NOT EXISTS artifact_comments (      -- Phase 4 UI; schema now
  id           TEXT PRIMARY KEY,
  artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
  version      INTEGER,
  anchor       JSONB NOT NULL DEFAULT '{}',
  parent_id    TEXT,
  author_id    TEXT NOT NULL,
  body         TEXT NOT NULL,
  to_assistant BOOLEAN NOT NULL DEFAULT false,
  resolved_at  TIMESTAMPTZ,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS org_artifact_settings (
  org_id             TEXT PRIMARY KEY,
  enabled            BOOLEAN NOT NULL DEFAULT true,
  templates          JSONB NOT NULL DEFAULT '{}',   -- {kind: bool}; missing = plan default
  external_sharing   BOOLEAN NOT NULL DEFAULT false,
  outside_invites    BOOLEAN NOT NULL DEFAULT false,
  presence           BOOLEAN NOT NULL DEFAULT true,
  connectors         BOOLEAN NOT NULL DEFAULT true,
  allowed_external   JSONB NOT NULL DEFAULT '[]',   -- artifact ids individually allowed outside
  updated_by         TEXT,
  updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

## 3. HTTP API (allternit-cloud-api, prefix `/api/v2/artifacts`)

All routes resolve the caller with `auth::resolve_user_scoped(.., "compute")` except the public link route. Errors use the standard `ApiError` JSON. A new `/api/v2` prefix keeps clear of every existing `/api/v1/artifacts*` path (allternit-api's sectioned store, which Desktop proxies on the same origin scheme).

### Access

`my_access` is computed per request: `owner` > `edit` > `comment` > `view` > none. Sources: owner; a share row for the user, their lower-cased email, or one of their Clerk groups; `visibility = org` and same `org_id` ⇒ `view`; `visibility = link` ⇒ `view` (signed in or not, public route). No access ⇒ **404** (don't reveal existence).

| Action | Needs |
|---|---|
| Read artifact / versions / comments | view |
| Add comment | comment |
| Append version, rename, change icon | edit |
| Sharing, `shared_version`, capabilities, delete | owner |

Viewers who aren't editors see `shared_version` (or latest when null); editors and the owner see every version.

### Endpoints

| Method + path | Body / query | Returns |
|---|---|---|
| `GET /api/v2/artifacts` | `scope=mine\|shared\|all` (default all), `kind`, `q` (title search), `origin` (surface), `cursor`, `limit≤100` | `{items:[ArtifactSummary], next_cursor}` newest `updated_at` first |
| `POST /api/v2/artifacts` | `{id?, kind, title, icon?, template_id?, origin?, capabilities?, body, body_format, meta?, runtime_version?}` | `Artifact` (201). Supplying an existing `id` you own returns it unchanged (idempotent). `runtime_version:1` only with `origin.legacy_source` set. |
| `GET /api/v2/artifacts/:id` | — | `Artifact` incl. `version_body` of the version this caller sees |
| `PATCH /api/v2/artifacts/:id` | `{title?, icon?, shared_version? (number\|null), capabilities?}` | `Artifact` |
| `DELETE /api/v2/artifacts/:id` | — | 204. Owner only. **Permanent**, no trash. |
| `GET /api/v2/artifacts/:id/versions` | — | `{items:[{version, author_id, created_at, size_bytes, meta}]}` |
| `GET /api/v2/artifacts/:id/versions/:v` | — | `{version, body, body_format, meta, ...}` |
| `POST /api/v2/artifacts/:id/versions` | `{base_version, body, body_format?, meta?, author?:'user'\|'assistant'}` | `Artifact` (201). **409 `stale_version`** when `base_version ≠ current_version` (body has `current_version`) so the client merges onto the newer one. |
| `GET /api/v2/artifacts/:id/sharing` | — | `Sharing` |
| `PUT /api/v2/artifacts/:id/sharing` | `{visibility, shares:[{principal_type, principal_id, level}]}` | `Sharing`. Rules below. |
| `GET /api/v2/public/artifacts/:id` | — (no auth) | `{id, kind, title, icon, body, body_format, meta, owner_name}` when `visibility='link'` and policy allows; else 404 |
| `GET /api/v2/org/artifact-settings` | — | settings for the caller's org (defaults when no row) |
| `PUT /api/v2/org/artifact-settings` | settings | Clerk org role `admin` (or `org:admin`) only |

Phase 4 adds, under the same prefix: `/:id/storage` (GET `?scope&prefix`, PUT/DELETE `/:key`), `/:id/consents`, `/:id/ai`, `/:id/connectors/:connector/:tool`, `/:id/comments`.

### Types

```ts
type Access = 'owner' | 'edit' | 'comment' | 'view';
interface ArtifactSummary { id; kind; title; icon?; owner:{id,name?,image_url?}; updated_at; current_version; visibility; my_access:Access; origin:{surface?,legacy_source?}; runtime_version; thumbnail_url? }
interface Artifact extends ArtifactSummary { template_id?; capabilities; shared_version:number|null; created_at; version:{version, body, body_format, meta, author_id, created_at} }
interface Sharing { visibility:'private'|'people'|'org'|'link'; shares:{principal_type, principal_id, level, display?, expires_at?, accepted_at?}[]; link_url?:string; policy:{link_allowed:boolean, outside_invites_allowed:boolean, reason?:string} }
```

### Sharing rules (server-enforced)

1. Default `private`. Nothing is shared until the owner shares it.
2. `org` needs the owner to be in an org; viewers in the same org get `view` (owners grant more with share rows).
3. `link` is refused (422 `link_not_allowed`) when `capabilities.ai` or `capabilities.connectors` is set, or when the org's `external_sharing` is off and the artifact isn't in `allowed_external`. Personal accounts (no org) may use links.
4. `email` principals outside the org: only when the org allows outside invites (personal accounts: allowed); **at most 50 per artifact**; `expires_at = now()+30 days` until accepted; level ≤ `comment` is not required, but outside invitees never get AI or connectors (Phase 4 enforces).
5. Kind `doc`: levels `view` and `edit` only (no `comment`); no `email` principals; no `link` unless the org allows external sharing.
6. Owner-only routes return 403 for editors.

## 4. App contract (allternit-ai)

- `src/lib/artifacts/`: `kinds.ts` (registry: kind → label, icon, viewer, editor, exports, plans), `api.ts` (cloud client using `getCloudApiBaseUrl()` + Clerk bearer, Desktop via its cloud proxy), `store.ts` (`useArtifacts`: list cache, open artifact, optimistic saves), `legacy-import.ts` (one-time import of local stores as `runtime_version:1`, idempotent by `id = 'art_legacy_' + source + '_' + localId`).
- `ArtifactWindow`: the one artifact surface (beside the chat on Desktop/web, full-screen sheet on phone/PWA). Header: icon, title, version picker (editors), presence slot, **Share**, **Export**. Body: the kind's viewer/editor.
- Transcript: artifacts appear as compact cards (title, kind, updated) that open the window; inline OpenUI cards stay inline with **Open as artifact**.
- **Artifacts tab** (replaces `views/library`): every artifact from `GET /api/v2/artifacts`, "Filter by" (kind, mine/shared), origin labels (Cowork, Code, Bot, Legacy), start-from-template gallery.
- **Output picker** in the composer: Artifact (auto), Docs, Slides, Sheets, Design, Dashboards, Motion (filtered by plan + org settings); slash commands `/docs /slides /sheets /design /dashboard /motion` preselect it. The choice is sent with the turn as `output: {kind}`.

## 5. Model contract (gizzi-code + app manual)

- Create an artifact when the content is substantial (≈15+ lines), self-contained, likely to be edited or reused, or the user asked ("make this an artifact", or an Output kind was picked). Otherwise answer inline; interactive cards stay inline.
- Tools (gizzi-code built-ins, model-agnostic):
  - `artifact_create {kind, title, body, body_format?, icon?, meta?}` → `{id, version:1}`
  - `artifact_update {id, body, base_version, note?}` → `{id, version}`
  - `artifact_read {id, version?}` → body
- gizzi calls the cloud store with the session's cloud credentials when it has them; otherwise it returns the payload as a tool result and the app persists it (same id, idempotent create). The app renders every `artifact_*` tool part as an artifact card.
- Back-compat: fences (` ```html `, ` ```mermaid `, ` ```svg `, `<artifact>` tags, `<document>`) still parse and are promoted to artifacts when they meet the size rule.

## 6. Migration of the old stores

- New writes go to v2 only once the app ships v2. Old local routes stay for reading.
- `legacy-import.ts` runs once per user per device after sign-in: content-artifacts (C), sectioned artifacts (D), canvases (E) and Cowork's browser store (F) → `POST /api/v2/artifacts` with `runtime_version:1`, `origin.legacy_source`, deterministic ids. Shares on D (`org` view/edit) map to `visibility:'org'` + edit share rows for the org members only if the owner is in a Clerk org; otherwise private.
- After all four phases ship, the old stores become read-only and are removed one release later.
