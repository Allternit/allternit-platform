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

- Read-only. Nothing here changes the host or needs root.
- Linux, macOS and Windows code sits behind `cfg(target_os = ...)`. Any other
  OS gets `ProbeError::Unsupported`, never a panic.
- Pure parsing (os-release, primary disk, rates, mount filter) is separate
  from the system calls and tested with fixtures in `fixtures/`.

## How later steps extend it

Each wave-1 step adds a module and keeps this crate a library:

| Module | Route | Notes |
|---|---|---|
| `services` | `GET /v1/host/services` | systemd, supervisord, docker, launchd, Windows SCM (built, see below) |
| `ports` | `GET /v1/host/ports` | listening sockets, owning process, exposure |
| `updates` | `GET /v1/host/updates` | apt, dnf, softwareupdate, Windows Update |
| `logs` | `GET /v1/host/logs/stream` | log lines masked before they leave the machine |
| `actions` | `POST /v1/host/actions` | the fixed action list; needs an approval id, records a change and its undo; the only part that writes |
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
the service's PID. Ports are filled by the `ports` step, so services carry none.
Parsers are pure and tested against `fixtures/services/`.
