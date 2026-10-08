#!/usr/bin/env bash
# Egress rules for hosted-driver computers (/v1/computers, outside developers).
# Run once per fleet host that takes hosted-driver computers, as root:
#   bash hosted-driver-egress.sh            # create or update
# It makes:
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

incus network acl show "$ACL" >/dev/null 2>&1 || incus network acl create "$ACL"
cat <<YAML | incus network acl edit "$ACL"
description: Hosted-driver computers reach the public internet only
egress:
- {action: allow, destination: ${GW}/32, protocol: udp, destination_port: "53", state: enabled}
- {action: allow, destination: ${GW}/32, protocol: tcp, destination_port: "53", state: enabled}
- {action: reject, destination: "10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,100.64.0.0/10,169.254.0.0/16,127.0.0.0/8,0.0.0.0/8", state: enabled}
- {action: reject, destination: "fc00::/7,fe80::/10,::1/128", state: enabled}
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
echo "ok: $PROFILE -> $NET ($SUBNET) with ACL $ACL"
