# Cloud phones (redroid)

Cloud Android phones that show up in Phone Link like a phone on a USB cable.
Each phone is one [redroid](https://github.com/remote-android/redroid-doc)
Docker container on a fleet host. The Desktop app reaches its adb port over the
mesh (Tailscale/Headscale) and runs the normal Phone Link Android setup on it.

**Status: off.** Nothing runs until an operator enables it. Two switches, both
default off:

1. `CLOUD_PHONES_ENABLED=1` in the cloud-api environment. Without it every
   `/api/v1/cloud-phones` route answers 503 "Cloud phones are not available yet".
2. `provisioned_hosts.phones_enabled = TRUE` on at least one host, with
   `phone_ssh_target`, `phone_mesh_ip` and `phone_slots` filled in.

**Pricing and Stripe are not set up.** There is no Stripe product or price for
cloud phones. Creating them, choosing a price, and opening phones to paying
customers all need Eoj's approval first. Until then the create route only
accepts admins (`ALLTERNIT_ADMIN_USER_IDS`) and rows carry `plan = 'preview'`.

## Why not an Incus container like cloud computers

Redroid needs the kernel `binder` driver and a `--privileged` container. Cloud
computers are unprivileged Incus containers, which cannot load kernel modules
or grant those devices. So a phone runs on the host's own Docker daemon.

## Files

| File | Runs on | What it does |
| --- | --- | --- |
| `setup-host.sh` | the host, as root, once | Installs `linux-modules-extra-$(uname -r)`, loads `binder_linux devices="binder,hwbinder,vndbinder"`, persists it in `/etc/modules-load.d` and `/etc/modprobe.d`, checks Docker, installs `run-phone.sh` to `/opt/allternit/cloud-phone/`. Idempotent. |
| `run-phone.sh` | the host, called by cloud-api over SSH | `create <name> <mesh-ip> <adb-port> <link-port> [memory-mb]`, `delete <name> [--purge]`, `status <name>`. One named container per phone, hard memory limit, data volume under `/var/lib/allternit-cloud-phones/<name>`, adb (container 5555) and the Phone Link app (container 8200) published on the mesh IP only. |

## Enabling a host (operator steps)

1. The host must be on the mesh (`tailscale up --auth-key ...`, same as
   `infrastructure/provisioned-instance/init.sh`) and run Ubuntu with Docker.
2. Copy this folder to the host and run `sudo ./setup-host.sh`.
3. Make sure the cloud-api can `ssh <phone_ssh_target>` without a prompt
   (key-based, `BatchMode=yes`). Where the cloud-api runs in a container,
   that container needs an `ssh` client, the key and a `known_hosts` entry
   for the host.
4. Set the host row:

   ```sql
   UPDATE provisioned_hosts
      SET phones_enabled = TRUE,
          phone_ssh_target = 'root@<host>',
          phone_mesh_ip = '<host mesh ip>',
          phone_slots = 4
    WHERE id = '<host id>';
   ```

5. Set `CLOUD_PHONES_ENABLED=1` on the cloud-api and restart it.

## Mesh access (needed per phone)

The production Headscale policy (`infrastructure/mesh/headscale/policy.hujson`)
has one rule, `autogroup:member → autogroup:self:*`: a device reaches only
devices owned by the same customer. The phone host is not owned by the
customer, so a customer's Desktop **cannot reach their cloud phone** until a
rule for it is in the policy. The policy is a static file baked into the
Headscale image (no live policy API is used), so this is an operator step:

1. After a phone is created, fetch the rules (admin only):

   ```sh
   curl -H "Authorization: Bearer <admin token>" \
     https://api.allternit.com/api/v1/cloud-phones/acl-fragment
   ```

   Each live phone gets exactly one rule, the narrowest the policy can say:
   the owner's mesh user to the host's mesh IP on that phone's two ports.

   ```json
   { "action": "accept", "src": ["clerk-user-2abc@"], "dst": ["100.64.0.9:5555,6555"] }
   ```

2. Add the rules to the `acls` list in `policy.hujson`, after the
   `autogroup:self` rule. Remove rules for deleted phones (the fragment only
   lists live ones, so replacing the cloud-phone block with it is enough).
3. Redeploy Headscale as in `infrastructure/mesh/headscale/OPS.md`, and
   check `headscale policy get`.

Mesh traffic is stateful, so the phone host needs no rule back to the user.
Nothing tags the host, so the isolation model in `policy.hujson` is unchanged.

## Network safety

- `run-phone.sh` refuses to bind adb to `0.0.0.0`, `::`, or any address
  outside 100.64.0.0/10 or the RFC 1918 ranges, and checks the address is
  assigned on the host.
- Ports: phone N on a host gets adb on `5555 + N` and Phone Link on
  `6555 + N`.
- redroid's adb has no key check, so anyone who can reach `<mesh-ip>:<port>`
  controls that phone. The per-phone ACL rule above is what limits a phone's
  ports to its owner's devices; no other customer has a rule to them.

## Resource notes

A redroid 14 phone idles around 1.5–2 GB of RAM. The default limit is 3072 MB
(`cloud_phones.memory_mb`). Size `phone_slots` so phones plus the host's other
work fit in memory.
