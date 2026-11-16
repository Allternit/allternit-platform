# Files drive stub server

An in-memory server for the Files protocol v2 (`contracts/files-drive/SPEC.md`). UI lanes (web, phone layout, PWA, iOS, Android, Gizzi) build against it without compiling allternit-api. Node 20+, no dependencies, no build step. Dev tool only: it is not a product and is never deployed.

```
node tools/files-stub/server.mjs                          # http://127.0.0.1:47820, scenario awake
node tools/files-stub/server.mjs --port 5000 --scenario asleep
node tools/files-stub/server.mjs --self-test              # exit non-zero on any failure
```

Default port `47820` (env `FILES_STUB_PORT`, flag `--port`). Env: `FILES_STUB_SCENARIO`, `FILES_STUB_COMPUTER` (default `cmp_cloud_4gb`), `FILES_STUB_WAKE_MS` (3000), `FILES_STUB_SLOW_BPS` (262144), `FILES_STUB_HEARTBEAT_MS` (20000).

## Pointing a client at it

- **Runtime routes:** base URL `http://127.0.0.1:47820/api/v1/files/v2` (the `loopback` transport). Guests use `/api/v1/files-gateway/v2` with `Authorization: Files-Guest <token>`.
- **cloud-api routes:** `http://127.0.0.1:47820/api/v1/...` (`computer-files/:id/endpoints`, `wake-consent`, `wake-settings`, `shares`, `token-keys`, `files-keys`, `files-gateway/summary|redeem`). `endpoints` lists the stub as a `loopback` candidate and as a `relay` candidate.
- **Relay:** `POST /api/v1/runtime-devices/:id/proxy` takes `{method, path, headers, body, body_encoding}` (`bodyEncoding` is accepted too, as cloud-api does) and streams the runtime's response back as-is: real status, the allowed headers, raw bytes. Same as cloud-api (SPEC section 2, Relay).
- CORS is open (`*`), so a browser dev server can call it directly.
- Auth is not checked for owner calls. Send `X-Allternit-Device-Id: dev_...` (and `X-Allternit-Device-Name`) to act as an own device. `X-Allternit-Unlock: 1` opens the protected Memory Drive root.

## Scenarios

Set with env or `--scenario` (global), `POST /__stub/scenario {"scenario": "..."}` (global, live), or the header `X-Files-Stub-Scenario` (one request).

| Scenario | Behaviour |
|---|---|
| `awake` | Default. Everything live. |
| `asleep` | `roots`, `list`, `stat`, `changes`, name `search` come from the snapshot (`served_by: snapshot`, `computer_state: asleep`, `snapshot_at`). Content, manifest, chunks, writes, uploads, content search return 409 `wake_consent_required` with a `WakeHint`. `POST /api/v1/computer-files/:id/wake-consent` returns a ticket and the stub goes `waking` then `awake` after `FILES_STUB_WAKE_MS`. Send the ticket in `X-Allternit-Wake-Ticket` to be served at once (it waits for the wake). With wake setting `never`: `wake_denied`. |
| `waking` | Metadata from the snapshot with `computer_state: waking`; content returns 503 `waking` with `retry_after_s`. |
| `conflict` | Every write or move that carries a `base_rev` is treated as stale, so `keep_both` makes a conflict copy (and `fail` returns `rev_mismatch`). Stale `base_rev` conflicts also happen in every other scenario. |
| `slow` | Chunk and content transfer throttled to `FILES_STUB_SLOW_BPS` bytes/s. |
| `guest` | The runtime routes require a Files-Guest token (401 otherwise). Gateway routes always do. |
| `inbox` | Seeds one pending Inbox task (`W-9.txt`, dropped by guest Dana) so the web, phone and Android Inbox cards show. Setting it live adds one if none is pending. Any scenario creates a task when a non-agent writes a file to Inbox. See Inbox tasks below. |
| `error` | Every request fails. `X-Files-Stub-Error: <code>` picks the `ErrorCode` (any of the 37, right HTTP status, extras such as `wake`, `retry_after_s`, `current`, `missing_chunks`). Without the header the codes cycle in order. |

Guest links (redeem with `POST /api/v1/files-gateway/redeem`): `view_downloadx9` (passcode `4417`), `upload_onlyx9`, `editx9`, `commentx9`, `view_downloadopen` (no passcode), `expired0stub` (`share_expired`), `revoked0stub` (`share_revoked`). Create more with `POST /computer-files/:id/shares` (the link token is the last part of `link_url`). `upload_only` cannot list (`forbidden`); while asleep its small write becomes a `QueuedDrop` (202). Other guests get `guest_wake_not_allowed` for content while asleep.

## Seeded data

Roots Allternit, Inbox, Agent Output, Meetings, Memory Drive (protected), Models, Shared, built from `fixtures/roots-awake.json` and `list-home.json`. Files have real bytes, so hashes and revs are real: `Lease.pdf`, `Notes.txt`, `Scan-0001.pdf`, `Closing docs/` (shared with guests). `POST /__stub/reset` restores the seed.

## Chunked upload

`POST /chunks/have-need` then `POST /uploads` (you chunk and hash with blake3; the server does not care how you cut), `PUT /uploads/:id/chunks/:hash` per needed chunk (413 over 4 MiB, 400 `chunk_hash_mismatch` on a wrong hash), `GET /uploads/:id` to resume, `POST /uploads/:id/commit`. The commit checks every chunk and the whole-file hash, then the new revision is listed, readable (Range and `If-None-Match` work) and in the change feed. `tools/files-stub/blake3.mjs` exports `blake3hex(buffer)` if you need it in a script.

## Inbox tasks

The routes from platform #2445 (`api/files-drive-inbox`): `GET /inbox/tasks?state=&limit=`, `GET /inbox/tasks/:id`, `POST /inbox/tasks/:id/approve` (`{tools?, output?}`, unknown fields and tools are 400) and `POST /inbox/tasks/:id/reject`. Owner and own devices only (guests get 403). A file dropped in Inbox by anyone but an agent makes a `needs_approval` task and a `files.inbox.dropped` event. Approve returns `approved` with the grant, then after `FILES_STUB_TASK_MS` (600) the task runs: `<stem> summary.md` and `<result>.provenance.json` appear beside the file (or in the Agent Output root with `output: agent_output`), the task becomes `succeeded` and `files.task.result` is emitted. Reject returns `rejected`, writes nothing and drops the task out of `?state=needs_approval`. The stub writes a fixed stub summary, not a model reply, and does not enforce grant expiry.

## Change feed

`GET /roots/:root_id/changes?cursor=&limit=&wait_s=` (long poll up to 25 s) and the WebSocket `/api/v1/files/v2/subscribe` (first frame `SubscribeMessage`, then `changes`, `reset`, `state`, `heartbeat` frames). An old or foreign cursor gives `reset_required: true`.

## Control routes (not protocol)

`GET /__stub/state` (scenario, wake setting, activity touches), `POST /__stub/scenario`, `POST /__stub/reset`, `GET /__stub/events` (`files.*` events emitted so far), `GET /__stub/errors` (code to HTTP status).

## What it does not do

No real file system, no auth, no agent grants beyond `X-Files-Stub-Principal: agent` on `whoami`, no real cross-computer copy (a `JobRef` that completes after about 2 s), no replica data movement, no thumbnails. Folder trash hides the subtree rather than moving it.

## Files

`server.mjs` (CLI), `stub.mjs` (the server), `selftest.mjs`, `blake3.mjs` (`node blake3.mjs --test` checks known vectors). The schema loader and validator are shared with the generator in `contracts/files-drive/schema-lib.mjs`.
