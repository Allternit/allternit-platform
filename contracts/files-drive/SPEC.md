# Files drive protocol v2 (`files/2`)

Status: contract for step 4 of the Files drive plan (`PLAN-files-drive-2026-11-15.md`). Everything ships dark behind the per-user flag `files_drive`; a caller without it gets `feature_disabled`.

This file says how the protocol behaves. The message shapes live in `schema/*.json`; examples live in `fixtures/`. The generated types are built from the schemas:

| Output | Path | Consumers |
|---|---|---|
| Rust | `platform/types/allternit-files-core` (crate `allternit-files-core`) | allternit-api runtime, cloud-api, Windows helper, Linux FUSE, replicas |
| TypeScript | `contracts/files-drive/gen/files-drive.gen.ts` | allternit-ai (web, phone layout, PWA, Desktop), Gizzi Code |
| Swift | `contracts/files-drive/gen/FilesDriveContract.gen.swift` | AllternitFilesKit (iOS app + File Provider extension, macOS File Provider) |
| Kotlin | `contracts/files-drive/gen/FilesDriveContract.gen.kt` | Android DocumentsProvider (stdlib only; `@Wire` names the JSON field) |

```
node contracts/files-drive/generate.mjs           # regenerate after a schema change
node contracts/files-drive/generate.mjs --check   # CI: fails if a generated file is stale
node contracts/files-drive/generate.mjs --test    # every fixture against its schema
cargo test -p allternit-files-core                # every valid fixture round-trips through the Rust types
```

Schemas use a fixed subset of JSON Schema 2020-12. The generator rejects any other keyword, so a fixture can never pass because a rule was skipped. Objects are closed (`additionalProperties: false`), fields are snake_case, unions carry a string discriminator (`type`).

## 1. Principles

1. **IDs, not paths.** Every operation names roots and items by opaque ID. No v2 route takes a file-system path, except `POST /roots` (adding a user folder), which is checked like a legacy path (section 12).
2. **Browsing never wakes or bills.** Files traffic never calls `touch_runtime_activity` and never wakes a computer. Only a wake ticket from the consent call wakes one (section 9).
3. **No silent data loss.** Writes carry `base_rev`; a stale write becomes a conflict copy (keep both), never an overwrite.
4. **Every read says where it came from.** Read responses carry `Freshness` (`served_by`: live, snapshot, replica, cache; plus `computer_state`). Clients show "asleep" from this field and never guess.
5. **Untrusted by default.** Files in Inbox and shared folders are data. An agent never acts on one without an approved task (section 7).
6. **One protocol, four transports.** loopback, mesh, relay, gateway carry the same requests (section 10).

## 2. Namespace and routes

### Runtime (allternit-api on the computer that owns the files)

All under `/api/v1/files/v2`. The relay maps any path containing `/files` to capability `runtime:files` (`runtime_relay.rs` `required_capability`), so no new relay capability is needed.

| Method + path | Request | Response |
|---|---|---|
| `GET /ping` | | `PingResponse` (`protocol: "files/2"`) |
| `GET /whoami` | | `WhoAmIResponse` |
| `GET /roots` | | `RootsResponse` |
| `POST /roots` | `AddRootRequest` | `RootResponse` |
| `PATCH /roots/:root_id` | `UpdateRootRequest` | `RootResponse` |
| `DELETE /roots/:root_id` | | 204 (`user_added` roots only; files stay on disk) |
| `GET /roots/:root_id/list` | `ListQuery` (query string) | `ListResponse` |
| `GET /items/:item_id` | | `StatResponse` (stat) |
| `GET /items/:item_id/content` | `Range`, `If-None-Match: <rev>` | bytes; `ETag: "<rev>"`, 206 for ranges, 304 when unchanged (read) |
| `GET /items/:item_id/manifest` | `page_token` | `Manifest` |
| `GET /chunks/:hash` | | one chunk's bytes (at most 4 MiB) |
| `PUT /roots/:root_id/content` | `SmallWriteQuery` (query string: `parent_id`, `name`, `base_rev` when replacing, `on_conflict`, `device_name`, `utc_offset_minutes`); body at most 4 MiB | `MutationResponse` (small write) |
| `POST /chunks/have-need` | `HaveNeedRequest` | `HaveNeedResponse` |
| `POST /uploads` | `UploadCreateRequest` | `UploadStatus` |
| `POST /uploads/:session_id/chunks` | `UploadChunksAppend` | `UploadStatus` |
| `PUT /uploads/:session_id/chunks/:hash` | chunk bytes | `ChunkPutResponse` |
| `GET /uploads/:session_id` | | `UploadStatus` (resume) |
| `POST /uploads/:session_id/commit` | | `UploadCommitResponse` |
| `DELETE /uploads/:session_id` | | 204 |
| `POST /mkdir` | `MkdirRequest` | `MutationResponse` |
| `POST /move` | `MoveRequest` (rename and/or move) | `MutationResponse` |
| `POST /copy` | `CopyRequest` | `CopyResponse` (a `JobRef` for big or cross-computer copies) |
| `POST /delete` | `DeleteRequest` | `DeleteResponse` (to trash unless `permanent`) |
| `GET /roots/:root_id/trash` | `page_token` | `TrashListResponse` |
| `POST /restore` | `RestoreRequest` | `MutationResponse` |
| `GET /items/:item_id/versions` | | `VersionsResponse` |
| `POST /items/:item_id/versions/restore` | `RestoreVersionRequest` | `MutationResponse` |
| `POST /search` | `SearchRequest` | `SearchResponse` |
| `POST /pin` | `PinRequest` | `PinResponse` |
| `GET /roots/:root_id/changes` | `ChangesQuery` | `ChangesResponse` |
| `GET /subscribe` (WebSocket) | first frame `SubscribeMessage` | `FeedMessage` frames |
| `GET /jobs/:job_id` | | `JobRef` |
| `GET /storage` | | `StorageDiscovery` |
| `GET /items/:item_id/comments`, `POST /comments` | `CommentCreateRequest` | `Comment[]`, `Comment` |
| `GET /replicas`, `POST /replicas` | `CreateReplicaRequest` | `ReplicasResponse` (on the replica host) |
| `POST /replicas/restore` | `ReplicaRestoreRequest` | `ReplicaRestoreReport` |

Writing a file: a client with a file of 4 MiB or less may use the small write; anything bigger, or anything that must resume, uses an upload session (section 5).

Body limits on the runtime: axum's default body limit is 2 MB, so the chunk and small-write routes set `DefaultBodyLimit::max(4 MiB + 64 KiB)`. All other v2 routes keep a 1 MiB limit.

### cloud-api

| Method + path | Request | Response |
|---|---|---|
| `GET /api/v1/computer-files/:computer_id/endpoints` | | `EndpointsResponse` |
| `POST /api/v1/computer-files/:computer_id/wake-consent` | `WakeConsentRequest` | `WakeConsentResponse` |
| `GET` / `PUT /api/v1/computer-files/:computer_id/wake-settings` | `UpdateWakeSettingsRequest` | `WakeSettings` |
| `GET /api/v1/computer-files/:computer_id/storage` | | `StorageDiscovery` (last heartbeat report) |
| `POST /api/v1/computer-files/:computer_id/shares` | `CreateShareRequest` | `ShareResponse` |
| `GET /api/v1/computer-files/:computer_id/shares` | | `SharesResponse` |
| `DELETE /api/v1/computer-files/shares/:share_id` | | 204 (revoke; applies on the guest's next request) |
| `DELETE /api/v1/computer-files/shares/:share_id/guests/:guest_id` | | 204 (revoke one guest; applies on that guest's next request) |
| `GET /api/v1/computer-files/shares/:share_id/access` | | `ShareAccess[]` (audit) |
| `GET /api/v1/computer-files/token-keys` | | `{keys: [{kid, alg: "Ed25519", public_key (base64url, 32 bytes)}]}`, so runtimes verify tokens offline |
| `GET /api/v1/files-keys` | | `FilesKeyInfo` |
| `POST /api/v1/files-keys/requests` | `KeyShareRequest` | 202 |
| `POST /api/v1/files-keys/grants` | `KeyShareGrant` | 204 |
| `GET /api/v1/files-keys/envelopes/:device_id` | | `KeyEnvelope` |
| `GET /api/v1/files-gateway/summary?link=<token>` | | `ShareSummary` (public, no auth) |
| `POST /api/v1/files-gateway/redeem` | `GuestRedeemRequest` | `GuestSessionResponse` |
| `/api/v1/files-gateway/v2/*` | same shapes as the runtime routes, guest subset | same |

Relay: the client posts the existing cloud-api `RelayRequest` (`runtime_relay.rs`: `{method, path, headers: map, body: string, body_encoding}`; this contract does not redefine it) to `POST /api/v1/runtime-devices/:computer_id/proxy` with `path` set to `/api/v1/files/v2/...`. Live change-feed sockets use the existing socket-ticket flow (`/runtime-devices/:id/socket-ticket`, then `/socket`).

- **Field name.** cloud-api's browser proxy body (`BrowserProxyRequest`) is camelCase and reads `bodyEncoding`. It accepts `body_encoding` too, so clients may send either; Files clients send `body_encoding`. `utf8` (default) or `base64`.
- **Raw response.** The proxy streams the runtime's response as-is: real status, the allowed response headers, raw body bytes (binary content stays binary). It does not wrap it in a `{status, headers, body}` envelope. Only the runtime-side WebSocket protocol uses envelopes.
- **Relayed request headers.** Besides today's allow-list (`authorization`, `accept`, `content-type`, `if-none-match`, `if-modified-since`, `last-event-id`, `x-request-id`, `x-allternit-device-id`, `x-allternit-device-name`, `x-allternit-device-platform`, `x-allternit-human-proof`, `x-allternit-human-action`), the relay must pass `x-allternit-wake-ticket`, `range`, `if-match` and `x-allternit-unlock`. Without them wake tickets, ranged reads, conditional writes and protected-root unlocks fail over the relay.
- **Relayed response headers.** Besides today's list (`content-type`, `cache-control`, `content-disposition`, `etag`, `last-modified`, `x-request-id`), the relay must pass `content-range`, `accept-ranges` and `retry-after`, so 206 ranged reads and `waking` retries work over the relay.

### Computer ids

`computer_id` in every cloud-api route and in the relay path is cloud-api's **runtime device id** (`runtime_devices.id`), the id the relay is keyed by. Clients map their own computer records to it like this:

| Client record | Where the id comes from |
|---|---|
| allternit-ai `Computer` (`/api/v1/computers`), `kind: local`, `provider: host` (this device) | `native_id` is this device's runtime device id. Desktop talks to it over loopback. |
| allternit-ai `Computer`, `provider: fabric` (paired computers and cloud computers) | `native_id` is the paired-computer id (`pc_…`). cloud-api resolves it. A cloud computer resolves through `paired_computers.provisioned_instance_id` → `provisioned_instances.device_id` to its runtime device. A machine paired by pairing code has no runtime device linked in cloud-api (`paired_computers` stores only its mesh address), so it stays mesh-only: `computer_id` is the `pc_…` id, `host: own_device`, and the endpoints list one `mesh` candidate at its `mesh_ip` and no relay. |
| allternit-ai `Computer`, `kind: cloud_desktop` and bot boxes (`native_id` is a sandbox id) | Not a Files computer. The Files tab is hidden. |
| allternit-ai `Computer` with no `native_id` | Not a Files computer. Clients never fall back to `Computer.id` (`cmp_…`), which cloud-api does not know. |
| iOS / Android / Gizzi | They already list runtime devices from cloud-api; they use that id as is. |

So clients call `GET /computer-files/:id/endpoints` with the id from this table, and from then on use `EndpointsResponse.computer_id` (the resolved runtime device id, or the `pc_…` id of a mesh-only machine) and the candidates' `url`. They never build a relay URL from a `pc_…` id. An id cloud-api cannot resolve to a runtime device the caller owns returns `not_found`.

## 3. Roots

A root is a named folder on one computer with a policy (`RootPolicy`: read, write, shareable, agent_process, protected, offline, replica, versions, trash_days). Only files inside a root exist to v2.

Default roots, created on first use of the drive:

| Kind | Name | Location | Notes |
|---|---|---|---|
| `allternit_home` | Allternit | `~/Allternit/` | parent of the six folders below |
| `inbox` | Inbox | `~/Allternit/Inbox` | drops land here; `agent_process` on, tasks need approval |
| `shared` | Shared | `~/Allternit/Shared` | the default folder to share |
| `agent_output` | Agent Output | `~/Allternit/Agent Output` | results written by agents, with provenance |
| `downloads` | Downloads | `~/Allternit/Downloads` | agent-browser downloads and computer transfers |
| `meetings` | Meetings | `~/Allternit/Meetings` | recordings and transcripts (no runtime path exists today; this is their new home) |
| `scans` | Scans | `~/Allternit/Scans` | document scans from phones |
| `memory_drive` | Memory Drive | existing Memory Drive store | writes go through the Memory Drive write path so its git history stays; protected |
| `code` | Code | the user's code folder(s) | |
| `models` | Models | local-engine model cache | `transfer` and `execute` are separate permissions |
| `user_added` | user's choice | a folder the user picks | checked as in section 12 |

`provider_ready: false` means the folder exists but nothing fills it yet. `path_hint` is shown to the owner only and never sent to guests.

## 4. Items, revisions and conflicts

- **Identity.** `ItemId` (`it_…`) comes from the index DB on the owning computer. It survives rename and move on that computer, including moves between roots. It is opaque: never parse it, never derive it from a path.
- **Revision.** `Rev` = `"<meta_version>-<blake3 content hash>"`, or `"<meta_version>-dir"` for folders. It is the HTTP `ETag`. `meta_version` rises on every content or metadata change.
- **Writes carry `base_rev`.** Small write (when replacing), upload commit (when replacing), move, delete and version restore all carry the rev the client last saw. A stale `base_rev`:
  - `on_conflict: keep_both` (default for sync clients): the write lands as a conflict copy beside the original. The response carries `ConflictInfo` and a `files.conflict` event fires. Name: `<stem> (conflict <device name> <YYYY-MM-DD HHmm>).<ext>`, e.g. `Lease (conflict iPhone 2026-11-15 1042).pdf`; if that name is taken too, ` 2`, ` 3`… is added before `)`.
  - **The date and `HHmm` are the editing device's local time**, not the runtime's. The writer sends `utc_offset_minutes` (small write query, `UploadCreateRequest`); the runtime applies it to the time it accepts the write. Without it the runtime uses UTC. `device_name` comes from the request field, else `X-Allternit-Device-Name`, else the principal's display name.
  - A create (no `base_rev`) whose name already exists in the parent is not a conflict: with `keep_both` it lands as `<stem> (2).<ext>` (then `(3)`…) with no `ConflictInfo`; with `fail` it returns `already_exists`.
  - `move`, `delete`, `trash` and version `restore` with a stale `base_rev` always return `rev_mismatch` (409) whatever `on_conflict` says; there is no keep-both form of them.
  - `on_conflict: fail`: `rev_mismatch` with the `current` item.
  - `on_conflict: replace`: refused while `base_rev` is stale (`rev_mismatch`). `replace` only settles name clashes.
  - **`base_rev` names a file that is now gone** (deleted, trashed or purged since the client saw it): `keep_both` creates the file again under its name in the parent (as a create, so a taken name gets `(2)`), with no `ConflictInfo`. `fail` and `replace` return `not_found`.
- **Versions and trash.** Each root keeps versions per `VersionPolicy` (count and age) and trash for `trash_days`. Delete moves to trash unless `permanent`. Restoring into a taken name follows `on_conflict`.
  - `keep_count` is how many replaced versions are kept; 0 keeps none (the Models root uses `keep_count: 0`, since model files are big and re-downloadable).
  - `keep_days` counts from when a version was **replaced**, not from when it was written. 0 means no age limit (only `keep_count` applies).
  - `trash_days: 0` means no trash: a delete removes the item at once, as if `permanent`.
- **Changes made outside the engine.** The watcher keeps IDs for files that other apps rename or move (matched by inode, then by content hash). A file deleted outside the engine goes to trash if its content is still in the chunk store (a version or an earlier upload kept it), so it can be restored; otherwise it is dropped from the index and a `files.changed` delete fires.

## 5. Chunks, manifests and upload sessions

- **Chunking.** FastCDC (2020) with blake3-256: min 256 KiB, avg 1 MiB, **max 4 MiB (4194304 bytes)**. The schema rejects a bigger chunk.
- **Pinned chunker (v1).** Dedup only works if every client cuts the same boundaries, so the parameters are fixed: the `fastcdc` crate 3.2.1 `v2020` module, normalization level 1, its default gear table (seed 0), min 262144 / avg 1048576 / max 4194304, `mask_s` `0x0000d91767537000`, `mask_l` `0x0000d91707537000`, and `mask_s_ls` / `mask_l_ls` the same masks shifted left one bit (`0x0001b22ecea6e000`, `0x0001b22e0ea6e000`). Chunk and file hashes are blake3-256, lower-case hex. `contracts/files-drive/vectors/chunker-v1.json` holds the step-by-step cut algorithm, the gear table and test cases with the expected cuts and hashes. The Rust engine, the web chunker and the Swift chunker must all pass it; a chunker that doesn't must not upload (the runtime would still accept its chunks, but dedup and replica checks would miss).
- **Why 4 MiB.** The relay caps a request body at 5 MiB (`MAX_RELAY_BODY_BYTES`, `runtime_relay.rs:97`). It measures a base64 body as `len * 3 / 4`, which is the decoded size, so a 4 MiB chunk is measured as about 4 MiB and leaves about 1 MiB for the envelope and headers. The base64 JSON envelope the browser sends to cloud-api is about 5.4 MiB, under cloud-api's 10 MiB global body limit (`lib.rs`, `MAX_BODY_SIZE_BYTES`). Relay `max_request_bytes` is advertised as 5242880.
- **Manifest.** Each file revision has a `Manifest`: ordered `ChunkRef`s (hash, offset, size) plus the whole-file `content_hash`. Big manifests page with `next_page_token`.
- **Dedup.** Before uploading, the client sends hashes to `have-need`; it uploads only the `need` list. The same mechanism checks replicas.
- **Upload session.** `POST /uploads` names the target (new file under `parent_id`, or replace `item_id` at `base_rev`), the size, the hash and the first chunks. The client `PUT`s each needed chunk, appends more chunk refs for very large files, and commits. The runtime verifies every chunk hash and the whole-file hash at commit (`chunk_hash_mismatch` for a chunk whose bytes don't match its hash, at PUT or commit; `chunk_missing` for a needed chunk never sent; `manifest_invalid` for a bad manifest or a whole-file hash that doesn't match the assembled chunks). A session survives a dropped connection until `expires_at`; `GET /uploads/:id` returns what is still needed. Chunks are stored once per computer and shared between revisions.

## 6. Change feed

- Each root has a monotonic change cursor (`c1.<opaque>`). Clients compare cursors only by asking the server.
- **Poll:** `GET /roots/:root_id/changes?cursor=&limit=&wait_s=` returns the changes after `cursor` and a new cursor. `wait_s` (at most 25) holds the request open until something changes (long poll).
- **Subscribe:** a WebSocket. The client sends `SubscribeMessage` with a cursor per root; the server sends `FeedMessage` frames: `changes`, `reset`, `state` (computer went asleep or woke), `heartbeat`.
- **Reset.** If the cursor is too old, the index was rebuilt or the root was removed, the server answers 200 with `reset_required: true` (poll) or a `reset` frame (subscribe). The client then lists from scratch. `410 cursor_expired` is only for a cursor the server cannot parse or that names another root; clients treat it like a reset.
- **No keep-awake.** A held socket or long poll never counts as activity and never keeps a computer awake. When the computer goes to sleep the server sends `state: asleep` and closes; clients then read from the snapshot.

## 7. Principals and permissions

| Principal | Who | How it authenticates | What it reaches |
|---|---|---|---|
| `owner` | the signed-in user | the user's session or API token (as today) | all roots on their computers |
| `own_device` | one of the owner's devices (phone, Mac, PC, browser profile) | the owner token plus `X-Allternit-Device-Id`; over mesh, the mesh identity | same as owner; pins, offline cache and replicas are per device |
| `guest` | someone holding a share link | `Authorization: Files-Guest <capability token>` via the gateway | one folder or file, one role |
| `agent` | an agent session on the runtime | the runtime's own session, carrying an `AgentGrant` | only the roots/items in its grant, only the tools in its grant |

`Permissions` (list, read, write, delete, share, comment, agent_process, transfer, execute) are computed by the runtime per request and returned on each item. Clients use them to show or hide actions; the server enforces them.

- `agent_process` is separate from share roles: sharing a folder never lets an agent act on it.
- `transfer` (copy a model or dataset) is separate from `execute` (run it). A package with transfer only cannot run (`execute_denied`).
- Inbox and shared-folder files are untrusted. A drop creates a task in `needs_approval` (`files.inbox.dropped`), unless the owner pre-approved a folder rule with a fixed tool set (`approved_by_rule`). No tool call happens before approval, and none reaches outside the root after.

## 8. Capability tokens

Wake tickets (`WakeTicketClaims`, `typ: "wake"`) and guest tokens (`GuestCapabilityClaims`, `typ: "files_guest"`) share one format:

```
<prefix>.<base64url(claims JSON)>.<base64url(Ed25519 signature over "<prefix>." + payload part)>
prefix: awt1 (wake ticket) or fgt1 (guest token); the schemas pin these.
```

- Signed by cloud-api with the key named by `kid`. Runtimes fetch public keys from `GET /api/v1/computer-files/token-keys` and verify offline.
- `iat` and `exp` are Unix seconds. Wake tickets live 120 s. A ticket wakes its computer at most once (cloud-api records the `jti`; a repeat consent with the same `jti` does not start a second wake), but it may be sent on any number of requests to that one computer until `exp`, so a multi-request transfer reuses it. Guest tokens live at most until the share expires and at most 1 hour per session; the gateway re-mints them.
- Two checks for guests: the gateway checks the token and the share's revocation state, then the runtime re-checks scope (`computer_id`, `root_id`, `item_id`, `role`) on every request. A revoked share or guest fails on the next request.
- Roles: `view_download`, `upload_only` (cannot list: `forbidden`), `edit`, `comment`. Every guest access writes a `ShareAccess` audit row and a `files.share.accessed` event.
- A passcode, when set, is checked at redeem (`passcode_required`, `passcode_invalid`); the token records `passcode_ok`.

## 9. Wake consent

Today `connect_or_wake_runtime` (`runtime_relay.rs`) wakes a stopped computer on any relayed request. For v2 Files paths (`/api/v1/files/v2/*`) cloud-api changes that. Legacy `/api/v1/files/*` keeps today's wake behaviour for now: its callers (web plugin discovery, iOS MCP and HEARTBEAT screens, `filesApi`) cannot handle `wake_consent_required`. When steps 26 (web) and 19 (iOS) move those callers to v2, the same change puts legacy paths under these rules too:

1. **Files requests never wake and never touch activity.** The relay does not call `wake_for_device` or `touch_runtime_activity` for them.
2. **Metadata is answered from the snapshot.** While the computer sleeps, `roots`, `list`, `stat`, `changes` and name `search` are answered from the asleep snapshot (section 11) with `served_by: snapshot`, `computer_state: asleep`.
3. **Content needs consent.** `content`, `manifest`, `chunks`, uploads, writes and content search return `wake_consent_required` (409) with a `WakeHint` (`setting`, `consent_path`), unless the request carries a valid wake ticket in `X-Allternit-Wake-Ticket`.
4. **The consent call is the only waker.** `POST /computer-files/:id/wake-consent` with a `WakeReason` checks the computer's `WakeSetting`:
   - `ask` (default): the client showed the user a prompt; the call wakes the computer and returns a ticket.
   - `auto`: the client calls without a prompt; same result.
   - `never`: `wake_denied`.
   It returns `{ticket, expires_at, computer_state: waking|awake, estimated_wake_s}`. Requests carrying the ticket wait for the runtime (up to the relay's 30 s wake wait) instead of failing.
5. **The idle timer.** The consent call is a user action, not Files traffic. It records exactly one activity touch at wake time. This is needed because the free-tier sleep sweep uses `COALESCE(last_activity_at, last_started_at, created_at)` (`provisioning_free.rs`): an old `last_activity_at` would otherwise put the computer back to sleep right after the wake. A transfer that outlasts the idle window asks for a new ticket (silently under `auto`), which records one more touch. Nothing else in Files resets idle.
6. **Guests** never wake the owner's computer unless the share has `guest_wake: true` (`guest_wake_not_allowed`). An `upload_only` drop to a sleeping computer is stored in the owner's own R2 bucket as a `QueuedDrop` and pulled into Inbox on the next wake.

Errors: `asleep` (no snapshot yet), `waking` (with `retry_after_s`), `offline` (an own device that is not reachable; it cannot be woken).

## 10. Transports and replica resolution

`GET /computer-files/:id/endpoints` returns `TransportCandidate`s sorted by `priority` (lower first) plus `host`:

| Transport | When | Notes |
|---|---|---|
| `loopback` | Desktop app talking to its own runtime | local caller; section 12 rules for local callers |
| `mesh` | between the owner's own devices on Headscale | only for `host: own_device` |
| `relay` | always available | the baseline; requests are `RelayRequest`s to cloud-api |
| `gateway` | guests | capability token, cloud-api checks first |

**Cloud computers are relay only.** Computers provisioned by cloud-api (`host: cloud_computer`) do not join the mesh (no tailscale in their cloud-init or provisioning), so cloud-api never lists a mesh candidate for them.

Clients pick the source of an answer in this order:

1. loopback, if this device owns the files;
2. mesh, if the owner device is on the mesh and answers `ping`;
3. relay, if the computer is awake;
4. a **replica** (`ReplicaState: current`) on another of the user's devices, if the primary is asleep or offline: content is served from the replica, `served_by: replica`;
5. the **snapshot**, metadata only, `served_by: snapshot`;
6. the device's own **cache** (pinned files), `served_by: cache`.

Only step 3 with a wake ticket ever wakes a computer. Replication itself never wakes one: a replica catches up the next time the primary is awake for another reason.

## 11. Asleep snapshot

The runtime writes file names, sizes and revisions (never contents) to the **user's own R2 bucket**, the same place and trust class as files the user uploads. It writes only while it is awake: on sleep notice, and at most every 5 minutes while changes are pending.

```
files-snapshots/<computer_id>/v1/manifest.json          SnapshotManifest (root list)
files-snapshots/<computer_id>/v1/roots/<root_id>.json   SnapshotRootFile (items + the root's cursor)
```

**Decided (Eoj 2026-11-15): plain names.** v2 writers always write `encryption: "none"` and `names_encrypted: false`. The format keeps `names_encrypted`, `ciphertext`, `nonce` and `key_id` (a reference to the Files key in section 13, snapshot subkey) as **reserved, not used in v2**, so encrypted names can be added later without a format break. A v2 reader that meets `names_encrypted: true` shows the folder as "names unavailable" instead of failing.

Snapshots exclude roots whose policy is `protected` (Memory Drive), and never include `path_hint` or local paths.

**How it gets to R2.** The runtime never holds R2 credentials. It sends the files through cloud-api, signed like the presence heartbeat (`runtime_events` signature headers): `PUT /api/v1/computer-files/snapshot/manifest` (`SnapshotManifest`, at most 1 MiB), then `PUT /api/v1/computer-files/snapshot/roots/:root_id` (`SnapshotRootFile`, at most 8 MiB) for each root. cloud-api stores them under the keys above in the user-files bucket, refuses a manifest for another computer, drops `protected` roots and `path_hint`, and refuses a root file for a root the stored manifest does not list. While the computer has no relay connection, the relay answers `roots`, `list`, `stat`, `changes` (empty at the snapshot's cursor, `reset_required` for any other cursor) and name `search` from these files (section 9 item 2). The live change-feed socket is not offered for a sleeping computer; clients poll `changes`.

## 12. Scoping (same rules as the legacy routes)

The legacy `/api/v1/files/*` routes are being scoped by step 3 (`files_scope.rs`). v2 applies the same rules:

- **Relayed callers** (requests with `x-allternit-relayed`, `RELAYED_HEADER` in `cowork_devices_routes.rs`) reach **roots only**. There is nothing else to reach in v2, because every route takes IDs.
- `POST /roots` (add a user folder) takes `local_path`. The runtime canonicalizes it, resolves symlinks, requires it to be inside the home folder, and refuses the secrets deny-list (`.ssh`, keychains, token stores, the Allternit data dir) with `scope_violation`. A root added over the relay starts with `shareable: false` and `agent_process: false` until the owner changes them.
- Inside a root, a symlink that resolves outside that root is not followed (listed as a file with no content; reads return `scope_violation`).
- `protected` roots need a fresh device unlock (Face ID, biometrics) on the client; the runtime answers `protected_root_locked` until the client sends the unlock proof.
- Agents reach only what their `AgentGrant` names (`scope_violation` otherwise).

## 13. Files key hierarchy

- **Files root key (FRK):** 32 random bytes per user, made on the user's first device. It never leaves a device unwrapped. cloud-api stores only wrapped copies and cannot open them.
- **Device envelopes:** `KeyEnvelope` wraps the FRK to one device's X25519 key (`x25519-hkdf-sha256-xchacha20poly1305`). A new device sends `KeyShareRequest` (with a short `confirm_code`); an existing device shows the code, the user approves, and that device posts a `KeyShareGrant`.
- **Subkeys** via HKDF-SHA256 from the FRK:
  - `files-replica/1/<replica_id>`: replica chunks (`EncryptedChunkHeader`, XChaCha20-Poly1305). A replica host stores ciphertext it cannot read.
    - **Locator** (the name a sealed chunk is stored under): `blake3::keyed_hash(subkey, raw)` where `raw` is the 32 **raw bytes** of the plaintext blake3 hash (hex-decoded, not the hex string), written as lower-case hex.
    - **AAD:** the UTF-8 string `afc1|<v>|<cipher>|<key_id>|<locator>|<plaintext_size>` (decimal integers, no spaces), so a changed header field fails to open.
    - **Blob layout on the host:** `u32 big-endian header length | header JSON | ciphertext`, file name = locator.
  - `files-cache/1/<device_id>`: the device's offline cache.
  - `files-snapshot/1`: reserved for encrypted snapshot names (section 11).
- **Rotation:** a new `key_id` with `rotated_from`; old envelopes stay until every replica is re-sealed. Removing a device rotates the key.
- Restoring from a replica re-checks every chunk hash; a tampered chunk fails (`integrity_failed`) and the report lists it.

## 14. Events

Published on the existing events system, all with `event_id`, `at`, `computer_id`:

| Name | Type | When |
|---|---|---|
| `files.changed` | `FilesChanged` | any create, modify, move, rename, delete, trash, restore |
| `files.inbox.dropped` | `InboxDropped` | a file lands in Inbox; carries `untrusted` and the task state |
| `files.task.result` | `TaskResult` | an approved Inbox or folder-rule task finished, failed or was rejected |
| `files.share.accessed` | `ShareAccessed` | every guest access, allowed or denied. Also stored by cloud-api on the cloud events system (agents audience, push off by default), `data` = `{ computer_id, access }`. |
| `files.conflict` | `ConflictCreated` | a write landed as a conflict copy |

## 15. Storage discovery

The runtime reports `StorageDiscovery` (volumes with kind, total and free bytes, removable, encrypted; bytes per root; replica-host capacity) in its presence heartbeat (`POST /api/v1/runtime-presence/heartbeat`, key `filesStorage`; cloud-api stores it under the signing runtime's id, whatever `computer_id` says). cloud-api keeps the last report and serves it while the computer sleeps. Smart placement and the replica picker read it. Heartbeats do not count as activity.

## 16. Errors

Every error body is `ErrorBody`: the same flat `{error, message, code}` as cloud-api's `ErrorResponse`, plus Files fields (`retry_after_s`, `computer_state`, `wake`, `current`, `missing_chunks`). `error` and `code` both hold the `ErrorCode`.

| HTTP | Codes |
|---|---|
| 400 | `invalid_request`, `invalid_name`, `manifest_invalid`, `chunk_hash_mismatch` |
| 401 | `unauthorized`, `passcode_required`, `passcode_invalid` |
| 403 | `forbidden`, `feature_disabled`, `scope_violation`, `protected_root_locked`, `agent_process_denied`, `execute_denied`, `guest_wake_not_allowed`, `wake_denied` |
| 404 | `not_found`, `root_not_found`, `upload_session_not_found` |
| 409 | `already_exists`, `rev_mismatch`, `wake_consent_required`, `chunk_missing` |
| 410 | `share_expired`, `share_revoked`, `upload_session_expired`, `cursor_expired` |
| 413 | `payload_too_large` |
| 422 | `integrity_failed` |
| 429 | `rate_limited` |
| 503 | `asleep`, `waking`, `offline`, `replica_unavailable`, `key_unavailable` |
| 507 | `quota_exceeded` |
| 501 | `unsupported` |
| 500 | `internal` |

## 17. Surfaces this contract serves

| Surface | Uses |
|---|---|
| iOS / iPadOS | Swift types in the File Provider extension and app; relay transport (mesh only if the tsnet spike passes); wake prompt from `WakeHint` |
| Android | Kotlin types in the DocumentsProvider |
| macOS / Windows / Linux Desktop | Rust and Swift types; loopback for own runtime, mesh or relay for others |
| ai.allternit.com, phone layout, PWA | TS types; relay; gateway for the guest page |
| Gizzi Code | TS types; `gizzi files …` CLI and the TUI `/files` command (section 18) |
| Backend | Rust types in allternit-api (`files_v2`) and cloud-api (relay no-wake, consent, gateway, snapshot) |
| Docs | `surfaces/docs/api/files-drive.mdx` is written from this file in step 29 |

## Stub server

`tools/files-stub/` runs this contract as an in-memory server (Node 20, no dependencies) so UI lanes can build without compiling allternit-api: `node tools/files-stub/server.mjs`, then point a client at `http://127.0.0.1:47820`. It serves the runtime routes, the cloud-api routes, wake consent, the snapshot, the gateway and a change feed, with scenario switches (awake, asleep, waking, conflict, slow, guest, error). Every response is validated against `schema/*.json`; `node tools/files-stub/server.mjs --self-test` checks that. See `tools/files-stub/README.md`.

## 18. Decisions (were open; Eoj answered 2026-11-15)

1. **TUI command name.** The Files drive owns the Gizzi TUI command `/files`, matching the `gizzi files` CLI. Today's `/files` (the list of files in context, `cmd/gizzi-code/src/cli/ui/ink-app/commands/files/`) folds into the existing `/context` command. The move is the Gizzi lane's work (step 25), not part of this contract.
2. **Asleep snapshot names.** Plain file names in the user's own R2 bucket (same trust class as uploaded files). `names_encrypted` stays in the format, default `false`, reserved and not used in v2 (section 11).

No questions are open in this contract.
