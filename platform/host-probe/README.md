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
| `services` | `GET /v1/host/services` | systemd, supervisord, docker, launchd, Windows SCM |
| `ports` | `GET /v1/host/ports` | listening sockets, owning process, exposure |
| `updates` | `GET /v1/host/updates` | apt, dnf, softwareupdate, Windows Update |
| `logs` | `GET /v1/host/logs/stream` | log lines masked before they leave the machine |
| `actions` | `POST /v1/host/actions` | the fixed action list; needs an approval id, records a change and its undo; the only part that writes |
| `findings` | `GET /v1/host/findings` | rules over the above (disk, failed service, security updates, exposed port) |

allternit-api mounts the routes; this crate holds no HTTP code.
