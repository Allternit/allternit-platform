# Memory Drive API contract (all four phases)

Owner: the Claude session that resumed Codex's work on 2026-10-06. This file is the
single contract the API (Rust), gizzi-code (TS) and allternit-ai (UI) build against.
All routes are under the normal authenticated `/api/v1` prefix and use the caller's
identity from the auth middleware. Errors are JSON `{ "error": "<plain sentence>" }`
with status 400 (bad input), 401 (signed out), 403 (no access to that drive), 404
(no drive / file), 409 (stale revision, body adds `expected_revision`,
`actual_revision`), 503 (`committed: true, index_dirty: true`: the git commit
landed but indexing is pending; a later read repairs it).

## Format (Agent Memory Repo SPEC.md, MIT)

- `MEMORY.md`: `# Memory` (or `# Memory: Name`), short pinned lines, then `## Index`
  with one `- [[topic/path]]` per topic file (no `.md`). Max 200 lines.
- Topic files `*.md`: one-line bullets
  `- <text> [source: <SOURCE>; added: YYYY-MM-DD; id: <stable-id>; key: value…]`.
  `id` is an Allternit extension and optional for external writers.
- `SOURCE` is one of:
  - `/?session=<id>` web/Desktop/phone chat session (product-relative). Bot thread:
    `/?session=<id>&bot=<botId>`.
  - `gizzi:session/<id>` a local gizzi-code session (not resolvable on the server).
  - `https://…` any external link; `<scheme>:<opaque>` labels from external agents
    (e.g. `claude-code:session/abc`), except `javascript:`, `data:`, `file:`,
    `vbscript:`, `blob:`.
  - `imported:unknown` for imported rows with no known session.
- Optional metadata keys used by Allternit: `agent`, `observation`, `session`,
  `memory_type` (fact|preference|event|procedure|entity|relationship|task_state),
  `confidence` (0..1), `status` (proposed|rejected — never indexed as active),
  `scope` (twin* never indexed as user facts), `origin`, `origin_hash`.
- Never in a drive: transcripts, credentials (secret scanner rejects), raw HTML,
  images, scripts, symlinks, executables. 64 KiB/file, 2 MiB/drive, 256 files.

## Drives (Phase 3 adds non-personal kinds)

Every drive route takes optional `drive=<ref>`; default `personal`.
`<ref>` = `personal` | `project:<projectId>` | `team:<workspaceId>` |
`bot:<botId>` | `swarm:<swarmId>`. Access is checked server-side on every call.

- `GET /memory/drive/mounts` → `{ mounts: [{ ref, kind, name, access: "read"|"write", revision }] }`

## Phase 1 routes

- `GET /memory/drive/info?drive=` → `{ ref, name, brain_id, revision, indexed_revision,
  index_dirty, imported_at, clone_url, branch }`. `clone_url` is credential-free:
  `<public base>/api/v1/brains/<brain_id>/git`. First call provisions the personal drive.
- `GET /memory/drive/tree?drive=&revision=` → `{ revision, files: [{ path, bytes }] }`
- `GET /memory/drive/file?drive=&path=&revision=` → `{ revision, path, content }`
- `GET /memory/drive/history?drive=&path=&limit=` (limit ≤100, default 25) →
  `{ commits: [{ revision, parents[], author, timestamp, message }] }`
- `GET /memory/drive/diff?drive=&from=&to=&path=` → `{ from, to, path, diff }`
  (`from`/`to` are full 40-hex revisions; for a single commit use its first parent
  as `from`; root commit: `from` = `4b825dc642cb6eb9a060e54bf8d69288fbee4904`, the empty tree)
- `POST /memory/drive/write` body `{ drive?, expected_revision, message?, operations: [Operation] }`
  → `{ revision, changed }`. `Operation` (serde externally tagged):
  `{"SetFile":{"path","content"}}`, `{"DeleteFile":{"path"}}`,
  `{"UpsertEntry":{"path","entry":{"id","text","source","added","metadata":{}}}}`,
  `{"DeleteEntry":{"id"}}`.
- `POST /memory/drive/remember` body `{ drive?, text, source?, memory_type?, path? }` →
  `{ revision, entry }`. Server fills id/date; default path `notes.md`, default source
  `imported:unknown` is NOT allowed here — when no session is given the server uses
  `/?view=settings&section=memory` (a manual save from Settings).
- `POST /memory/drive/reindex`, `GET /memory/drive/health` → `{ revision, indexed_revision, index_dirty, imported_at }`
- `POST /memory/drive/import` body `{ apply?: false, expected_revision? }` → dry run
  `{ dry_run: true, plan: { total, converted, skipped, topic_files[], already_imported, rows[] } }`;
  apply `{ dry_run: false, revision, changed }`. Apply is never automatic.
- `GET /memory/drive/tokens?drive=` → `{ tokens: [{ id, label, access, created_at, last_used_at }] }`
- `POST /memory/drive/tokens` body `{ drive?, label?, access: "read"|"write" }` →
  `{ id, token, username: "x-access-token", clone_url, access }`. `token` is shown once.
  Read tokens can only fetch that one repo; write tokens can push that one repo
  (push is validated server-side before refs update).
- `DELETE /memory/drive/tokens/:id` → `{ revoked: true }`

Existing routes keep their shapes but become drive-backed: `/memory/v2/retain`,
`/memory/v2/recall`, `/memory/v2/facts*` (list/patch/delete), Settings "remember this".

## Phase 2 — Dreaming

- `GET /memory/drive/dreams?drive=&limit=` → `{ dreams: [{ id, date, status:
  "applied"|"no_changes"|"failed"|"undone"|"running", revision, base_revision,
  report, summary: { merged, resolved, lessons, pruned, proposals }, error, undo_revision, created_at }] }`
- `POST /memory/drive/dreams/run` body `{ drive? }` → runs today's Dream now (owner only);
  same shape as one dream. Idempotent per date.
- `POST /memory/drive/dreams/:id/undo` → `{ revision, undone: true }` or 409
  `{ error, conflicts: [paths] }` when later edits touch the same lines.
- Commit message `Dream YYYY-MM-DD`, author `Dream via Allternit`.
- `GET /memory/drive/settings` / `PUT` `{ dreaming_enabled: bool }` (default on).

## Phase 3 — questions board

`questions.md` in a team/swarm/project drive; one bullet per question, answers as
entries with `parent: <questionId>`. API:
- `GET /memory/drive/questions?drive=` → `{ revision, questions: [{ id, text, author,
  added, status: "open"|"answered"|"resolved", source, answers: [{ id, text, author, added, source }] }] }`
- `POST /memory/drive/questions` `{ drive, text, source? }` → `{ revision, question }`
- `POST /memory/drive/questions/:id/answer` `{ drive, text, source? }`
- `POST /memory/drive/questions/:id/resolve` `{ drive }`

## Git transport

- Clone: `git clone https://x-access-token:<token>@…` is NOT how we show it. UI shows
  the credential-free URL and the token separately, plus a credential-helper snippet:
  `git -c credential.helper='!f() { echo username=x-access-token; echo password=$ALLTERNIT_MEMORY_TOKEN; }; f' clone <clone_url> memory`.
- Smart-HTTP basic auth username is ignored; password (or Bearer) is the token.
- A push to a memory drive is validated before refs update (format, paths, modes,
  secrets, no deletes/non-fast-forward); rejected with a plain reason. After a
  successful push the API reindexes.

## Session source links (UI)

`/?session=<id>` (optionally `&bot=<botId>`) opens that session on Desktop, ai.allternit.com
phone layout and the PWA. If the session is missing or not the viewer's, show
"That session is no longer available." Never fabricate other URL shapes.

## Contract updates (implemented API, supersede the above where different)

- `GET /memory/drive/info` also returns `kind`, `access` ("read"|"write"), `pending_writes`
  (writes waiting to retry). `name` is `memory` for personal drives.
- `GET /memory/drive/mounts` → `revision` is `null` for a drive not created yet (opening it creates it).
- `POST /memory/drive/remember` personal → `{ revision, fact_id, path, entry_id }`; shared →
  `{ revision, path, entry_id }`. Body also accepts `session` (a session id; becomes `/?session=<id>`).
  When nothing is saved (empty, duplicate, looks like a credential) → 400 with a plain message.
- `POST /memory/drive/dreams/:id/undo` → `{ undone: true, revision, dream }`; 409 body
  `{ error, conflicts: [paths] }`; 400 when already undone / not applied.
- `POST /memory/drive/dreams/run` → one dream row; 409 if a run is in progress.
- `DELETE /memory/drive/tokens/:id` only revokes Memory Drive tokens.
- Dream rows: `summary` object always present (zeros), `report` markdown may be null for failed runs, `error` plain sentence.
- 403 = no access / read-only member writing.
- Managed folders `twin/` (twin projection, read-only) and `cowork/` (bot/project memory mirror, read-only):
  UI should show them read-only with a note; writes to them return 400.

- Bot and project drives: `memory.md` is the bot's/project's memory (canonical; cowork rows are its index).
  `cowork/` is no longer used or guarded. Entry metadata: `owner`, `grants` (comma list), `cowork_type`, `tags`,
  `cowork_source`, `project`, `session`.
