#!/usr/bin/env bash
# A patched opentunnel-server on loopback for @allternit/share-tunnel's integration test (train only).
# Needs docker (MySQL 8), cargo and openssl. Prints the env the test reads; stop with `local-relay.sh stop`.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
work="${OT_WORK:-${TMPDIR:-/tmp}/allternit-opentunnel-test}"
if [ "${1:-}" = stop ]; then
  [ -f "$work/server.pid" ] && kill "$(cat "$work/server.pid")" 2>/dev/null || true
  docker rm -f allternit-ot-mysql >/dev/null 2>&1 || true
  exit 0
fi
mkdir -p "$work/site"
"$here/scripts/fetch.sh" "$work/opentunnel"
docker run -d --name allternit-ot-mysql --tmpfs /var/lib/mysql -e MYSQL_ROOT_PASSWORD=opentunnel \
  -e MYSQL_DATABASE=opentunnel -p 127.0.0.1:13306:3306 mysql:8.0 >/dev/null
for _ in $(seq 1 60); do
  docker exec allternit-ot-mysql mysqladmin -popentunnel ping >/dev/null 2>&1 && break
  sleep 1
done
(cd "$work/opentunnel" && cargo build --release -p opentunnel-server)
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout "$work/key.pem" \
  -out "$work/cert.pem" -days 7 -subj /CN=opentunnel.test -addext subjectAltName=DNS:opentunnel.test 2>/dev/null
echo ok > "$work/site/index.html"
secret="test-admission-$(openssl rand -hex 8)"
admin="test-admin-$(openssl rand -hex 8)"
env OPENTUNNEL_DOMAIN=opentunnel.test TLS_LISTEN=127.0.0.1:18443 HTTP_LISTEN=127.0.0.1:18080 HTTP_MODE=serve ADMIN_LISTEN=127.0.0.1:18081 \
  DATABASE_URL=mysql://root:opentunnel@127.0.0.1:13306/opentunnel TLS_CERT_FILE="$work/cert.pem" \
  TLS_KEY_FILE="$work/key.pem" WEBSITE_DIR="$work/site" ISSUER=local LOCAL_CA_FILE="$work/ca.pem" \
  TRUSTED_PROXY_RANGES=off MESH_DNS=off ADMISSION_SECRET="$secret" ADMIN_TOKEN="$admin" RUST_LOG=warn \
  "$work/opentunnel/target/release/opentunnel-server" >"$work/server.log" 2>&1 &
echo $! > "$work/server.pid"
for _ in $(seq 1 60); do
  curl -sf http://127.0.0.1:18080/health >/dev/null && break
  sleep 1
done
cat <<VARS
export OPENTUNNEL_TEST_API=http://127.0.0.1:18080
export OPENTUNNEL_TEST_TLS=127.0.0.1:18443
export OPENTUNNEL_TEST_DOMAIN=opentunnel.test
export OPENTUNNEL_TEST_CA_FILE=$work/ca.pem
export OPENTUNNEL_TEST_ADMISSION_SECRET=$secret
export OPENTUNNEL_TEST_ADMIN_TOKEN=$admin
export OPENTUNNEL_TEST_ADMIN_API=http://127.0.0.1:18081
VARS
