# host-probe

The read-only host collector behind the Computers console. It runs on the
user's own computer (a paired Mac, a Linux server, a Windows PC, or a cloud
computer) inside the node runtime and answers the `/v1/host/*` routes in
`surfaces/docs/dev/computers-console-contract.mdx`. The JSON matches the
TypeScript types in allternit-ai `src/lib/computers/console-types.ts`
(camelCase).

## What it does today

`GET /v1/host/snapshot` data: CPU use, cores and load, memory, every real
mounted volume plus the primary one, network throughput, OS name, version,
family, kernel, arch, hostname, and uptime.

```rust
// One-shot (blocks ~200 ms for the two-point CPU and network reading):
let snap = host_probe::snapshot()?;

// Long-lived (the heartbeat): keep a Sampler, call it each tick.
let mut sampler = host_probe::Sampler::new()?;
let snap = sampler.sample()?; // rates since the previous call
```

Both are blocking; call them from `tokio::task::spawn_blocking` in async code.

Print one snapshot: `cargo run -p host-probe --example snapshot`.

## Rules

- Read-only, except the `actions` module (below). Nothing else changes the
  host or needs root.
- Linux, macOS and Windows code sits behind `cfg(target_os = ...)`. Any other
  OS gets `ProbeError::Unsupported`, never a panic.
- Pure parsing (os-release, primary disk, rates, mount filter) is separate
  from the system calls and tested with fixtures in `fixtures/`.

## How later steps extend it

Each wave-1 step adds a module and keeps this crate a library:

| Module | Route | Notes |
|---|---|---|
| `services` | `GET /v1/host/services` | systemd, supervisord, docker, launchd, Windows SCM (built, see below) |
| `ports` | `GET /v1/host/ports` | listening sockets, owning process, exposure (built, see below) |
| `updates` | `GET /v1/host/updates` | apt, dnf, softwareupdate, Windows Update (built, see below) |
| `logs` | `GET /v1/host/logs/stream` | log lines masked before they leave the machine (built, see below) |
| `actions` | `POST /v1/host/actions` | the fixed action list; needs an approval id, records a change and its undo; the only part that writes (built, see below) |
| `findings` | `GET /v1/host/findings` | rules over the above (disk, failed service, security updates, exposed port) |

allternit-api mounts the routes; this crate holds no HTTP code.

## Services (`services` module)

```rust
let list = host_probe::services()?;                       // Vec<HostService>
let report = host_probe::services_report(&ServiceOptions::default())?; // + provider status
```

Providers, each behind `cfg` plus runtime detection:

| Provider | OS | Command (fixed args, 3 s timeout, no shell) |
|---|---|---|
| `systemd` | Linux, when `/run/systemd/system` exists | `systemctl list-units --type=service --all --output=json` (text `--no-legend --plain` on systemd older than 246), then `systemctl show -p Id,MainPID,ActiveEnterTimestamp,UnitFileState` |
| `supervisord` | Linux, macOS (Orgo cloud computers) | `supervisorctl status` |
| `docker` | any | `docker ps -a --format '{{json .}}'` |
| `launchd` | macOS | `launchctl list`; `com.apple.*` and anonymous app jobs hidden unless `ServiceOptions::include_apple` |
| `scm` | Windows | `Get-Service` through `powershell -NoProfile`, `sc queryex` as fallback |

A provider that is missing, denied (for example Docker without socket access) or
slow shows up in `ServicesReport.providers` as `available: false` with a reason.
It is not an error. `services()` fails with `ProbeError::Unsupported` only when
no provider can run on the OS.

JSON follows the contract (`name`, `unit`, `state`, `enabled`, `description`,
`pid`, `memoryBytes`) plus additive `provider`, `cpuPercent` and `since`.
`state` is `running` | `stopped` | `failed` | `unknown`; a restart loop,
a non-zero exit (Docker, Windows) and a launchd job with a non-zero exit
status count as `failed`. `memoryBytes` and `cpuPercent` come from sysinfo for
the service's PID. `services()` carries no ports; `services_with_ports()` fills the additive `ports` list by matching each service's main PID against the listening sockets.
Parsers are pure and tested against `fixtures/services/`.

## Command runner (`cmd` module)

One runner for every module that shells out: no shell, a timeout, the child
killed when it passes. A missing or slow tool is a `SourceStatus`
(`available: false` plus a reason) in the report, never an error.

## Ports (`ports` module)

```rust
let list = host_probe::ports()?;                 // Vec<HostPort>
let report = host_probe::ports_report()?;        // + per-source status
let services = host_probe::services_with_ports()?; // HostService.ports filled by PID
```

| OS | Source |
|---|---|
| Linux | `ss -ltnupH`; without `ss`, `/proc/net/{tcp,tcp6,udp,udp6}` plus the socket inodes in `/proc/<pid>/fd` (only PIDs the caller can read) |
| macOS | `lsof -nP -iTCP -sTCP:LISTEN` and `lsof -nP -iUDP` (connected UDP rows are skipped) |
| Windows | `Get-NetTCPConnection -State Listen` and `Get-NetUDPEndpoint` joined with `Get-Process`; `netstat -ano` plus sysinfo names as fallback |

JSON follows the contract: `port`, `protocol` (`tcp` | `udp`), `address`,
`process`, `pid`, `exposure`. `exposure` is by bind address: `loopback`
(127.0.0.0/8, `::1`), `public` (the wildcard `0.0.0.0` / `::` / `*` or any
non-private address) and `lan` (RFC 1918, link-local, unique-local). The
mesh range `100.64.0.0/10` and `fd7a:115c:a1e0::/48` is `lan` with the
additive `mesh: true`. Without root, sockets owned by other users have no
`pid` or `process`.

## Updates (`updates` module)

```rust
let list = host_probe::updates()?;                              // Vec<HostUpdate>
let report = host_probe::updates_report(&UpdateOptions::default())?; // + sources, staleness
```

Read-only: it never runs `apt update`, so the list is as fresh as the host's
last refresh. `UpdatesReport.lastRefreshed` (apt's periodic stamp or lists
mtime) and `stale` (older than 7 days) say how fresh.

| Source | Command | Security flag |
|---|---|---|
| apt (Debian, Ubuntu) | `apt list --upgradable` (15 s) | a `-security` suite in the line |
| dnf (Fedora, RHEL) | `dnf check-update --quiet` (exit 100 = updates, 45 s) and `dnf updateinfo list --security` | advisory present; CVE ids kept |
| macOS | `softwareupdate --list` (20 s) | "security" in the label or title; `recommended` and `requiresRestart` from the tool |
| Windows | Windows Update Agent COM via `powershell` (60 s) | MSRC severity or the Security Updates category; CVE ids from the update |

JSON follows the contract: `id`, `name`, `current`, `available`, `security`,
`kind` (`package` | `os`), plus additive `source`, `cves`, `requiresRestart`
and `recommended`. `current` is only known from apt; Windows has no version
string, so `available` is the KB number (or the title when there is none).

## Logs (`logs` and `mask` modules)

```rust
use host_probe::{stream_logs, tail_logs, LogOptions, LogTarget};

let back = tail_logs(LogTarget::Systemd { unit: "nginx.service".into() }, 200)?; // Vec<LogLine>
let live = stream_logs(LogTarget::Docker { container: "web".into() }, LogOptions::default())?;
for line in live { /* LogLine { ts, unit, level, message } */ }  // drop `live` to stop
```

Both are blocking; run the iterator on a blocking thread (or use
`recv_timeout`) and write each `LogLine` as one SSE `data:` JSON line.

| Target | Source (fixed args, no shell) |
|---|---|
| systemd (Linux) | `journalctl -u <unit> -f -o json --lines N` (`-p err` for errors only); `MESSAGE`, `PRIORITY`, `__REALTIME_TIMESTAMP` |
| supervisord | `supervisorctl tail -f <program>` (backlog: `supervisorctl tail -<bytes>`) |
| Docker | `docker logs -f --tail N --timestamps <id>` (stdout and stderr merged) |
| file | a regular file under an allow-listed root (`/var/log`, `/Library/Logs`, `~/Library/Logs` on macOS, `%ProgramData%` on Windows; set `LogOptions::file_roots`); rotation and truncation are followed by reopening when the inode changes or the file shrinks |
| macOS | `log stream --style ndjson --predicate 'process == "<name>"'` |
| Windows | `Get-WinEvent` polled every 2 s for the provider (Application and System logs) through `powershell` |

Unit, container, program and process names must match `[A-Za-z0-9@._:-]+`
(max 255, never starting with `-`); a Windows provider may also hold inner
spaces. Anything else is `LogError::InvalidTarget` before a command runs. A
target for another OS is `LogError::Unsupported`; a missing tool is
`LogError::Unavailable`. `level` is `debug` | `info` | `warn` | `error`, from
the journal priority, macOS message type or Windows event level, or from a
level word at the start of the text. `errors_only` keeps `error` only.
Dropping a `LogStream` kills the child process and stops the threads.

### Privacy

- Every line is masked at the node, before it can leave this crate:
  IPv4 and IPv6 addresses (loopback stays; `MaskOptions::keep_mesh` also keeps
  `100.64.0.0/10`; `ips: false` turns IP masking off), email local parts
  (`[email]@example.com`), JWTs, `Bearer` and `Basic` credentials,
  `password=` / `secret=` / `token=` / `api_key=` values, AWS-style keys
  (`AKIA...`), private key blocks (including the body lines), and hex or base64
  values of 32+ characters in `key=value` context.
- Nothing is stored here: no file, no buffer beyond the in-flight channel.
  Retention is a cloud setting and is off by default.
- Masking is best effort on free text. It is table-tested in
  `fixtures/logs/mask-cases.txt`; add a row there for any new leak.

## Actions (`actions` and `changes` modules)

`POST /v1/host/actions` body `{ type, target, approval_id }` (`approvalId` is
accepted too; optional `restorePoint`), answer `{ changeId, undo?, note? }`.

```rust
let engine = host_probe::ActionEngine::system(); // real host, store in the runtime home
let res = engine.run(&req)?;                     // blocking: use spawn_blocking
```

| `type` | `target` | Runs | Undo |
|---|---|---|---|
| `service.start` / `stop` / `restart` | unit or unique name from `services` | `systemctl`, `supervisorctl`, `docker`, `launchctl start/stop`, `net start/stop` | start ↔ stop; restart is not undoable |
| `port.free` | port number | stops the owning service if `services` knows it, else `kill -TERM` / `taskkill /T /F` | `service.start` when a service held it; a killed process is not undoable |
| `updates.install` | update id or `all-security` | `apt-get install --only-upgrade`, `dnf upgrade`, `softwareupdate --install`, Windows Update COM by KB | not undoable (the restore point is the way back) |
| `space.clean` | `apt-cache`, `dnf-cache`, `journal-vacuum`, `docker-prune`, `brew-cleanup` (`clean_items()`) | fixed command per item | not undoable |
| `snippet.run` | saved snippet id (`<store>/snippets/<id>.json`) | `sh -c` / PowerShell `-Command` with the snippet's script | not undoable |

Order of work: unknown `type` → 400; missing, malformed or refused approval →
403 and nothing runs; target looked up through `services` / `ports` /
`updates` (only names the host reported reach a command line, no shell) →
400/404; needs root/admin without it → `needs_elevation` (403) before
anything runs, approval kept for after the elevation setup; then the approval
is claimed (single use), the change record is written as `pending`, the
commands run, and the record gets the result, masked output, the state read
back after, and the undo or a plain "Not undoable: ..." note. A permission
error at run time is also `needs_elevation` and gives the approval back. The
probe never stops the process serving it (409 `refused`). An undo is itself
an action and needs its own approval. `ActionError::http_status()` and
`body()` give the route its answer.

Change records are JSON files under `~/.allternit/host-probe/changes/`
(override with `ALLTERNIT_HOST_PROBE_DIR`); no database. `ChangeStore::list`
feeds the Activity tab. `ApprovalVerifier` lets the mounting runtime add a
signature check; the default accepts any well-formed id. Tests use a fake
`Executor`; no real command runs in tests.
