# gizzi in a self-hosted Agents Server

gizzi plays three parts in Allternit Self-Hosted Agents. The server API and
the worker protocol are in `cmd/allternit-api/openapi/selfhost-v1.yaml` and
`docs/SELFHOST_WORKER_PROTOCOL.md` (repo root).

## 1. The agent loop child (`GIZZI_TOOL_HOST=remote`)

`allternit-agents-server` runs `gizzi serve` as a child with:

| Variable | Value |
|---|---|
| `ALLTERNIT_SELF_HOSTED` | `1` (offline switch, below) |
| `GIZZI_TOOL_HOST` | `remote` |
| `GIZZI_TOOL_HOST_URL` | The server's loopback URL |
| `GIZZI_TOOL_HOST_TOKEN` | The internal token made at server start |
| `GIZZI_TOOL_HOST_TIMEOUT_MS` | Optional. How long one call may wait, including the wait for a worker (default 10 minutes; bash calls get their own timeout plus 60 s) |

Model calls and MCP stay in the child. Every sandbox tool (`bash`, `read`,
`write`, `edit`, `multiedit`, `apply_patch`, `notebook_edit`, `grep`, `glob`,
`list`, `symbol_edit`, `task_list`, `task_output`, `task_stop`, `scratchpad_*`)
goes to `POST /internal/v1/sessions/{gizzi session id}/tool_calls` after the
usual hard-ban and hook checks (`src/runtime/tools/remote-host.ts`, hooked in
`ToolDispatcher.executeInitialized`). The result comes back in the local tool
shape, so the session loop and UI don't change. Remote mode never runs a tool
locally: no URL or token, no worker (504), a timeout (408) or a transport error
is a tool error the model sees. `run_code` is refused, since it would run code
on the server.

`POST /session/import` (`{title?, agentID?, messages}`; messages in gizzi's
stored message + parts shape) creates a session from a transcript. The server
uses it to rebuild a session from its events after a restart.

## 2. The worker (`gizzi worker`)

```sh
gizzi worker --server https://agents.example.internal:8400 \
  --environment env_... --environment-key aek_... \
  [--worker-id NAME] [--block-ms 25000] [--drain] [--reclaim-older-than-ms N] \
  [--auto-stop SECS] [--on-work ./spawn.sh] [--workdir /workspace]
```

Env equivalents: `ALLTERNIT_SERVER_URL`, `ALLTERNIT_ENVIRONMENT_KEY`,
`ALLTERNIT_ENVIRONMENT_ID`, `ALLTERNIT_WORKER_ID`, `ALLTERNIT_WORKDIR`.

It claims a work item, renews the lease every 20 s (a 409 stops the item at
once and nothing more is posted for it), long-polls the session's tool calls,
runs each with gizzi's own tools through `ToolDispatcher` in the workdir, and
posts the result (truncated to 100 KiB with a note). It completes the item when
`tool_calls/next` answers 204 with `X-Session-Closed: true`. SIGTERM releases
the item and exits 0. Logs are JSON lines; the environment key and the session
secret never appear in them.

`bash` runs under `Sandbox.wrap` (bubblewrap on Linux, sandbox-exec on macOS)
with the claim's environment policy: `none` = no network, `limited` = only
`allowed_hosts` (exact match) through the CONNECT proxy, `full` = open. Git
hosts and push refs are checked by a `git` wrapper and pre-push hook. A Linux
worker needs `bubblewrap`, and `socat` for `limited`.

`--on-work SCRIPT` runs the script once per item while this process keeps the
lease. The script gets `ALLTERNIT_SERVER_URL`, `ALLTERNIT_WORK_ID`,
`ALLTERNIT_SESSION_ID`, `ALLTERNIT_WORK_SECRET`, `ALLTERNIT_LEASE_GENERATION`
and `ALLTERNIT_ENVIRONMENT_POLICY`, and usually starts a per-session sandbox
that runs `gizzi worker attach` (serves that one item from those variables).

systemd: `packaging/systemd/gizzi-selfhost-worker.service` with
`gizzi-selfhost-worker.env.example`.

## 3. Client profiles (`gizzi runtime`)

```sh
gizzi runtime register --self-hosted --server https://agents.example.internal:8400 --api-key ask_...
gizzi runtime status            # server info, environments, workers online, queue depth
gizzi runtime status --json
```

The key is checked against the server, then kept in the credential store
(macOS Keychain, Windows DPAPI, else the marked 0600 fallback file), never in
config or logs. `http://` is accepted only for loopback and private addresses.
The same commands are under `gizzi agents harness`.

## Offline switch (`ALLTERNIT_SELF_HOSTED=1`)

gizzi makes no calls to `*.allternit.com` or `*.gizziio.com`
(`src/shared/selfhost/offline.ts`). It is set for the server child and the
worker (`gizzi worker` turns it on by itself).

- One guard, installed by the first import of `src/cli/main.ts`, refuses those
  hosts in `fetch`, `WebSocket`, `EventSource` and node `http`/`https` with a
  `host_not_allowed` error.
- It turns on "essential traffic only": telemetry, analytics, GrowthBook,
  update checks, release notes and similar calls skip themselves.
- Call sites that would otherwise fail on every start return early: Allternit
  Cloud model list and plan, subagent catalog sync, remote-control push,
  instance registration, device-token rotation, Clerk JWKS and gateway-key
  validation, the unknown-route cloud proxy, `Installation.latest`, and
  `serve --mesh` with the default Allternit coordination server.
