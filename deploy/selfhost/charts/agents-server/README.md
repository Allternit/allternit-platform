# agents-server chart

Runs `allternit-agents-server`: the API, the agent loop (a gizzi child
process) and the link to your PostgreSQL. It makes no calls to
`*.allternit.com`. Sandbox workers are a separate chart (`../sandboxes`).

One replica by default. Set `replicaCount` higher to run several replicas on
the same PostgreSQL (the render fails if the server would use SQLite). The
Deployment rolls out with `RollingUpdate` (`maxUnavailable: 0`, `maxSurge: 1`):
the new pod is ready before the old one stops. Sessions are rebuilt from the
events in PostgreSQL on whichever replica serves them.

## What it creates

| Resource | Notes |
|---|---|
| Deployment | `replicaCount` replicas (default 1), RollingUpdate, non-root (uid 10001), read-only root filesystem, probes on `/health/live` and `/health/ready`. Each pod has its own data emptyDir (replica id, gizzi cache). |
| PodDisruptionBudget | Only when `replicaCount` > 1: `minAvailable: 1`. |
| Service | `http` on 8400; `metrics` on 9464 when `metrics.enabled`. |
| ConfigMap | The settings file, from `settings` in values. |
| Job (hook) | `admin migrate`, pre-install and pre-upgrade. The Deployment rolls out only after it succeeds. |
| ServiceMonitor | Optional (`metrics.serviceMonitor.enabled`). Needs the Prometheus Operator CRDs. |
| NetworkPolicy | Ingress from the sandbox workers and `ingressFrom`. Egress to DNS, the database, the provider and OTel only. |
| Secret | Only with `secret.create: true` (tests). Normally you create it. |

## You need

- Kubernetes 1.27 or later.
- PostgreSQL 16 that the cluster can reach. The chart does not install one.
- A Secret with `ALLTERNIT_DATABASE_URL`, `ALLTERNIT_ENCRYPTION_KEY`, your
  provider key (for example `ANTHROPIC_API_KEY`) and, for the first setup,
  `ALLTERNIT_SELF_HOSTED_SETUP_TOKEN`.
- Pull access to `ghcr.io/allternit/allternit-agents-server` (an
  `imagePullSecrets` entry), or the image in your own registry.

## Install

```sh
kubectl create namespace allternit
kubectl -n allternit create secret generic allternit-agents-env \
  --from-literal=ALLTERNIT_DATABASE_URL='postgres://allternit:PASSWORD@db.internal:5432/allternit' \
  --from-literal=ALLTERNIT_ENCRYPTION_KEY="$(openssl rand -base64 32)" \
  --from-literal=ANTHROPIC_API_KEY='...' \
  --from-literal=ALLTERNIT_SELF_HOSTED_SETUP_TOKEN="$(openssl rand -hex 32)"

cat > my-values.yaml <<'EOF'
secret:
  existingSecret: allternit-agents-env
settings:
  server:
    public_url: https://agents.example.internal
networkPolicy:
  database:
    to:
      - ipBlock: { cidr: 10.20.0.15/32 }
  provider:
    to:
      - ipBlock: { cidr: 203.0.113.0/24 }   # your provider's published ranges, or your egress proxy
EOF

helm install agents-server ./agents-server -n allternit -f my-values.yaml
kubectl -n allternit exec deploy/agents-server -- allternit-agents-server admin setup
```

`admin setup` prints the first admin API key (`ask_...`) once. Store it.

## Another provider

The values above use the default provider, `anthropic`. For that provider
only, the chart adds `credentials.api_key: env:ANTHROPIC_API_KEY` and a
`model_map` entry when you set none. For any other provider, set the provider,
its credentials (or `gateway`) and a `model_map` under `settings.inference`.
Nothing from the Anthropic defaults is added. For example, a gateway in
Anthropic Messages format on port 8080:

```yaml
settings:
  inference:
    provider: gateway
    gateway:
      format: anthropic-messages
      base_url: http://llm-gateway.example.internal:8080/v1
    model_map:
      claude-sonnet: sonnet-prod
networkPolicy:
  provider:
    ports: [8080]                    # default [443]
    to:
      - ipBlock: { cidr: 10.30.0.20/32 }
```

Leave `ANTHROPIC_API_KEY` out of the Secret in that case. The providers and
their credentials are listed in the self-hosted docs ("Models").

## Values that matter

| Key | Default | What it does |
|---|---|---|
| `settings` | see values.yaml | The settings file. The chart sets `server.listen` and `observability.prometheus` itself. |
| `replicaCount` | `1` | Server replicas. More than 1 needs PostgreSQL (`settings.database.url`); the render fails on SQLite. Adds a PodDisruptionBudget. See "Several replicas". |
| `settings.server.shutdown_grace` | `45s` | On SIGTERM, how long running turns get to finish before the replica fails them (session error code `server_restarted`). |
| `terminationGracePeriodSeconds` | empty | Empty means `shutdown_grace` + 15 s (60). A value may raise it; one below that fails the render. |
| `settings.sessions.idle_timeout` | `30s` | An idle session releases its worker after this long. `1h` keeps the worker and its `/workspace` between turns, as in 1.0. |
| `secret.existingSecret` | `""` | The Secret with the server's secret environment variables. |
| `secret.keys` | 4 names | Which Secret keys become environment variables. |
| `otel.endpoint` | `""` | OTLP endpoint. Empty exports nothing. |
| `metrics.enabled` | `false` | Opens the separate Prometheus listener on `metrics.port`. |
| `migrate.enabled` | `true` | The pre-install/pre-upgrade migration Job. |
| `networkPolicy.*` | on | See below. |

## NetworkPolicy

NetworkPolicy matches pods and IP ranges, not hostnames. Fill in
`networkPolicy.database.to` and `networkPolicy.provider.to`, or the server
cannot reach them (`helm install` prints a warning when they are empty).
`networkPolicy.provider.ports` defaults to `[443]`; change it when your
provider or gateway listens on another port. If
your provider has no stable ranges, send provider traffic through an egress
proxy and allow the proxy. The server also checks every outbound host against
its settings file. NetworkPolicy is a second layer.

The sandbox workers must be in the same namespace to be let in by default. For
anything else that calls the API (an ingress controller, other namespaces),
add it to `networkPolicy.ingressFrom`.

## Several replicas

```yaml
replicaCount: 3
```

- PostgreSQL only. The render fails when `settings.database.url` is unset
  (SQLite or memory, which are per pod).
- Replicas share all state through the database. Live events between
  replicas use `LISTEN/NOTIFY`, which needs a direct or session-mode
  connection in `ALLTERNIT_DATABASE_URL`. Through a transaction-mode pooler
  (for example PgBouncer in transaction mode) the replicas poll, and events of
  a turn on another replica arrive 1-5 s late.
- A turn runs on one replica. If that replica dies, the session ends with
  error code `server_restarted` within about a minute; the client sends the
  message again, on any replica.
- The data volume is an emptyDir per pod. Do not replace it with a shared
  ReadWriteMany volume: it holds the replica id file.
- `GET /v1/server/info` reports `replicas`.

## Upgrades

`helm upgrade` runs the migrate Job first, then rolls the pods one at a time:
each new pod is ready before an old one stops. A stopping pod gives running
turns `shutdown_grace` to finish. Back up PostgreSQL before an upgrade
(OPERATOR-GUIDE.md, "Backups").

### Upgrading from 1.0

- Migration 003 runs at the start of the upgrade (the migrate Job; `serve`
  also applies it at start). After it, a 1.0 pod cannot start against the
  database. A 1.0 pod that is still running keeps going until it stops.
- Rolling back to 1.0 needs a restore of the database backup taken before
  the upgrade. `helm rollback` alone is not enough.
- Turns that were running on the 1.0 pod when it stopped end with error code
  `server_restarted`.
- The default `sessions.idle_timeout` is now 30 s (was 1 h). Set `1h` to keep
  the old behaviour.
