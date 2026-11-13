# agents-server chart

Runs `allternit-agents-server`: the API, the agent loop (a gizzi child
process) and the link to your PostgreSQL. It makes no calls to
`*.allternit.com`. Sandbox workers are a separate chart (`../sandboxes`).

Version 1 runs one replica. The Deployment uses the `Recreate` strategy, so an
upgrade stops the old pod before the new one starts. Sessions are rebuilt from
the events in PostgreSQL after a restart.

## What it creates

| Resource | Notes |
|---|---|
| Deployment | 1 replica, non-root (uid 10001), read-only root filesystem, probes on `/health/live` and `/health/ready`. |
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

## Values that matter

| Key | Default | What it does |
|---|---|---|
| `settings` | see values.yaml | The settings file. The chart sets `server.listen` and `observability.prometheus` itself. |
| `secret.existingSecret` | `""` | The Secret with the server's secret environment variables. |
| `secret.keys` | 4 names | Which Secret keys become environment variables. |
| `otel.endpoint` | `""` | OTLP endpoint. Empty exports nothing. |
| `metrics.enabled` | `false` | Opens the separate Prometheus listener on `metrics.port`. |
| `migrate.enabled` | `true` | The pre-install/pre-upgrade migration Job. |
| `networkPolicy.*` | on | See below. |

## NetworkPolicy

NetworkPolicy matches pods and IP ranges, not hostnames. Fill in
`networkPolicy.database.to` and `networkPolicy.provider.to`, or the server
cannot reach them (`helm install` prints a warning when they are empty). If
your provider has no stable ranges, send provider traffic through an egress
proxy and allow the proxy. The server also checks every outbound host against
its settings file. NetworkPolicy is a second layer.

The sandbox workers must be in the same namespace to be let in by default. For
anything else that calls the API (an ingress controller, other namespaces),
add it to `networkPolicy.ingressFrom`.

## Upgrades

`helm upgrade` runs the migrate Job first, then replaces the pod. Expect a
short API outage while the single pod restarts. Sessions in flight pick up
from their stored events. Back up PostgreSQL before an upgrade
(OPERATOR-GUIDE.md, "Backups").
