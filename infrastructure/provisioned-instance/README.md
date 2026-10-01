# Provisioned per-subscription instance (cloud computer)

> **Update 2026-10-01 (plan PLAN-cloud-computer-provisioning-2026-09-30, A3/B):**
> every paying user's cloud computer is an unprivileged Incus container from
> the `allternit-desktop` image, named deterministically from the user and
> subscription, sized from the plan, and signed in by the **Allternit Desktop
> app** through the bootstrap contract below. `init.sh` (allternit-api node)
> is now the legacy lane: it only ships when
> `ALLTERNIT_PROVISION_NODE_INIT=1`, because it would consume the same
> one-time token the Desktop app redeems.

## Bootstrap contract (A2) — what the Desktop app reads

On create, the provisioning service mints a one-time token (24 random chars;
only its sha256 is stored, in `provisioned_instances.pairing_code_hash`) and
writes it into the container **before first start** at:

```
/etc/allternit/bootstrap.json      mode 0600, dir /etc/allternit 0700
```

```json
{ "api": "https://api.allternit.com", "token": "<one-time>", "instance_id": "pi_…", "user_id": "user_…", "expires_at": "<RFC3339>" }
```

- Owner = the uid/gid the Desktop app runs as: `ALLTERNIT_PROVISION_DESKTOP_UID`
  / `_GID` on cloud-api, default `0:0` (the current image runs
  `allternit-desktop.service` as root). Set them when A1 moves the app to a
  dedicated user.
- Delivery: primary is the Incus file API (`POST /1.0/instances/<name>/files`,
  pushed between create and start). The same file also rides in
  `cloud-init.user-data` as a fallback, but the current `allternit-desktop`
  image never runs cloud-init's network stage (`cloud-init.service` is skipped
  with `ConditionResult=no`, so `write_files` never runs — verified on
  allternit-standby 2026-10-01). Fixing that belongs to the A1 image rebuild
  (e.g. `cloud-init clean` before publish, and check the unit's conditions).
- `api` is `ALLTERNIT_CLOUD_API_BASE` (default `https://api.allternit.com`).
- `expires_at` = create time + `ALLTERNIT_PAIRING_CODE_TTL_HOURS` (default 24).

### Redeem = the normal Desktop pairing, pre-approved

No separate sign-in endpoint. The Desktop app runs its usual pairing
(`auth-manager.ts` `startPairing` / `exchangePendingPairing`) and adds the token:

1. `POST /api/v1/runtime-pairings` with the usual body
   (`name`, `runtimeType`, `hostname`, `platform`, `version`,
   `publicKey` = raw ed25519 base64url) **plus** `"bootstrapToken": "<token>"`
   (or header `X-Allternit-Bootstrap-Token: <token>`). A valid token
   (unexpired, instance live and not yet paired) creates the pairing
   **already approved** for the instance's owner, forces
   `runtimeType: "provisioned"` and the name `"Allternit cloud computer"`, and
   links it to the `provisioned_instances` row. No browser approval step.
   Invalid / expired / already-used → `401` (expired → token-expired error).
2. `POST /api/v1/runtime-pairings/exchange`
   `{ pairingId, deviceCode, signature }` with the signature over
   `allternit-runtime-pairing:<pairingId>:<challenge>` → the usual
   `{ runtimeId, userId, userEmail, deviceToken, tokenType, expiresAt, capabilities }`.
   The exchange binds `runtime_devices` (`kind = 'provisioned'`) to the
   instance and **consumes the token** (hash cleared). It is not consumed at
   step 1, so a crash between 1 and 2 can retry within the expiry.
3. Desktop deletes `/etc/allternit/bootstrap.json` after a successful
   exchange and holds the relay exactly as on a Mac.

The older explicit form (`runtimeType: "provisioned"`,
`provisionedInstanceId`, `provisionedBootstrapToken`) still works.

## Lifecycle (B1/B2)

| Event | Effect |
|---|---|
| Stripe `customer.subscription.created/updated` active/trialing | `create(user, subscription)` fire-and-forget — only when `ALLTERNIT_PROVISION_ON_PAYMENT=1` (go-live switch, default off). Idempotent: repeats return the existing instance. |
| `customer.subscription.deleted` | stop now → `suspended`, `delete_after = cancel + 30 days`; disk-only snapshot (`stateful: false`, no CRIU) published as a compressed local image `allternit-snap-<name>` (images outlive the instance; instance snapshots do not), kept 6 months. |
| Hourly lifecycle task (`PROVISIONED_LIFECYCLE_SECONDS`) | retries missing snapshots; deletes suspended instances past `delete_after` (only once the snapshot exists); deletes snapshot images past 6 months. |
| Re-subscribe within 30 days | the suspended container is started and re-bound to the new subscription. |
| Re-subscribe within 6 months | a new container is created from the snapshot image on the host that holds it, with a fresh bootstrap token. |
| Container missing on its host | reconcile marks the row `error` ("missing: …"), closes metering, releases capacity — never a "running" ghost. |

Sizes come from `plan_tiers.computer_base_*` keyed by the subscription's
plan id (migration 018): Plus 2 vCPU / 4 GiB / 20 GB (burst 4 / 8 GiB),
Super 4 / 8 GiB / 40 GB (burst 8 / 16 GiB), Ultra 8 / 16 GiB / 80 GB
(burst 16 / 32 GiB). `ALLTERNIT_PROVISION_CPU/MEMORY_MB/DISK_GB` are the
fallback when a plan has no sizing row. Optional
`ALLTERNIT_PROVISION_SNAPSHOT_COMPRESSION` (e.g. `zstd`) sets the image
compression.

## Fleet (G2)

Hosts are rows in `provisioned_hosts`, registered through the admin route
`POST /api/v1/provisioned-hosts`; the scheduler picks the enabled host with
the most free memory (then cpu, then id) that fits the plan size.

- `mail` — the first fleet host (already registered in prod).
- `allternit-standby` — Incus 6.0 at `https://100.83.199.24:8443`
  (mesh-only), dir pool `default`, `incusbr0`, trusted client certs
  `allternit-api` / `allternit-desktop`, image `allternit-desktop`
  (fingerprint 86552d91…). Not registered in prod yet: Eoj registers it
  (8 cores, 23 GB RAM, ~122 GB disk; leave failover headroom, plan G3).

---

## Legacy: init.sh (allternit-api node)

`init.sh` is the first-boot contract for the P2 per-subscription provisioning
lane — one unprivileged Incus container per paid subscription (decisions
A3/D2/D3 in `docs/architecture/2026-09-03-control-plane-data-plane-decision.md`).
The cloud-api provisioning service
(`cmd/allternit-cloud-api/src/services/provisioning.rs`) embeds this file and
ships it to the container as cloud-init user-data:

```yaml
#cloud-config
write_files:
  - path: /usr/local/sbin/allternit-node-init
    owner: root:root
    permissions: '0755'
    content: |
      <init.sh, embedded at provisioning-service build time>
runcmd:
  - env ALLTERNIT_PROVISIONED_INSTANCE_ID=… ALLTERNIT_PAIRING_CODE=… … /usr/local/sbin/allternit-node-init
```

Parameters travel as environment variables — the DevPod "options as env"
contract the ADR adopts. Incus applies `user.user-data` on first container
start; every re-provision gets a fresh container, so no upgrade-in-place path
is needed for v1.

## What the script does (six steps)

1. **Dependencies** — curl, ca-certificates, openssl, python3 (python3 is part
   of the contract: the pairing dance parses JSON and does base64url).
2. **Install + pin allternit-api** — `$ALLTERNIT_NODE_RELEASE_URL` must be a
   `.tar.gz` whose archive root contains the `allternit-api` binary;
   `$ALLTERNIT_BINARY_SHA256` pins the digest and is verified before install.
   The provisioning service should always pass the pin.
3. **Phone home / pairing** — generates an Ed25519 keypair, calls
   `POST /api/v1/runtime-pairings` with `runtimeType: "provisioned"` and the
   one-time `provisionedBootstrapToken` (the pairing code), then exchanges the
   pairing (signing `allternit-runtime-pairing:<pairingId>:<challenge>`). The
   exchange returns the long-lived device credential and the node id
   (`rt_…`); both land in `/etc/allternit-node/env` (mode 0600). Server-side,
   the exchange binds the new `runtime_devices` row (`kind='provisioned'`) to
   the `provisioned_instances` row and flips it to `running`. This is the only
   registration path — **no inbound ports** (ADR A1, DevPod agent-phones-home).
4. **Mesh join (optional)** — `tailscale up` with the operator-supplied
   Headscale pre-auth key. Best-effort: the outbound WebSocket relay remains
   the primary control path.
5. **Supervisor** — systemd units when systemd is PID 1 (standard Incus Ubuntu
   images): `allternit-node.service` runs a restart loop that also
   **heartbeats `POST /api/v1/runtime-devices/:id/heartbeat` every 60 s**, so
   the fleet scheduler's status enum and the node registry's `last_seen_at`
   stay honest. Without systemd the loop is started detached (no boot
   persistence — fleet images should use systemd).
6. **Daily backup hook** — `allternit-node-backup.timer` (systemd) snapshots
   the SQLite data dir (`/var/lib/allternit-node`, decision D3: per-customer
   instance = per-customer SQLite) and invokes `$ALLTERNIT_BACKUP_COMMAND`
   with the snapshot path as `$1`.

### Backup contract (restic/rclone-agnostic)

Nothing about a specific backup tool is hardcoded:

- `ALLTERNIT_BACKUP_COMMAND` — arbitrary command; receives the snapshot path
  as `$1`. Point it at e.g. a wrapper doing `restic backup "$1"` or
  `rclone copy "$1" remote:…`.
- Upload credentials live in `/etc/allternit-node/backup.env` (mode 0700),
  provisioned by ops out of band. The script never writes credentials.

### Health reporting

Liveness is the existing heartbeat mechanism: the supervisor loop POSTs the
device heartbeat every 60 s, which stamps `runtime_devices.last_seen_at`.
Node resolution (`services/node_resolution.rs`) and the pairing UI already
treat `last_seen_at` within the 120 s staleness window as "online", so the
provisioned node becomes routable the moment init step 3 completes and stays
honest while the container runs.

## Status: contract implemented, fleet pending

- The script is exercised only by review + shellcheck so far. A real end-to-end
  run needs: a fleet host registered in `provisioned_hosts`, the pinned
  `allternit-node` Incus image on that host, and a published
  `allternit-api` release tarball.
- The release URL the service defaults to is a placeholder
  (`ALLTERNIT_NODE_RELEASE_URL`); production must set it (and the sha256 pin)
  before any create() call reaches a live host.
