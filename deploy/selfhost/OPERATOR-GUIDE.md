# Allternit Self-Hosted Agents: operator guide

This guide is for the person who installs and runs the Agents Server. It
covers install, first run, workers, upgrades, backups, key rotation and
troubleshooting.

## What you run

| Part | What it does |
|---|---|
| `allternit-agents-server` | The API, the agent loop and the session state. It runs the gizzi CLI as a child process for each agent turn. Model calls go from here to the provider you configure. |
| PostgreSQL 16 | Agents, environments, sessions, events, the work queue, keys and encrypted credentials. |
| Workers (`gizzi worker`) | Poll the server over outbound HTTP(S), claim work for one environment, and run sandbox tools (bash, file edits, search) in `/workspace`. Nothing connects to a worker. |

The server makes no calls to `*.allternit.com`. It has no license check and
no Allternit account. Model calls go only to the provider in your settings
file. You can test this with the air-gap check (`compose/verify-airgap.sh`).

Version 1 runs one server replica. Sessions are rebuilt from the events in
PostgreSQL after a restart.

## Files in this package

| Path | What it is |
|---|---|
| `compose/docker-compose.yml` | PostgreSQL, a one-shot migrate container, the server and 10 workers. |
| `compose/.env.example` | The environment file for compose. Copy it to `compose/.env`. |
| `compose/compose.airgap.yml`, `compose/verify-airgap.sh` | The air-gap test. |
| `charts/agents-server`, `charts/sandboxes` | Helm charts for Kubernetes. Each has a README. |
| `settings.example.yaml` | An example settings file. The schema is `selfhost-settings.schema.json` in the server source. |

## 1. Install with Docker Compose

You need a Linux host with Docker Engine 24 or later and Docker Compose 2.24 or
later. A small test install (server, PostgreSQL, 10 mostly idle workers) runs
on 4 CPU cores and 8 GB of memory. Each worker container is capped at 1 CPU
and 2 GB, so size the host for the number of tool calls you expect at once.
The workers need Linux user namespaces (see "Workers").

The images are in a private registry. Log in with the read token you were
given (or load the images into your own registry and set
`ALLTERNIT_SERVER_IMAGE` and `ALLTERNIT_WORKER_IMAGE` in `.env`):

```sh
echo "$GHCR_TOKEN" | docker login ghcr.io -u <user> --password-stdin
cd compose
cp .env.example .env
chmod 600 .env
```

Fill in `.env`:

| Variable | How to make it |
|---|---|
| `ALLTERNIT_PG_PASSWORD` | `openssl rand -base64 24` |
| `ANTHROPIC_API_KEY` | Your provider key. For other providers, see "Settings". |
| `ALLTERNIT_SELF_HOSTED_SETUP_TOKEN` | `openssl rand -hex 32`. Used once by `admin setup`. |
| `ALLTERNIT_ENCRYPTION_KEY` | `openssl rand -base64 32`. Encrypts stored credentials. Back it up apart from the database. Without it, stored credentials cannot be read. |

Make your own settings file and point compose at it:

```sh
cp ../settings.example.yaml ../settings.yaml
echo 'ALLTERNIT_SETTINGS_FILE=../settings.yaml' >> .env
```

Start PostgreSQL, the migration and the server:

```sh
docker compose up -d
docker compose ps
curl -fsS http://127.0.0.1:8400/health/ready
```

The `migrate` container runs `admin migrate` and exits with code 0. The server
starts only after that. Running the migration again is safe. It only applies
what is missing.

The server's port is published on `127.0.0.1:8400` only. To reach it from other
machines, put a TLS reverse proxy in front of it (see "TLS").

### Settings

The settings file is YAML. Secrets never go in it. Use a reference instead:
`env:NAME`, `file:/path` or `vault:path#key`. The main sections:

- `inference`: the provider (`anthropic`, `bedrock`, `vertex`, `foundry` or
  `gateway`), its credentials, and `model_map`. When a model map is set, a model
  that is not listed is refused with `422 model_not_mapped`, and no request
  leaves the server. A `gateway` provider needs a model map.
- `mcp.allowed_hostnames`: MCP servers agents may use. Exact hostnames, HTTPS only.
- `git.allowed_hostnames` and `git.allowed_push_refs`: where workers may clone and push.
- `auth.oidc`: sign-in with your identity provider, with roles from a token claim.
- `data_dir`: a top-level key (default `/var/lib/allternit-agents-server`), not
  under `server`. Used for SQLite when no `database.url` is set.
- `secrets`: where secrets come from (`env`, `file` or `vault`), the encryption
  key reference (`encryption_key_ref`) and earlier keys (`previous_key_refs`).
  With Vault, `vault:path#key` reads `{mount}/data/{vault.path}/{path}`, and
  `vault:#key` reads `vault.path` itself. Vault auth is `token`, `approle` or
  `kubernetes`.
- `sessions`: `idle_timeout` (default 1h; after it the worker's item closes)
  and `no_worker_timeout` (default 10m; then a tool call fails with
  `no_worker`). A session shows `waiting_for_worker` while a tool call has no
  worker.
- `observability`: log format and the Prometheus listener (off by default;
  `127.0.0.1:9464`, never the main port).

OpenTelemetry is set with the standard `OTEL_*` environment variables, not in
the settings file: `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`
(default `allternit-agents-server`) and `OTEL_RESOURCE_ATTRIBUTES`. Span names:
`http.request`, `session`, `session.turn`, `tool_call`, `work.claim`,
`work.result`. Prometheus series (all `allternit_selfhost_`): `sessions`,
`turns_total`, `turn_duration_seconds`, `tool_calls_total`, `queue_depth`,
`workers_online`, `work_reclaims_total`, `leases_expired_total`,
`egress_requests_total`, `egress_refusals_total`, `store_up`. For a local check, set `COMPOSE_PROFILES=otel` and
`OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317` in `.env`, then run
`docker compose logs -f otel-collector`.

### Client origins (CORS)

Browser clients need their origin in `server.cors_origins`. The example lists:

| Origin | Client |
|---|---|
| `https://ai.allternit.com` | The web app. Desktop also uses it when it falls back to the hosted web app. |
| `https://m.allternit.com` | The phone app (PWA). |
| `http://127.0.0.1:8013` | Packaged Allternit Desktop. Its window is served by the local runtime on this origin. |

Remove the ones you do not use. The iOS app and the gizzi CLI do not need CORS.
The web app and the PWA can only connect to an `https://` server URL. The iOS
app is HTTPS only too: with a private CA, install the CA profile on the device
and trust it (Settings, General, About, Certificate Trust Settings).

The `bedrock-invoke` gateway format sends Bedrock Converse API paths
(`/model/{id}/converse`), not InvokeModel. Clients use the `model_map` keys as
model names; the server and gizzi map them to provider ids.

### TLS

Put a reverse proxy in front of `127.0.0.1:8400` and give it a certificate
your clients trust. Example with Caddy:

```
agents.example.internal {
    reverse_proxy 127.0.0.1:8400 {
        flush_interval -1    # keep the events stream (SSE) unbuffered
    }
}
```

Set `server.public_url` to the HTTPS URL. Event streams are long-lived. Set the
proxy's read timeout to at least 5 minutes, and turn off response buffering.

## 2. First run

Create the first admin API key. It is printed once:

```sh
docker compose exec server allternit-agents-server admin setup
```

Store the `ask_...` key in your secret manager, then remove
`ALLTERNIT_SELF_HOSTED_SETUP_TOKEN` from `.env` and run `docker compose up -d`.

Create an environment and an environment key for its workers:

```sh
export ALLTERNIT_API_KEY=ask_...
API=http://127.0.0.1:8400

curl -fsS -X POST $API/v1/environments \
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" -H 'content-type: application/json' \
  -d '{"name":"default","network":"none"}'
# note the "id": "env_..."

curl -fsS -X POST $API/v1/environments/env_.../keys \
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" -H 'content-type: application/json' \
  -d '{"name":"compose-workers"}'
# note the "secret": "aek_..." (shown once)
```

Or, from the server container:
`allternit-agents-server admin env-keys create --environment <env id> --config <path>`.
The admin CLI: `admin setup | migrate | keys list|create --role|revoke |
env-keys list|create --environment|revoke --environment | rotate-key |
doctor [--offline]`, all with `--config`.

The environment's `network` is `none` (no outbound traffic from the sandbox),
`limited` (only `allowed_hosts`, exact match) or `full`.

Run `docker compose exec server allternit-agents-server admin doctor` to check
the database, settings, provider configuration and keys.

## 3. Workers

Put the environment id and key in `.env` and turn on the workers profile:

```
ALLTERNIT_ENVIRONMENT_ID=env_...
ALLTERNIT_ENVIRONMENT_KEY=aek_...
COMPOSE_PROFILES=workers
```

```sh
docker compose up -d
curl -fsS -H "Authorization: Bearer $ALLTERNIT_API_KEY" \
  $API/v1/environments/env_.../workers
```

Ten workers start by default (`ALLTERNIT_WORKER_REPLICAS`). A worker shows as
`online` when the server has seen it in the last 60 seconds. One worker serves
one session at a time, so run as many workers as you want concurrent sessions.
Custom tools are run by SDK workers (the gizzi CLI worker answers them with an
error).

### The sandbox

Every bash call runs in a sandbox with the environment's network rules:
bubblewrap on Linux (plus `socat` for `limited` networking), `sandbox-exec` on
macOS. The
worker always turns the sandbox on and has no option to turn it off. If
bubblewrap cannot start, bash calls fail with an error. They do not fall back
to running without the sandbox.

bubblewrap runs without added privileges. The compose file gives the worker
containers these options, and nothing more:

```yaml
security_opt:
  - seccomp=unconfined     # allow unprivileged user namespaces
  - apparmor=unconfined    # allow the sandbox's mounts
  - systempaths=unconfined # allow a fresh /proc inside the sandbox
```

The host kernel must allow unprivileged user namespaces:

- Debian kernels: `sysctl kernel.unprivileged_userns_clone=1`
- Ubuntu 23.10 and later set `kernel.apparmor_restrict_unprivileged_userns=1`.
  Then bash fails in every worker (`bwrap: loopback: Failed RTM_NEWADDR:
  Operation not permitted`). Do not turn the restriction off for the whole
  host. Load the worker profile shipped in `deploy/selfhost/apparmor/` and
  point the workers at it:

  ```sh
  sudo install -m 0644 deploy/selfhost/apparmor/allternit-selfhost-worker /etc/apparmor.d/
  sudo apparmor_parser -r /etc/apparmor.d/allternit-selfhost-worker
  echo ALLTERNIT_WORKER_APPARMOR=allternit-selfhost-worker >> deploy/selfhost/compose/.env
  ```

  The profile allows what `apparmor=unconfined` allows, plus user namespaces,
  and applies only to containers started with it. `verify-airgap.sh` uses it
  when it is loaded and stops with this hint when it is not.

To check from a running worker:

```sh
docker compose exec worker bwrap --unshare-all --die-with-parent \
  --ro-bind / / --proc /proc --dev /dev true && echo ok
```

### Workers on other hosts

A worker needs only outbound access to the server URL. On a plain Linux host,
use the systemd unit from the gizzi package
(`cmd/gizzi-code/packaging/systemd/gizzi-selfhost-worker.service`) and its env
file, or run the worker image:

```sh
docker run -d --restart unless-stopped \
  --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
  --security-opt systempaths=unconfined \
  -e ALLTERNIT_SERVER_URL=https://agents.example.internal \
  -e ALLTERNIT_ENVIRONMENT_ID=env_... -e ALLTERNIT_ENVIRONMENT_KEY=aek_... \
  ghcr.io/allternit/allternit-selfhost-worker:<version>
```

Worker flags (add them after the image name): `--drain` (run one item, then
exit), `--block-ms`, `--reclaim-older-than-ms`, `--auto-stop SECS`,
`--on-work SCRIPT`. See `docs/SELFHOST_WORKER_PROTOCOL.md` in the server source.

On SIGTERM, a worker hands its item back and exits, and another worker takes it.
On a graceful server shutdown, open long-polls end at once.

## 4. Kubernetes

Use `charts/agents-server` for the server and `charts/sandboxes` for the
workers. Install both in the same namespace. Each chart's README covers values,
NetworkPolicy and the pod settings the sandbox needs. In short:

```sh
helm install agents-server charts/agents-server -n allternit -f server-values.yaml
kubectl -n allternit exec deploy/agents-server -- allternit-agents-server admin setup
# create an environment and key (section 2), store the key in a Secret
helm install sandboxes charts/sandboxes -n allternit -f worker-values.yaml
```

The chart runs the migration as a Job before each install and upgrade. You run
PostgreSQL yourself.

## 5. Upgrades

1. Read the release notes for the new version.
2. Back up the database (section 6).
3. Compose: set `ALLTERNIT_VERSION` in `.env` to the new version, then:

   ```sh
   docker compose pull
   docker compose up -d
   ```

   `migrate` runs first, then the server restarts, then the workers.
   Helm: `helm upgrade` with the new chart. The migrate Job runs first.

4. Check `curl -fsS $API/health/ready` and the worker list.

Keep the server and the workers on the same version. The single server is
down for a short time while it restarts. Clients reconnect, and running
sessions continue from their stored events.

To go back to the previous version, restore the backup you took in step 2,
then start the previous image. A migration may change the schema in a way the
older server cannot read.

## 6. Backups

PostgreSQL holds all state. Back it up with `pg_dump`:

```sh
docker compose exec -T postgres pg_dump -U allternit -d allternit --format=custom \
  > allternit-$(date +%Y%m%d-%H%M).dump
```

Restore into an empty database:

```sh
docker compose stop server worker
docker compose exec -T postgres dropdb -U allternit allternit
docker compose exec -T postgres createdb -U allternit allternit
docker compose exec -T postgres pg_restore -U allternit -d allternit --no-owner < allternit-YYYYMMDD-HHMM.dump
docker compose up -d
```

Also keep, apart from the database dumps:

- `ALLTERNIT_ENCRYPTION_KEY`. A restored database is of no use for stored
  credentials without the key that encrypted them.
- Your settings file and `.env` (or their Kubernetes Secrets and values).

Workers hold no state you need to back up. `/workspace` is scratch space for
the sessions running on that worker.

Test a restore on a separate host from time to time.

## 7. Key rotation

| Key | How to rotate |
|---|---|
| Server API keys (`ask_...`) | Create a new key (`POST /v1/api_keys`, admin role), switch your clients to it, then revoke the old one (`DELETE /v1/api_keys/{id}`). `admin keys` lists and revokes keys from the server container. |
| Environment keys (`aek_...`) | Create a new key for the environment, update the workers' `ALLTERNIT_ENVIRONMENT_KEY` (`.env` or the Kubernetes Secret), restart the workers, then revoke the old key (`DELETE /v1/environments/{id}/keys/{key_id}`). |
| Encryption key | Back up the database first. Add the new key, point `secrets.encryption_key_ref` at it and move the old key's reference into `secrets.previous_key_refs`. Stop the server, run `allternit-agents-server admin rotate-key --config <path>`, start the server, then remove the old key from `previous_key_refs` and your secret store. Keep the old key with the backups taken before the rotation. |
| Provider credentials | Change the value behind the reference (the env var, file or Vault secret) and restart the server. |
| Session secrets | Nothing to do. Each work claim gets a new one, and it ends with the lease. |

## 8. Troubleshooting

Start with:

```sh
docker compose ps
docker compose logs --tail 200 server
docker compose exec server allternit-agents-server admin doctor
```

Server logs are JSON lines on stderr.

| What you see | Likely cause and what to do |
|---|---|
| `migrate` exits with a non-zero code | The database is unreachable or the password is wrong. Check `ALLTERNIT_PG_PASSWORD` and `docker compose logs migrate`. The server does not start until the migration succeeds. |
| `/health/live` answers but `/health/ready` does not | The server is up but cannot use the database or its settings. The server log names the check that failed. |
| Workers missing from the worker list | Check `docker compose logs worker`. A 401 means the environment key is wrong or revoked. A 403 means the key belongs to another environment. A connection error means the worker cannot reach `ALLTERNIT_SERVER_URL`. |
| Session stays `waiting_for_worker` | No worker is online for that session's environment, or all are busy (one worker serves one session). Start more workers for that environment. After `sessions.no_worker_timeout` the call fails with `no_worker`. |
| bash calls fail with "no isolation driver" or bwrap "Operation not permitted" | The sandbox cannot start. Check the worker's security options and the host's user-namespace settings (section 3). |
| `422 model_not_mapped` | The agent's model is not a key in `inference.model_map`. Add it, or change the agent's model. |
| `host_not_allowed` | The host is not on the allowlist: `mcp.allowed_hostnames`, `git.allowed_hostnames`, or the environment's `allowed_hosts`. Lists are exact match. A subdomain does not match its parent. |
| `413 result_too_large` in worker logs | A tool's output was over 100 KiB. The worker truncates it and posts again. No action needed unless it repeats. |
| `409 lease_stale` in worker logs | Another worker took over the item after this worker's lease ran out (often after a long pause or a network break). The worker drops the item. Check for CPU starvation or network drops. |
| Browser shows a CORS error | Add the client's origin to `server.cors_origins` and restart the server. |
| The web app or PWA cannot connect | They need an `https://` server URL with a certificate the device trusts, and their origin in `cors_origins`. |
| Event stream drops every minute or so | A proxy in front of the server is timing out or buffering. Raise its read timeout and turn off buffering for `/v1/sessions/*/events/stream`. |

When you report a problem, include the server version (`GET /v1/server/info`),
the output of `admin doctor`, and the server and worker logs around the time
of the problem. Remove keys and secrets from anything you share.
