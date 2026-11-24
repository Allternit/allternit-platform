# allternit-node — always-on `node.core` daemon

`allternit-node` is a Rust daemon that keeps a paired Mac present on the
Allternit cloud runtime relay even when Allternit Desktop is quit. Fabric
Transport can then use the machine as a node for `node.core` surfaces:
health, exec, fs, processes, metrics, launch, and terminal sessions
(see "Terminals" below).

- **Pairing**: none of its own. The daemon adopts the runtime identity that
  Allternit Desktop already persists (see "Identity" below). Pair with the
  desktop app first, then `sudo allternit-node install`.

## Commands

| Command | Purpose |
|---|---|
| `allternit-node run` | Foreground run (service entry point). |
| `allternit-node install` | Install the LaunchDaemon (macOS) / systemd unit (Linux), start at boot. Requires root. |
| `allternit-node uninstall [--purge]` | Remove the service. `--purge` also removes the shared pairing identity (unpairs the desktop) and node config. The desktop app is never touched. |
| `allternit-node status` | Service status + log location. |
| `allternit-node logs` | Tail the daemon log. |

## Identity

The desktop app is the **single writer** of the daemon identity. On
pair/rotate, `surfaces/allternit-desktop/src/main/auth-manager.ts` writes a
plain-JSON copy (mode 0600) to `~/.config/allternit/runtime-identity.json`:

```json
{ "runtimeId": "rt_…", "deviceToken": "…", "userId": "…", "expiresAt": "RFC3339" }
```

`expiresAt` must be RFC3339 — a numeric epoch is rejected. On unpair/revoke
(`clearSession`) the desktop deletes the file.

**Rotation flow**: the desktop rotates the device token with the cloud →
rewrites both its own encrypted identity and this file → the daemon re-reads
the file before every relay connect attempt and adopts the rotated
credential (logged as `adopted rotated identity`). If the file is missing or
unreadable at reconnect time, the daemon keeps serving with its last-known
identity and logs a warning; transient disk issues never kill a healthy
relay.

## No competition with the desktop app

The daemon and the desktop are designed to coexist without overlap:

- **Outbound-only**: the daemon binds zero listening ports. It holds one
  outbound WSS connection to the cloud runtime relay; every `node.core`
  request arrives over that socket.
- **Port ownership**: the desktop app owns `:8013` (the local gateway /
  allternit-api sidecar) and `:8477` (phone-remote / screen-capture
  surfaces). The daemon never binds either port.
- **Surface ownership**: the desktop serves the full capability set
  (screen capture, ACI/browser, voice, providers, phone remote). The daemon
  advertises only `node.core` + `runtime:connect/execute/terminal/files` —
  explicitly *not* `runtime:remote_control` or provider capabilities — so a
  request that needs the GUI session routes to whichever runtime is the
  desktop, and everything else can be served by the daemon.
- **`node.launch restart` is surgical**: it only signals processes whose
  exact name is `Allternit Desktop` (via `pgrep -x`), then relaunches the
  app. Nothing else on the machine is touched.
- **Single writer**: the desktop app writes the identity file (see above);
  the daemon only reads it.

## Terminals

The daemon serves the same terminal API as allternit-api on Desktop
(`/terminal/create`, `/terminal/list`, `/terminal/:id/input|resize|close|stream`,
documented in `surfaces/docs/api/terminal.mdx`), from one of two backends:

1. **The Factory pane engine, when one answers** (`allternit-factory pane
   server`). The daemon serves terminals through the shared
   `allternit-terminal-service` crate (`factory/terminal-service`), the same
   code allternit-api uses: create with `command`/`args`/`env`/`title`,
   `/terminal/list` with titles, `?preview=N` screen rows and `pane_id`, and
   `close {"graceful":true}`. Terminals live in the engine, so they **survive a
   daemon restart**: the next daemon lists them and a stream re-attaches with
   the scrollback replayed. Session ids are the engine's terminal ids.
2. **In-process PTYs otherwise** (portable-pty, `src/terminal.rs`): what Orgo,
   headless and older cloud images get today, since they have no pane engine.
   Create takes the same body (`command`, `args`, `env` and `title` work too),
   and `/terminal/list` reports the PTYs with `pane_id: null` and no preview.
   These terminals **end when the daemon restarts**, and a graceful close is a
   plain close.

The daemon never starts the engine; its service manager (or Desktop) owns
it. At startup the daemon probes `node-config.json`'s `pane_socket` if set,
else `$ALLTERNIT_FACTORY_PANE_SOCKET`, `$HERDR_SOCKET_PATH` and the engine's
default `~/.config/ao/sessions/ao/herdr.sock` (also root's, where the cloud
image's `allternit-factory-pane.service` runs it). A probe is a real
`factory.terminal.list` call, so an engine too old to serve terminals counts
as missing. While no engine has answered, a terminal request probes again at
most every 30 s; when the engine stops answering (503), the next request
probes again. PTY sessions the daemon already holds stay on the PTY.

**Which backend is serving:** `GET /terminal/list` has a top-level
`"engine": "pane"` or `"engine": "missing"`. The daemon log says the same
(`terminals: served by the Factory pane engine` / `no Factory pane engine
answers`). Step 17 of the terminal-workspace plan installs the pane engine
on every computer image (Orgo and headless included), after which `missing`
means a computer built from an older image.

## Lifecycle cleanliness

- SIGTERM (launchd shutdown) and SIGINT kill and reap every tracked PTY
  child before exit — no zombie terminal sessions across restarts. Pane
  engine terminals are the engine's and keep running.
- `node.exec` waits for and reaps what it spawns; on timeout the child is
  killed and reaped, never orphaned.
- `node.launch` uses `open -na "Allternit Desktop"` on macOS: the launcher
  resolves immediately and the desktop is not a child process the daemon
  owns.

## Service user (macOS)

When the pairing user is known (`$SUDO_USER` at install time, else the owner
of the identity file), the LaunchDaemon runs as that user via a `UserName`
key and logs to `~/Library/Logs/allternit/node.log` (a non-root daemon
cannot write `/Library/Logs`); `install` creates and chowns that directory.
Otherwise the daemon runs as root with logs at
`/Library/Logs/allternit/node.log` (headless installs).
