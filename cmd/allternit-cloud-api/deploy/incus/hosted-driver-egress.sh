#!/usr/bin/env bash
# Egress rules for hosted-driver computers (/v1/computers, outside developers).
# Run once per fleet host that takes hosted-driver computers, as root:
#   bash hosted-driver-egress.sh            # create or update
# It makes:
#   * nft table     inet allternit_drv_guard (+ unit allternit-drv-guard): the
#     bridge's computers may send the host only DHCP, and DNS to the bridge gateway, so no host
#     service (Postgres 5432, MinIO 9000/9001, allternit-api, SMTP 25, the
#     Incus API, VNC) is reachable from them on any host address; plus
#     DOCKER-USER accepts so Docker's FORWARD DROP doesn't cut them off.
#   * network ACL  allternit-hosted-driver-egress: reject egress to RFC1918,
#     100.64.0.0/10 (CGNAT, our Tailscale/mesh), 169.254.0.0/16 (link-local,
#     includes the 169.254.169.254 metadata address) and IPv6 ULA/link-local;
#     DNS to the bridge gateway stays allowed so name resolution works.
#   * managed bridge   allternit-drv0 with that ACL (ACLs need a managed bridge).
#   * profile          allternit-hosted-driver: eth0 on allternit-drv0.
# cloud-api picks the profile through ALLTERNIT_HOSTED_DRIVER_PROFILES
# (default "default,allternit-hosted-driver"). A host without the profile
# refuses to create hosted-driver computers, so nothing starts unfiltered.
set -euo pipefail
ACL=allternit-hosted-driver-egress
NET=allternit-drv0
PROFILE=allternit-hosted-driver
SUBNET=${HOSTED_DRIVER_SUBNET:-10.231.0.1/24}
GW=${SUBNET%/*}
# The host's own public addresses (its services listen there too: Postgres,
# MinIO, allternit-api, SMTP, the Incus API). Private ones are covered above.
HOST_IPS=${HOSTED_DRIVER_HOST_IPS:-$(ip -4 -o addr show scope global | awk '{split($4,a,"/"); print a[1]"/32"}' | paste -sd, -)}

incus network acl show "$ACL" >/dev/null 2>&1 || incus network acl create "$ACL"
cat <<YAML | incus network acl edit "$ACL"
description: Hosted-driver computers reach the public internet only
egress:
- {action: allow, destination: ${GW}/32, protocol: udp, destination_port: "53", state: enabled}
- {action: allow, destination: ${GW}/32, protocol: tcp, destination_port: "53", state: enabled}
- {action: reject, destination: "10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,100.64.0.0/10,169.254.0.0/16,127.0.0.0/8,0.0.0.0/8", state: enabled}
- {action: reject, destination: "fc00::/7,fe80::/10,::1/128", state: enabled}
- {action: reject, destination: "${HOST_IPS}", state: enabled}
- {action: allow, state: enabled}
ingress:
- {action: allow, state: enabled}
YAML

if ! incus network show "$NET" >/dev/null 2>&1; then
  incus network create "$NET" ipv4.address="$SUBNET" ipv4.nat=true ipv6.address=none
fi
incus network set "$NET" security.acls="$ACL" security.acls.default.egress.action=reject

incus profile show "$PROFILE" >/dev/null 2>&1 || incus profile create "$PROFILE"
incus profile device remove "$PROFILE" eth0 >/dev/null 2>&1 || true
incus profile device add "$PROFILE" eth0 nic network="$NET" name=eth0
# Defence in depth on top of the per-computer limits cloud-api sets
# (limits.cpu / limits.memory / limits.cpu.priority / root disk size).
incus profile set "$PROFILE" limits.processes=2000 limits.disk.priority=1 limits.memory.swap=false

# Host-bound traffic from the bridge (INPUT) and Docker's FORWARD DROP.
cat > /usr/local/sbin/allternit-drv-guard <<GUARD
#!/bin/sh
set -e
nft -f - <<NFT
table inet allternit_drv_guard
delete table inet allternit_drv_guard
table inet allternit_drv_guard {
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
iptables -N DOCKER-USER 2>/dev/null || true
iptables -C DOCKER-USER -o $NET -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || iptables -I DOCKER-USER -o $NET -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
iptables -C DOCKER-USER -i $NET -j ACCEPT 2>/dev/null || iptables -I DOCKER-USER -i $NET -j ACCEPT
GUARD
chmod 0755 /usr/local/sbin/allternit-drv-guard
cat > /etc/systemd/system/allternit-drv-guard.service <<UNIT
[Unit]
Description=Allternit developer computers: host guard + forward rules for $NET
After=docker.service incus.service network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/allternit-drv-guard

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable allternit-drv-guard.service >/dev/null 2>&1
systemctl restart allternit-drv-guard.service

echo "ok: $PROFILE -> $NET ($SUBNET) with ACL $ACL"
