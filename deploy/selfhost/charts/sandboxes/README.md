# sandboxes chart

Runs `gizzi worker` pods for Allternit Self-Hosted Agents. A worker polls the
Agents Server over outbound HTTP(S), claims work for one environment and runs
sandbox tools (bash, file edits, search) in `/workspace`. Nothing connects to
a worker.

Each entry under `environments` gets its own Deployment, an optional HPA and a
NetworkPolicy that matches the environment's network mode.

## You need

- The agents-server chart installed in the same namespace, and its admin API key.
- For each environment: its id and an environment key (`aek_...`).
- Nodes that can run bubblewrap in a pod (see "Sandbox" below).

## Install

Create the environment and its key on the server first:

```sh
export ALLTERNIT_API_KEY=ask_...          # from admin setup
kubectl -n allternit port-forward svc/agents-server 8400:8400 &
curl -fsS -X POST http://127.0.0.1:8400/v1/environments \
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" -H 'content-type: application/json' \
  -d '{"name":"default","network":"none"}'
# -> {"id":"env_...", ...}
curl -fsS -X POST http://127.0.0.1:8400/v1/environments/env_.../keys \
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" -H 'content-type: application/json' \
  -d '{"name":"k8s-workers"}'
# -> {"secret":"aek_...", ...}   shown once
```

Or, inside the server pod: `allternit-agents-server admin env-keys create <env id>`.

```sh
kubectl -n allternit create secret generic allternit-env-default \
  --from-literal=ALLTERNIT_ENVIRONMENT_KEY=aek_...

cat > workers.yaml <<'EOF'
server:
  url: http://agents-server.allternit.svc:8400
environments:
  - name: default
    id: env_...
    keySecret: { name: allternit-env-default }
    network: none
    replicas: 4
EOF

helm install sandboxes ./sandboxes -n allternit -f workers.yaml
```

## Network modes

Set `network` to the same value as the environment on the server.

| Mode | The worker (per host, inside the sandbox) | This chart's NetworkPolicy (per IP, whole pod) |
|---|---|---|
| `none` | Blocks all sandbox traffic. | DNS and the server only. |
| `limited` | Allows only `allowed_hosts`, exact match, through its CONNECT proxy. | DNS, the server and the CIDRs in `allowedEgress`. |
| `full` | Allows any host. | DNS, the server and `0.0.0.0/0` minus `fullEgressExcept`. |

NetworkPolicy cannot match hostnames. For `limited`, list the addresses behind
your `allowed_hosts` in `allowedEgress`. The worker's own per-host check is
the precise one. The policy keeps the pod inside the same bounds if the worker
process is ever bypassed.

## Sandbox

Every bash call runs in bubblewrap. `gizzi worker` always turns the sandbox
on and has no flag to turn it off. Without a working bwrap, the bash tool
fails closed with an error. bwrap runs unprivileged, so the pod needs:

| Need | Default in values.yaml |
|---|---|
| Unprivileged user namespaces (seccomp and AppArmor must allow `unshare` and `mount`) | `sandbox.seccompProfile.type: Unconfined`, `sandbox.appArmorProfile.type: Unconfined` |
| A fresh `/proc` mount inside the sandbox | `procMount: Unmasked`, which needs `hostUsers: false` (`sandbox.userNamespace: true`) |
| Node support for pod user namespaces | Kubernetes 1.30+, Linux 6.3+, containerd 2.0+ or CRI-O 1.25+; the `UserNamespacesSupport` and `ProcMountType` feature gates where they are off by default |
| Kernel settings on the nodes | `kernel.unprivileged_userns_clone=1` (Debian); on Ubuntu 23.10+ load `deploy/selfhost/apparmor/allternit-selfhost-worker` on each node and set `sandbox.appArmorProfile` to `{type: Localhost, localhostProfile: allternit-selfhost-worker}` |

The container drops all capabilities and sets `allowPrivilegeEscalation: false`.

If your nodes cannot do pod user namespaces, set `sandbox.userNamespace:
false` and `sandbox.privileged: true`. That runs the worker privileged, which
gives the pod broad access to its node. If you do that, put the workers on
dedicated nodes (`nodeSelector`, `tolerations`) or a sandboxed runtime
(`runtimeClassName: gvisor` or `kata`).

Check the sandbox in a running pod:

```sh
kubectl -n allternit exec deploy/sandboxes-default -- \
  bwrap --unshare-all --die-with-parent --ro-bind / / --proc /proc --dev /dev true && echo ok
```

## Scaling

`replicas` sets a fixed count. With `autoscaling.enabled`, an HPA scales on CPU
by default. Workers spend most of their time waiting on a long-poll, so CPU is
a rough signal. To scale on queued work, read `queue_depth` from
`GET /v1/environments/{id}` into an external metric (for example with KEDA)
and list it under `autoscaling.metrics`.

On scale-down, a worker gets SIGTERM, hands its item back (`/release`) and
exits. Another worker takes the item at once.

## Values that matter

| Key | Default | What it does |
|---|---|---|
| `server.url` | required | The server URL as the workers reach it. |
| `environments[]` | `[]` | One worker group per environment: `name`, `id`, `keySecret`, `network`, `allowedEgress`, `replicas`, `autoscaling`. |
| `defaults.*` | see values.yaml | Defaults for every environment: `blockMs`, `reclaimOlderThanMs`, `autoStop`, `workspace`, `resources`. |
| `sandbox.*` | user namespace on | See "Sandbox". |
| `fullEgressExcept` | private ranges | What a `full` environment still cannot reach. |
