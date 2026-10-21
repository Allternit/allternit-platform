# Headscale ops runbook

App: `allternit-headscale` on Fly.io (region `lax`). Server URL:
`https://allternit-headscale.fly.dev`. Pinned image: `headscale/headscale:0.29.2`
(config schema verified against the v0.29.2 `config-example.yaml`).

All admin commands run inside the machine over the headscale unix socket:

```sh
fly ssh console -a allternit-headscale -C "headscale <command>"
```

## First deploy

```sh
cd infrastructure/mesh/headscale

fly apps create allternit-headscale
fly volumes create headscale_data --region lax --size 1 -a allternit-headscale
fly deploy -a allternit-headscale

# smoke test
curl -s https://allternit-headscale.fly.dev/health   # expect OK
fly logs -a allternit-headscale
```

Note: exactly one machine per volume — `min_machines_running = 1` and the
SQLite DB mean this app is single-machine by design. Do not scale past 1.

## Isolation model (per-customer ACLs)

`policy.hujson` is baked into the image (`policy.path` in `config.yaml`) and
contains exactly one rule: `autogroup:member` → `autogroup:self:*`. Combined
with one headscale **user** per customer (`clerk-<sanitized-clerk-id>`,
created by allternit-cloud-api at enroll time) and **untagged** preauth
keys, that means:

- A node's traffic may go only to nodes owned by the same headscale user,
  i.e. the same customer's devices. ACLs are default-deny, so all
  cross-customer traffic is blocked.
- No internet exit, no subnet routes: there are no `autoApprovers`, so
  advertised routes are never approved automatically (manual
  `headscale nodes approve-routes` only).
- Nodes must stay **user-owned**. Do NOT pass `--tags` when minting keys:
  a tagged node loses its user ownership, matches no rule in this policy
  (tagged nodes cannot use `autogroup:self`), and is cut off. Tagging via
  `headscale nodes tag` / `--advertise-tags` is rejected anyway because the
  policy declares no `tagOwners`. See the header of `policy.hujson` for why
  the model uses `autogroup:self` instead of per-customer tags.

Verifying isolation:

```sh
# each node shows its owning user (one user = one customer)
fly ssh console -a allternit-headscale -C "headscale nodes list"

# policy currently in force
fly ssh console -a allternit-headscale -C "headscale policy get"
```

On a connected node, `tailscale status` must list only same-customer peers.
To test that cross-customer traffic is denied, enroll one node under each
of two different users and, from node A:

```sh
tailscale status                     # node B must NOT appear
tailscale ping <node-B-100.x IP>     # must fail/time out
```

## Customer onboarding

Normal enrollments go through allternit-cloud-api (`POST
/api/v1/mesh/enroll`), which creates the `clerk-<id>` user and mints a
single-use, untagged, 24h preauth key. Manual onboarding for testing:

```sh
# 1. create the user (use the platform customer ID as the name)
fly ssh console -a allternit-headscale -C "headscale users create cust_abc123"

# 2. find the numeric user ID (preauthkeys create wants the ID, not the name)
fly ssh console -a allternit-headscale -C "headscale users list"

# 3. mint a preauth key for device enrollment
#    reusable: multiple devices (desktop + iOS) can enroll with one key
#    ephemeral: node auto-deletes when it goes offline (good for CI/VPS throwaway)
#    --expiration: how long the KEY stays valid for enrollment (not the node)
#    NO --tags: nodes must stay user-owned or the isolation policy cuts them off
fly ssh console -a allternit-headscale -C \
  "headscale preauthkeys create --user <numeric-user-id> --reusable --expiration 720h"

# one-shot ephemeral key variant:
fly ssh console -a allternit-headscale -C \
  "headscale preauthkeys create --user <numeric-user-id> --ephemeral --expiration 1h"

# list keys
fly ssh console -a allternit-headscale -C "headscale preauthkeys list"

# revoke a key before expiry (by numeric authkey ID from the list)
fly ssh console -a allternit-headscale -C "headscale preauthkeys expire --id <authkey-id>"
```

Recommended defaults for manual keys: `--reusable` with a short
`--expiration` (hours/days), minted just-in-time. The platform integration
(allternit-cloud-api) mints single-use, non-ephemeral 24h keys. Keys are
enrollment credentials — store them like secrets and expire them
aggressively.

## Node administration

```sh
# list all nodes (all users)
fly ssh console -a allternit-headscale -C "headscale nodes list"

# filter to one customer
fly ssh console -a allternit-headscale -C "headscale nodes list --user cust_abc123"

# delete a node (forces re-enrollment with a fresh key)
fly ssh console -a allternit-headscale -C "headscale nodes delete --identifier <node-id>"

# manually expire a node's session
fly ssh console -a allternit-headscale -C "headscale nodes expire --identifier <node-id>"
```

## How gizzi-code connects (desktop / VPS)

Classic tailscaled:

```sh
tailscale up --login-server https://allternit-headscale.fly.dev --auth-key <preauth-key>
```

Embedded tsnet (Go) — same control URL, key passed directly:

```go
srv := &tsnet.Server{
    Hostname:   "gizzi-" + deviceID,
    ControlURL: "https://allternit-headscale.fly.dev",
    AuthKey:    preauthKey, // minted by allternit-cloud-api via the CLI/API above
    Dir:        stateDir,   // persists node key across restarts
}
```

## How the iOS app connects (embedded tsnet)

Same shape: tsnet in the app binary with `ControlURL` set to
`https://allternit-headscale.fly.dev` and an `AuthKey` fetched from the
platform (allternit-cloud-api) at pair time. Persist tsnet state in the app's
container so the node keeps its 100.64.x.x identity across launches. iOS
nodes should use non-ephemeral keys (the node should survive app restarts);
desktop CI/VPS nodes can use ephemeral keys.

## Backups

All state lives on the `headscale_data` volume at `/var/lib/headscale`:
`db.sqlite` (+ WAL), `noise_private.key`. Losing the volume = every node must
re-enroll, so back it up.

- Fly keeps automatic daily volume snapshots (retention ~5 days):
  `fly volumes snapshots list vol_xxx -a allternit-headscale`.
- For a manual consistent dump, stop writes first (machine has no sqlite3 CLI;
  the image is minimal). Simplest safe path:

  ```sh
  fly machine stop <machine-id> -a allternit-headscale
  fly sftp -a allternit-headscale get /var/lib/headscale/db.sqlite ./backup/db-$(date +%F).sqlite
  fly sftp -a allternit-headscale get /var/lib/headscale/noise_private.key ./backup/
  fly machine start <machine-id> -a allternit-headscale
  ```

  Brief downtime during the copy; nodes keep working peer-to-peer meanwhile
  (headscale is coordination-only) but no new enrollments happen.

## Upgrades

1. Bump the tag in `Dockerfile` and verify `config.yaml` against that
   release's `config-example.yaml` (headscale changes config keys between
   minor versions — e.g. the 0.25 → 0.26 policy/DNS reshuffle).
2. Take a manual backup (above) — headscale runs DB migrations automatically
   on boot and they are one-way.
3. `fly deploy -a allternit-headscale`, then `fly logs` and
   `curl https://allternit-headscale.fly.dev/health`.

## Known follow-ups / not yet done

- ~~**ACLs / tagging**~~ **Done:** per-customer isolation is enforced by
  `policy.hujson` (`autogroup:member` → `autogroup:self:*`) over user-owned
  nodes — see "Isolation model" above. It takes effect on the next deploy;
  no node-side changes are needed (nodes receive the new packet filter with
  their next map update).
- **Custom domain:** `allternit-headscale.fly.dev` works but leaks the
  provider; consider `mesh.allternit.com` (`fly certs add`). If you change
  `server_url`, existing nodes keep working only until reauth — do it before
  customers enroll.
- **gRPC remote admin:** loopback-only for now; the platform integration will
  either shell out via `fly ssh` (as documented) or we expose gRPC with TLS
  client certs later.
