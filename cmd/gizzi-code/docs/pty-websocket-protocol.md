# PTY WebSocket Protocol

How remote clients (e.g. the iOS SwiftTerm app) attach to a terminal session
running on a standalone `gizzi serve` server.

## Lifecycle

1. **Create** — `POST /pty/create` (also under `/v1/pty/create`) with JSON
   `{ "command"?, "args"?, "cwd"?, "title"?, "env"? }`. Returns
   `{ "id", "title", "command", "args", "cwd", "status", "pid" }`.
   `command` defaults to the user's login shell.
2. **Connect** — upgrade `GET /pty/:ptyID/connect` to a WebSocket.
3. **Resize** — `PUT /pty/:ptyID` with `{ "size": { "rows": 24, "cols": 80 } }`
   whenever the client terminal changes size. (`title` can be set the same way.)
4. **Kill** — `DELETE /pty/:ptyID`.

All routes are mounted under both `/pty/...` and `/v1/pty/...`.

## Auth

The WebSocket upgrade is an ordinary HTTP request and goes through the same
auth middleware as REST:

- `Authorization: Bearer <clerk-jwt>` — validated against Clerk JWKS
  (RS256, issuer `https://clerk.allternit.com`). A valid token
  authorizes the request by itself; an invalid/expired one gets `401`.
- Otherwise, if `GIZZI_SERVER_PASSWORD` is set, HTTP Basic auth
  (`GIZZI_SERVER_USERNAME`, default `gizzi`).
- Otherwise the request passes through (loopback dev mode).

A failed upgrade returns a normal HTTP error response, never a half-open socket.

## Connection

```
wss://<host>/v1/pty/<ptyID>/connect[?cursor=<n>]
```

`cursor` is reserved for incremental scrollback resume. The current server
ignores it and always replays the full scrollback; clients must still accept
the full replay.

## Server → client frames

1. **Scrollback replay** — zero or more **text frames**, raw UTF-8 terminal
   output (ANSI escapes included), chunked at 64 KiB. Feed them to the
   terminal emulator in order.
2. **Meta frame** — one **binary frame**: first byte `0x00`, remaining bytes
   are UTF-8 JSON. Currently the only meta message is:

   ```json
   { "cursor": <length of the replayed scrollback> }
   ```

   The meta frame marks the end of the replay. Unknown meta JSON keys must be
   ignored; frames whose first byte is `0x00` must never be rendered.
3. **Live output** — **text frames**, raw UTF-8 terminal output, forwarded as
   the Factory pane engine emits it. Render directly.

## Client → server frames

Any **text frame** (or binary frame, decoded as UTF-8) is written verbatim to
the PTY's stdin — keystrokes, escape sequences, paste, etc. There is no
message envelope.

Do not send input before the meta frame arrives: input is only hooked up once
the replay has completed, and earlier frames may be dropped.

Terminal resize does **not** go over the socket — use `PUT /pty/:ptyID`.

## Close semantics

- When the PTY's process exits, the server closes the WebSocket. Reconnecting
  to the same `ptyID` replays the output and closes again while the pane
  engine still keeps the exited terminal (10 minutes, or until `DELETE`);
  `status` is `"exited"`.
- When the client disconnects, only the output subscription is torn down — the
  PTY itself keeps running (it is a terminal on the Allternit Factory pane
  engine, `allternit-factory pane`, and survives `gizzi serve` restarts). Use
  `DELETE /pty/:ptyID` to kill it.
