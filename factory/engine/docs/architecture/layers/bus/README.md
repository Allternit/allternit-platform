# Bus & Transport Layer

## Purpose
Provides deterministic coordination across tmux panes, unix sockets, mail threads, and other transport targets. Every bus message is also an event in the ledger so we maintain a single audit trail.

## Stores
- `.allternit/bus/queue.db` (SQLite) — durable queue with `(id, correlation_id, to, from, kind, payload, status, created_at)` so the runner can dispatch messages atomically.
- `.allternit/transports/tmux/<pane>.json` and `.allternit/transports/socket/<agent>.json` — keep `TransportState` (busy flag, owner, iteration, last_message_id, stdout/stderr). Supports inspections and prevents overwrites.

## Commands
| Command | Purpose |
| --- | --- |
| `allternit rails bus send --target tmux:<pane>` | Pushes a `tmux` message to the queue, sets `TransportState.busy`, and waits for the transport runner to respond. |
| `allternit rails bus socket send --agent <id>` | Similar for unix sockets; payloads/responses are structured JSON with `status`, `stdout`, `stderr`. |
| `allternit rails transport tmux-inspect` / `socket-inspect` | View active contexts, busy status, iteration IDs, owners, and last outputs. |
| `allternit rails bus tmux-runner` / `socket-runner` | Background workers that execute tmux/sockets commands, release busy flags, and write replies. |

## Coordination safeguards
1. Gate checks call `TransportState::ensure_available` before writing to a target, so a busy pane/agent refuses the request and returns a descriptive error (pane `X` is busy for iteration `Y`).
2. Each transport run emits ledger events `BusMessageSent`, `BusMessageDelivered`, `BusMessageReplied`, `BusMessageFailed`, all with the same `correlation_id` for traceability.
3. CLI tooling (tmux/socket inspect) reads `.allternit/transports` files so humans see live iterations before sending messages, preventing double work.
