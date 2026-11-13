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

### Trying it on kind (test clusters only)

The default mode (`sandbox.userNamespace: true`) runs on kind, but a kind node
needs changes that a real node does not. Checked with kind v0.33.0,
Kubernetes 1.37.0 and containerd 2.3 on an Ubuntu 24.04 host (kernel 6.8).

1. kind bind-mounts two files over `/sys/devices/virtual/dmi/id/` in the node.
   The kernel then refuses a fresh sysfs mount in a pod user namespace, and
   every `hostUsers: false` pod fails with `error mounting "sysfs" ...
   operation not permitted`. Remove them after `kind create cluster`:

   ```sh
   docker exec <cluster>-control-plane sh -c \
     'umount /sys/devices/virtual/dmi/id/product_uuid; umount /sys/devices/virtual/dmi/id/product_uuid; umount /sys/devices/virtual/dmi/id/product_name'
   ```

2. Hosts with `kernel.apparmor_restrict_unprivileged_userns=1` (Ubuntu
   23.10+) also need the `Localhost` AppArmor profile in the node, and the kind
   node image has no AppArmor support (`apparmor is not supported`). Mount the
   host's securityfs in the node config:

   ```yaml
   nodes:
     - role: control-plane
       extraMounts:
         - hostPath: /sys/kernel/security
           containerPath: /sys/kernel/security
   ```

   Then load `deploy/selfhost/apparmor/allternit-selfhost-worker` on the host
   with `apparmor_parser -r`, and give containerd in the node an
   `/sbin/apparmor_parser` (it checks the file exists). Once containerd sees
   AppArmor, it also applies its own default profile,
   `cri-containerd.apparmor.d`, to every other pod. That profile must be loaded
   on the host too. Make the stub copy the profile file containerd passes it,
   then load that copy on the host:

   ```sh
   docker exec <cluster>-control-plane sh -c \
     'printf "#!/bin/sh\nfor a; do f=\$a; done\ncp \"\$f\" /root/cri-profile\n" > /sbin/apparmor_parser && chmod +x /sbin/apparmor_parser && systemctl restart containerd'
   # start any pod, then:
   docker cp <cluster>-control-plane:/root/cri-profile ./cri-containerd.apparmor.d
   sudo apparmor_parser -r ./cri-containerd.apparmor.d
   ```

   Unload both profiles (`apparmor_parser -R <file>`) when you delete the
   cluster. Without the AppArmor profile, bash in the sandbox fails with
   `bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted`.

3. Load the images with `kind load docker-image` and set
   `image.pullPolicy: Never` in both charts.

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
