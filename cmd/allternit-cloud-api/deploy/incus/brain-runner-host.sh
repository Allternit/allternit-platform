#!/usr/bin/env bash
# Host setup for Brain runners (plan step 1.3, PLAN-brain-runner-2026-11-14.md).
# Run as root on the Incus host that serves runners (D1: mail, host_mail).
#
#   bash brain-runner-host.sh [flags]              # DRY RUN: prints every command, changes nothing
#   bash brain-runner-host.sh --confirm [flags]    # does it
#   bash brain-runner-host.sh --refresh-acl [--confirm]   # only re-resolve the egress IPs
#
# What it sets up (each step skips itself if already done, so re-running is safe):
#   1. backup   incus config/profiles/networks/ACLs + firewall to /root/backups/brain-runner-<time>/
#   2. sysctl   /etc/sysctl.d/90-incus-inotify.conf (see the standby outage, 2026-11-12)
#   3. pool     btrfs pool "runner" (loop file, --pool-size, default 60GiB), or --reuse-cow
#   4. network  managed bridge runnerbr0 (10.232.0.1/24, no IPv6) + ACL allternit-brain-runner-egress
#   5. guard    nft table inet allternit_runner_guard (+ unit): runners may reach the host
#               only for DHCP and DNS on the bridge gateway
#   6. profile  brain-runner: 768MiB hard memory, no swap, 1 CPU, 256 processes, no nesting,
#               not privileged, eth0 on runnerbr0, root disk on the pool
#   7. image    optional: --image-tarball <path> imports it as alias allternit-brain-runner
#
# It never touches Postgres, cloud-api, allternit-api, MinIO, other pools/bridges/profiles,
# or any existing instance.
#
# EGRESS ALLOWLIST: Incus 6.0 network ACLs match IP addresses and CIDRs, not domain names
# (address sets arrived later). So the vendor domains are resolved when this script runs and the
# resulting IPs go into the ACL as /32 allow rules on tcp/443. Everything else is rejected, and
# nothing may connect in. The cost: vendor hosts behind CDNs change IPs, and a stale ACL fails
# CLOSED (a runner turn errors out, nothing leaks). Re-run with --refresh-acl --confirm when a
# vendor starts failing, and after any change to the domain list. (An nftables set fed by
# dnsmasq would track IPs live, but Incus owns dnsmasq for managed bridges and rewrites its
# config, so that route fights Incus on 6.0.0. A refresh by hand matches "no scheduled jobs".)
set -euo pipefail

CONFIRM=0
REFRESH_ONLY=0
REUSE_COW=0
POOL=${RUNNER_POOL:-runner}
POOL_SIZE=${RUNNER_POOL_SIZE:-60GiB}
NET=${RUNNER_NET:-runnerbr0}
SUBNET=${RUNNER_SUBNET:-10.232.0.1/24}
ACL=allternit-brain-runner-egress
PROFILE=brain-runner
IMAGE_ALIAS=${RUNNER_IMAGE_ALIAS:-allternit-brain-runner}
IMAGE_TARBALL=""
INOTIFY_FILE=/etc/sysctl.d/90-incus-inotify.conf

# FINDINGS-brain-runner-0.2.md, "Egress allowlist". codex and grok ignore HTTPS_PROXY, so a
# destination allowlist is the control. Keep in step with ALLTERNIT_RUNNER_VENDORS.
DOMAINS=(
  api.allternit.com
  api.anthropic.com claude.com platform.claude.com
  chatgpt.com auth.openai.com api.openai.com
  cli-chat-proxy.grok.com accounts.x.ai auth.x.ai api.x.ai
  api.kimi.com auth.kimi.ai auth.kimi.com telemetry-logs.kimi.com
)

usage() {
  cat <<USAGE
Usage: $0 [--confirm] [--pool-size SIZE | --reuse-cow] [--image-tarball PATH] [--refresh-acl]

  (no flags)            dry run: print every command and file, change nothing
  --confirm             execute
  --pool-size SIZE      loop-file size for the new btrfs pool "${POOL}" (default ${POOL_SIZE})
  --reuse-cow           use the existing btrfs pool "cow" instead of creating "${POOL}"
  --image-tarball PATH  also import the image built by build-brain-runner-image.sh
  --refresh-acl         only re-resolve the vendor domains and rewrite the egress ACL
  -h, --help            this text

Env: RUNNER_POOL RUNNER_POOL_SIZE RUNNER_NET RUNNER_SUBNET RUNNER_IMAGE_ALIAS
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --confirm) CONFIRM=1; shift ;;
    --pool-size) POOL_SIZE=${2:?--pool-size needs a value}; shift 2 ;;
    --reuse-cow) REUSE_COW=1; shift ;;
    --image-tarball) IMAGE_TARBALL=${2:?--image-tarball needs a path}; shift 2 ;;
    --refresh-acl) REFRESH_ONLY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
[ "$REUSE_COW" = 1 ] && POOL=cow
GW=${SUBNET%/*}

if [ "$CONFIRM" = 1 ]; then
  [ "$(id -u)" = 0 ] || { echo "ERROR: --confirm must run as root" >&2; exit 1; }
  command -v incus >/dev/null 2>&1 || { echo "ERROR: incus not found" >&2; exit 1; }
  echo "== EXECUTING (--confirm) =="
else
  echo "== DRY RUN: nothing is changed. Add --confirm to execute. =="
fi

# run CMD...: print, and execute only with --confirm.
run() {
  printf '+ %s\n' "$*"
  [ "$CONFIRM" = 1 ] || return 0
  "$@"
}
# put FILE MODE (content on stdin): print the file, write only with --confirm.
put() {
  local file=$1 mode=$2 content
  content=$(cat)
  printf '+ write %s (mode %s):\n' "$file" "$mode"
  printf '%s\n' "$content" | sed 's/^/    | /'
  if [ "$CONFIRM" = 1 ]; then
    printf '%s\n' "$content" > "$file"
    chmod "$mode" "$file"
  fi
}
# have KIND NAME: does it already exist? Without incus (dry run on another machine) = no.
have() {
  command -v incus >/dev/null 2>&1 || return 1
  case "$1" in
    pool) incus storage show "$2" >/dev/null 2>&1 ;;
    net) incus network show "$2" >/dev/null 2>&1 ;;
    acl) incus network acl show "$2" >/dev/null 2>&1 ;;
    profile) incus profile show "$2" >/dev/null 2>&1 ;;
    image) incus image show "$2" >/dev/null 2>&1 ;;
  esac
}
step() { printf '\n--- %s ---\n' "$*"; }

# resolve DOMAIN: IPv4 addresses, one per line. Several lookups catch rotating answers.
resolve() {
  local d=$1
  for _ in 1 2; do
    if command -v getent >/dev/null 2>&1; then getent ahostsv4 "$d" | awk '{print $1}'
    elif command -v dig >/dev/null 2>&1; then dig +short A "$d" | grep -E '^[0-9.]+$' || true
    elif command -v host >/dev/null 2>&1; then host -t A "$d" | awk '/has address/{print $4}'
    fi
    sleep 1
  done | sort -u
}

write_acl() {
  local ips=() d ip all
  for d in "${DOMAINS[@]}"; do
    while read -r ip; do [ -n "$ip" ] && ips+=("$ip/32"); done < <(resolve "$d")
  done
  if [ "${#ips[@]}" -eq 0 ]; then
    echo "ERROR: resolved no addresses for the allowlist (DNS down?). Refusing to write an empty ACL." >&2
    [ "$CONFIRM" = 1 ] && exit 1
    ips=("203.0.113.1/32")   # dry run only: documentation address as a stand-in
  fi
  all=$(printf '%s\n' "${ips[@]}" | sort -u | paste -sd, -)
  echo "  resolved $(printf '%s\n' "${ips[@]}" | sort -u | wc -l | tr -d ' ') addresses for ${#DOMAINS[@]} domains"
  have acl "$ACL" || run incus network acl create "$ACL"
  # DNS to the bridge gateway is allowed so names resolve (the gateway's dnsmasq does the lookups);
  # then 443 to the resolved vendor IPs; everything else falls to the network default: reject.
  # Ingress stays empty: with the network default below, nothing may connect in.
  put /root/.allternit-brain-runner-acl.yaml 0600 <<YAML
description: Brain runners may reach only vendor sign-in/API hosts and api.allternit.com on 443
egress:
- {action: allow, destination: ${GW}/32, protocol: udp, destination_port: "53", state: enabled}
- {action: allow, destination: ${GW}/32, protocol: tcp, destination_port: "53", state: enabled}
- {action: allow, destination: "${all}", protocol: tcp, destination_port: "443", state: enabled}
ingress: []
YAML
  if [ "$CONFIRM" = 1 ]; then
    printf '+ incus network acl edit %s < /root/.allternit-brain-runner-acl.yaml\n' "$ACL"
    incus network acl edit "$ACL" < /root/.allternit-brain-runner-acl.yaml
    rm -f /root/.allternit-brain-runner-acl.yaml
  else
    printf '+ incus network acl edit %s < /root/.allternit-brain-runner-acl.yaml\n' "$ACL"
  fi
}

if [ "$REFRESH_ONLY" = 1 ]; then
  step "refresh egress ACL only"
  write_acl
  echo; echo "Done (refresh). Running runners pick the new ACL up without a restart."
  exit 0
fi

step "1/7 backup (before any change)"
B=/root/backups/brain-runner-$(date -u +%Y%m%dT%H%M%SZ)
run mkdir -p "$B"
if [ "$CONFIRM" = 1 ]; then
  incus config show > "$B/incus-config.yaml"
  incus storage list -f yaml > "$B/incus-storage.yaml"
  incus network list -f yaml > "$B/incus-networks.yaml"
  incus profile list -f yaml > "$B/incus-profiles.yaml"
  incus network acl list -f yaml > "$B/incus-acls.yaml" 2>/dev/null || true
  iptables-save > "$B/iptables.rules" 2>/dev/null || true
  nft list ruleset > "$B/nft.ruleset" 2>/dev/null || true
  sysctl fs.inotify.max_user_instances fs.inotify.max_user_watches > "$B/inotify.txt"
  [ -f "$INOTIFY_FILE" ] && cp -p "$INOTIFY_FILE" "$B/" || true
  echo "backup: $B"
else
  echo "+ incus config|storage|network|profile|network acl show  > $B/*.yaml"
  echo "+ iptables-save, nft list ruleset, inotify sysctls, $INOTIFY_FILE  > $B/"
fi

step "2/7 inotify sysctl (persist)"
# Unprivileged containers share one mapped host uid, so their inotify instances count against one
# limit. On allternit-standby dnsmasq for incusbr0 died ("Too many open files") when the live value
# fell back to 128 and cloud computers lost their network (memory: standby-inotify-dnsmasq-outage,
# 2026-11-12; it recurred 11-13, the thing that resets it is still unknown).
put "$INOTIFY_FILE" 0644 <<'CONF'
# Allternit: raise inotify limits for Incus containers and their dnsmasq.
# Why: standby outage 2026-11-12, see memory standby-inotify-dnsmasq-outage.
# Something resets the live value to 128 after boot, so check it after reboots:
#   sysctl fs.inotify.max_user_instances
fs.inotify.max_user_instances = 8192
fs.inotify.max_user_watches = 1048576
CONF
run sysctl -p "$INOTIFY_FILE"

step "3/7 storage pool ${POOL} (btrfs)"
if [ "$REUSE_COW" = 1 ]; then
  if have pool cow; then echo "reusing existing pool cow"; else
    echo "ERROR: --reuse-cow but pool cow does not exist on this host" >&2
    [ "$CONFIRM" = 1 ] && exit 1
  fi
elif have pool "$POOL"; then
  echo "pool ${POOL} exists: skipped"
else
  run incus storage create "$POOL" btrfs size="$POOL_SIZE"
fi

step "4/7 network ${NET} + egress ACL ${ACL}"
write_acl
if have net "$NET"; then
  echo "network ${NET} exists: updating ACL settings only"
else
  run incus network create "$NET" ipv4.address="$SUBNET" ipv4.nat=true ipv6.address=none
fi
# Default action for anything no ACL rule matches: reject, both directions.
run incus network set "$NET" security.acls="$ACL" \
  security.acls.default.egress.action=reject security.acls.default.ingress.action=reject

step "5/7 host guard (runners reach the host only for DHCP + DNS on the gateway)"
put /usr/local/sbin/allternit-runner-guard 0755 <<GUARD
#!/bin/sh
set -e
nft -f - <<NFT
table inet allternit_runner_guard
delete table inet allternit_runner_guard
table inet allternit_runner_guard {
  chain input {
    type filter hook input priority -10; policy accept;
    iifname "$NET" udp dport 67 accept
    iifname "$NET" ip daddr $GW udp dport 53 accept
    iifname "$NET" ip daddr $GW tcp dport 53 accept
    iifname "$NET" ct state established,related accept
    iifname "$NET" drop
  }
}
NFT
GUARD
put /etc/systemd/system/allternit-runner-guard.service 0644 <<UNIT
[Unit]
Description=Allternit Brain runners: host guard for $NET
After=incus.service network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/allternit-runner-guard

[Install]
WantedBy=multi-user.target
UNIT
run systemctl daemon-reload
run systemctl enable --now allternit-runner-guard.service

step "6/7 profile ${PROFILE}"
have profile "$PROFILE" || run incus profile create "$PROFILE"
# 768 MiB per runner, hard (there is no swap on this host); one vCPU; 256 processes; no nesting, not
# privileged (D4: the runner is the first process, no systemd, so no nesting needed). The root disk
# has no size quota on purpose (D3): a quota on a CoW root counts the shared image blocks, so
# cloud-api puts the CLI homes on a per-runner 256 MiB custom volume at /home/runner instead.
run incus profile set "$PROFILE" limits.memory=768MiB limits.memory.enforce=hard \
  limits.memory.swap=false limits.cpu=1 limits.processes=256 \
  security.nesting=false security.privileged=false
# Devices are replaced, not appended, so a re-run converges (remove fails when absent: ignored).
for dev in root eth0; do
  printf '+ incus profile device remove %s %s (if present)\n' "$PROFILE" "$dev"
  [ "$CONFIRM" = 1 ] && { incus profile device remove "$PROFILE" "$dev" >/dev/null 2>&1 || true; }
done
run incus profile device add "$PROFILE" root disk pool="$POOL" path=/
run incus profile device add "$PROFILE" eth0 nic network="$NET" name=eth0

step "7/7 image"
if [ -n "$IMAGE_TARBALL" ]; then
  if have image "$IMAGE_ALIAS"; then
    echo "image alias ${IMAGE_ALIAS} exists: skipped (delete or use another alias to replace it)"
  else
    [ "$CONFIRM" = 1 ] && [ ! -f "$IMAGE_TARBALL" ] && { echo "ERROR: $IMAGE_TARBALL not found" >&2; exit 1; }
    run incus image import "$IMAGE_TARBALL" --alias "$IMAGE_ALIAS"
  fi
else
  echo "no --image-tarball given: import later with: incus image import <tarball> --alias ${IMAGE_ALIAS}"
fi

echo
if [ "$CONFIRM" = 1 ]; then
  echo "Done. cloud-api creates a runner with: incus launch ${IMAGE_ALIAS} <name> -p ${PROFILE} (no -p default)"
  echo "and sets raw.lxc 'lxc.init.cmd = /usr/local/bin/allternit-runner-init' plus the runner env."
else
  echo "Dry run finished. Re-run with --confirm to execute."
fi
