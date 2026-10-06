#!/usr/bin/env bash
# Install the bot-email mail host on a Debian/Ubuntu box (allternit-standby).
# Run as root from services/mail-relay/deploy. Idempotent.
#   RELAY_SECRET=… WORKER_URL=https://allternit-agent-mail.allternitpbc.workers.dev ./install.sh
set -euo pipefail
: "${RELAY_SECRET:?set RELAY_SECRET (same value as MAIL_RELAY_SECRET on the mail worker)}"
: "${WORKER_URL:?set WORKER_URL (the agent mail worker)}"
HERE="$(cd "$(dirname "$0")" && pwd)"

DEBIAN_FRONTEND=noninteractive apt-get install -y postfix >/dev/null

# Certificate for SMTP TLS and the relay API (nginx already runs certbot here).
if [ ! -d /etc/letsencrypt/live/mx.allternit.com ]; then
  certbot certonly --nginx -d mx.allternit.com --non-interactive --agree-tos -m allternitpbc@gmail.com
fi
install -m 644 "$HERE/nginx-mx.allternit.com.conf" /etc/nginx/sites-available/mx.allternit.com
ln -sf /etc/nginx/sites-available/mx.allternit.com /etc/nginx/sites-enabled/mx.allternit.com
nginx -t && systemctl reload nginx

# Relay: secrets readable by root only, data dir owned by the container's node user (uid 1000).
install -d -m 700 /etc/allternit-mail-relay
umask 077
printf 'RELAY_SECRET=%s\nWORKER_URL=%s\nMX_HOST=mx.allternit.com\nDATA_DIR=/var/lib/allternit-mail-relay\n' "$RELAY_SECRET" "$WORKER_URL" > /etc/allternit-mail-relay/relay.env
install -d -m 700 -o 1000 -g 1000 /var/lib/allternit-mail-relay
(cd "$HERE" && docker compose up -d --build)

# Postfix: our main.cf, socketmap support, capped memory, reload.
install -m 644 "$HERE/postfix-main.cf" /etc/postfix/main.cf
mkdir -p /etc/systemd/system/postfix@-.service.d
printf '[Service]\nMemoryMax=512M\nCPUQuota=50%%\n' > /etc/systemd/system/postfix@-.service.d/limits.conf
systemctl daemon-reload
postfix check
systemctl enable --now postfix
systemctl restart postfix

# Inbound SMTP must be reachable.
iptables -C INPUT -p tcp --dport 25 -j ACCEPT 2>/dev/null || iptables -I INPUT -p tcp --dport 25 -j ACCEPT
echo "mail host ready: $(curl -s https://mx.allternit.com/relay/health)"
