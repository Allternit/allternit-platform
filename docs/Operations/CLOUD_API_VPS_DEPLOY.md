# Allternit Cloud API — VPS Deploy Runbook

**Server:** `mail` (Contabo control plane, api.allternit.com) via `ssh root@mail` (Tailscale SSH).
**Binary:** `/opt/allternit-cloud-api/bin/allternit-cloud-api`, systemd unit `allternit-cloud-api`, port 8082 (nginx fronts 443 → 8082).
**Build tree:** `/opt/allternit-build` (full repo checkout — NOT `/opt/allternit-src`, stale Fly-era).
**Env:** `/opt/allternit-cloud-api/.env` (chmod 600). Prod DB: `sudo -u postgres psql -d allternit`.

## Deploy loop

**Prefer the codified script: `scripts/deploy-cloud-api.sh`** (from the repo root; `--fast` skips tests, `--dry-run` prints steps). It runs the loop below and *verifies the swap* — the check that catches a stale binary. The manual steps remain for reference:

1. `rsync -a --partial --timeout=60 cmd/allternit-cloud-api/{src,tests,migrations_pg}/ root@mail:/opt/allternit-build/cmd/allternit-cloud-api/...` (mirror each dir; touch any changed `.rs` files on mail — rsync `-a` preserves mtimes and cargo skips otherwise).
2. Apply schema manually — there is no migration runner:
   `ssh root@mail "sudo -u postgres psql -d allternit -v ON_ERROR_STOP=1 -f /opt/allternit-build/cmd/allternit-cloud-api/migrations_pg/00X.sql"`.
   After any `ALTER TYPE` (or on sqlx cached-plan errors) restart the service.
3. **Build the binary — `cargo test --release --lib` does NOT refresh it** (2026-09-03 incident: three consecutive deploys copied a stale 07:26 binary because the loop skipped the bin build; the guards/pricing fix/broker were not actually live for hours). Required, in order:
   ```bash
   cargo build --release -p allternit-cloud-api      # refreshes target/release/allternit-cloud-api
   cargo test --release -p allternit-cloud-api --lib # suite (known local-only failure: contabo_runtime_service provision test, no docker on mac; passes on mail)
   ```
4. Swap + restart:
   `systemctl stop allternit-cloud-api && cp target/release/allternit-cloud-api /opt/allternit-cloud-api/bin/ && systemctl start allternit-cloud-api`
5. **Verify the swap actually took** (do not skip — this is what catches a stale binary):
   - `ls -la target/release/allternit-cloud-api` — mtime must be now, size should track recent builds.
   - `curl -s localhost:8082/api/v1/health` → `{"status":"healthy"}`.
   - Check for the new feature's startup log line (e.g. `Inference pools seeded: ...`).
   - New DB objects present when applicable (e.g. `SELECT * FROM inference_pools;`).
   - `journalctl -u allternit-cloud-api --since "2 min ago" -p err` — no new errors.

## Adjacent operational facts

- Builds take 2–4 min; run over ssh in a background task with no timeout. Long test runs: `nohup` on the server + poll journal.
- Standby VPS `allternit-standby` (31.220.95.165) uses `~/.ssh/id_ed25519_gizziio` (Tailscale SSH disabled there); PG streaming replica — schema changes replicate automatically, apply migrations on `mail` only. Failover: `docs/Operations/FAILOVER_RUNBOOK.md`.
- Billing smoke tests: webhook signature recipe + credit_transactions FK notes in the session handoff; smoke users must be inserted into `users` first (CASCADE cleanup).
- Reconciliation timer: `reconcile-billing.timer` (daily, /usr/local/bin + /etc/systemd/system, files in `infrastructure/cloud/`).
- P1 agent-sessions namespace (2026-09-04): deploys that include `routes/agent_sessions.rs` must apply `migrations_pg/012_data_plane_nodes.sql` (step 2 above) — it extends `runtime_devices` with `kind`/capacity metadata the node resolver reads. The migration is additive with defaults; existing devices keep working as `kind='paired'` if it is not applied yet (the new handlers answer 428 "pair a device" only when no healthy online device exists).

## Dev-token backdoor gate — `ALLTERNIT_ALLOW_DEV_TOKEN`

Audit finding B1 (2026-09-03): the cloud API historically honored a hardcoded
`dev-api-token` bearer token at every auth surface (legacy auth layer, DB
token validator, WebSocket validator, token-info route, dispatch handoff) —
a production backdoor. The token is now **gated**:

- Honored **only** when `ALLTERNIT_ALLOW_DEV_TOKEN=true` (or `=1`) is present
  in `/opt/allternit-cloud-api/.env`. **Default OFF** — production rejects the
  token. Template: `cmd/allternit-cloud-api/.env.example`.
- The service logs a **warn-level startup line** whenever the gate is open, so
  it can never be enabled silently. After any deploy, verify absence with:
  `journalctl -u allternit-cloud-api --since "2 min ago" | grep -i ALLOW_DEV_TOKEN`
  (no output = gate off = good).
- The token literal still exists in the binary (gating only, not removal) —
  the iOS app in the field still presents it. **Coordinated-removal sequence:**
  1. Ship an iOS build that no longer sends the dev token (the
     `-skip-auth` DEBUG shim now sources it from
     `ALLTERNIT_DEV_API_TOKEN` in Info.plist, empty in Release).
  2. Wait until no production traffic relies on the token.
  3. Only then remove the literal from `auth/dev_token.rs` (separate change).

Until step 3, never set `ALLTERNIT_ALLOW_DEV_TOKEN=true` on `mail`.

## CI/CD (GitHub Actions → Contabo control plane)

Workflow: `.github/workflows/deploy-cloud-api-contabo.yml`. On every push to
`main` touching the cloud API (plus `workflow_dispatch`):

1. **test** job — Postgres 16 service container (the lib suite connects to
   `allternit_test` on localhost), then `cargo test --release -p
   allternit-cloud-api --lib` for the full target triple. Docker is available
   on the runner, so the contabo provision test runs too.
2. **deploy** job (needs: test) — cross-builds the x86_64 release binary,
   joins the Tailscale network, and swaps it onto `mail`
   (100.108.37.126) via `cmd/allternit-cloud-api/deploy-contabo.sh`, which
   health-checks `/api/v1/health` post-swap and **rolls back automatically**
   to the previous binary on failure.

No public SSH exposure: the runner only reaches mail over the tailnet.

### One-time setup (owner actions)

1. **Tailscale OAuth client** — https://login.tailscale.com/admin/settings/oauth →
   *Generate OAuth client* → scope **Auth keys / Write**, tag `tag:ci`.
   (The action mints a short-lived auth key per run from this client; the
   client secret is shown once — copy both values.)
2. **ACL policy** — the tag must exist and be allowed to reach (and, unless
   using an SSH key, SSH into) mail:
   ```json
   "tagOwners": { "tag:ci": ["allternitpbc@"] },
   "acls": [
     { "action": "accept", "src": ["tag:ci"], "dst": ["tag:mail:*"] }
   ],
   "ssh": [
     { "action": "check", "src": ["tag:ci"], "dst": ["tag:mail"], "users": ["root"] }
   ]
   ```
   (adjust the mail host tag to whatever `mail` actually carries; `tailscale status`
   shows tags with `tailscale status --json`). If you skip the `ssh` block,
   also generate an SSH keypair for root@mail and set `CONTABO_SSH_KEY` below.
3. **GitHub secrets** (repo `Allternit/allternit-platform`):
   ```bash
   gh secret set TS_OAUTH_CLIENT_ID      # from step 1
   gh secret set TS_OAUTH_CLIENT_SECRET  # from step 1, shown once
   gh secret set CONTABO_SSH_KEY         # optional, only without the ssh ACL
   ```
4. Sanity-check once from any machine on the tailnet:
   `ssh root@mail curl -s localhost:8082/api/v1/health`.

After that, merging to `main` deploys itself. Watch runs with
`gh run list --workflow=deploy-cloud-api-contabo.yml`.
