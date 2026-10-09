# @allternit/computers-server

Standalone stdio MCP server exposing the Allternit **Computers API** as MCP
tools. This is the **Phase 5 distribution surface** for Cloud Computer: any
MCP-capable agent client (Claude Desktop, gizzi-code, Kimi Code, …) can drive
Allternit computers through it.

The server is a **thin HTTP client**. It runs in one of two modes, picked by
the credential:

- **First-party (Clerk token):** talks to the `allternit-api` `/api/v1`
  routes and exposes the 22 lifecycle/control tools below. Unchanged.
- **Platform API project key (`alt_live_…` / `alt_test_…`):** talks to the
  hosted computer driver at `https://api.allternit.com/v1/computers…` and adds
  four contract tools (`computer_toolset`, `computer_toolset_schema`,
  `computer_events`, `computer_approve`). See [Project-key mode](#project-key-mode).

## Config (env)

| Var | Default | Purpose |
|-----|---------|---------|
| `ALLTERNIT_API_URL` | `http://127.0.0.1:8013` | Base URL of allternit-api. |
| `ALLTERNIT_TOKEN` | — | Clerk bearer token, sent as `Authorization: Bearer …`. A value starting with `alt_live_`/`alt_test_` switches to project-key mode. |
| `ALLTERNIT_API_KEY` | — | Platform API project key (takes precedence over `ALLTERNIT_TOKEN`). The key needs the `computers` scope. |
| `ALLTERNIT_PLATFORM_URL` | `https://api.allternit.com` | Platform API base URL in project-key mode (`/v1` is appended). |

## Run

```bash
pnpm --filter @allternit/computers-server build
ALLTERNIT_TOKEN=sk_... computers-mcp        # or: node dist/index.js
```

Register it with any MCP client as a stdio server command.

## Tools (22, 1:1 with the REST routes)

Lifecycle — `POST/GET /api/v1/computers*`:

- `computers.create` — `POST /computers`
- `computers.list` — `GET /computers` (filters: `bot_id`, `kind`, `group_id`, `include_roles`)
- `computers.get` — `GET /computers/:id`
- `computers.start` — `POST /computers/:id/start`
- `computers.stop` — `POST /computers/:id/stop`
- `computers.restart` — `POST /computers/:id/restart`
- `computers.resize` — `PATCH /computers/:id/resize` (disk resize requires stopped)
- `computers.clone` — `POST /computers/:id/clone` (body `{name?}`)
- `computers.delete` — `POST /computers/:id/delete` (204; missing ⇒ also 204)

Control:

- `computers.screenshot` — `GET /computers/:id/screenshot` → PNG, **base64** in the result JSON
- `computers.mouse` — `POST /computers/:id/mouse`
- `computers.keyboard` — `POST /computers/:id/keyboard`
- `computers.shell` — `POST /computers/:id/shell`
- `computers.files.upload` — `POST /computers/:id/files/upload?path=…`, raw `application/octet-stream` bytes (`content` arg is base64-decoded before sending)
- `computers.files.download` — `GET /computers/:id/files/download?path=…` → binary, **base64** in the result JSON

Snapshots:

- `computers.snapshots.list` — `GET /computers/:id/snapshots`
- `computers.snapshots.create` — `POST /computers/:id/snapshots` (`{stateful}`)
- `computers.snapshots.restore` — `POST /computers/:id/snapshots/:snapshot_id/restore`
- `computers.snapshots.delete` — `DELETE /computers/:id/snapshots/:snapshot_id`

Desktop templates (Phase 4) — `/api/v1/desktop-templates*`:

- `templates.list` — `GET /desktop-templates` (filters: `os`, `tag`)
- `templates.import` — `POST /desktop-templates/import` (canonical `apiVersion: allternit.ai/v1` `ComputerTemplate` doc as a YAML/JSON string)
- `templates.build` — `POST /desktop-templates/:id/build` (async golden build, 202 on start)

## Approval semantics

Routes backed by the ACI confirmation policy accept an optional **`approvalId`**
string argument, threaded verbatim as the `?approval_id=` query param:

`computers.create`, `computers.start`, `computers.stop`, `computers.restart`,
`computers.resize`, `computers.clone`, `computers.delete`, `computers.mouse`,
`computers.keyboard`, `computers.shell`, `computers.files.upload`,
`templates.build`.

The server **never mints or auto-obtains approvals**. If the action needs
confirmation, the API returns its `confirmation_required` / `approval_denied`
payload and that message is surfaced verbatim as the tool result error, along
with the `approval_id` / `action_hash` the caller must route through the ACI
handoff endpoints (`/api/aci/handoff/:id/*`).

## Project-key mode

With a project key, `tools/list` returns the 22 tools above plus:

- `computer_toolset` — `POST /v1/computers/{id}/toolset`. Args: `computer_id`,
  `toolset` (`computer` | `browser`), `member`, `input`, and optional
  `approval_grant`, `run_id`, `turn_id`, `call_index`. Text content comes back
  as MCP text, screenshots as MCP **image** content, then one text item with
  the rest of the result (`screen`, `browser_state`, `error`).
- `computer_toolset_schema` — `GET /v1/computers/{id}/toolset/schema?toolset=`
- `computer_events` — `GET /v1/computers/{id}/events?after=&limit=` (poll with `next_cursor`)
- `computer_approve` — `POST /v1/computers/{id}/approvals/{approval_id}`. Works
  for API keys only when the project's `approval_mode` is `api_key`; otherwise
  the API answers 403 `approval_requires_owner` and the owner approves in the console.

Approval flow: when an action needs approval the API answers 409
`approval_required`. The tool result is an error whose text names the
approval id and says who can approve it. After approval, resend the **same**
`computer_toolset` call with `approval_grant` set to that id.

Lifecycle tools with a `/v1` route (`computers.create`, `.list`, `.get`,
`.start`, `.stop`, `.delete`) go to `/v1`. The others (restart, resize, clone,
shell, files, snapshots, templates, raw mouse/keyboard/screenshot) return an
error in this mode; use `computer_toolset` instead.

The project needs `hosted_driver_enabled` (Allternit turns it on); until then
every `/v1/computers` route answers 404 `hosted_driver_disabled`.

## Layout

- `src/tool-spec.ts` — `McpToolSpec`-style declarations for all 22 tools
- `src/client.ts` — thin fetch wrapper over the REST API
- `src/platform.ts` — project-key mode: `/v1` client, contract tool specs + dispatch
- `src/server.ts` — `Server` + `StdioServerTransport` + tool dispatch
- `src/index.ts` — `computers-mcp` bin entry
- `skills/operating-a-computer.md` — agent skill for driving a computer with these tools (moved from the retired `platform/packages/computer-use/plugins`)
- `scripts/smoke.mjs` — JSON-RPC handshake smoke test against the built server (`pnpm --filter @allternit/computers-server smoke`)

## Verification

```bash
pnpm --filter @allternit/computers-server build
pnpm --filter @allternit/computers-server test   # vitest: client + tool↔route mapping + project-key mode, mocked fetch
pnpm --filter @allternit/computers-server smoke
```
