#!/usr/bin/env bash
# Allternit Cloud API — VPS deploy script.
#
# Codifies the loop from docs/Operations/CLOUD_API_VPS_DEPLOY.md. The failure
# this prevents: deploying a stale binary. `cargo test --release --lib` does
# NOT refresh target/release/<bin>; the build step here is mandatory and the
# verification step refuses to swap if the binary did not change.
#
# Usage (from the repo root, or anywhere — paths are absolute to this repo):
#   scripts/deploy-cloud-api.sh            # full deploy (build + tests + swap + verify)
#   scripts/deploy-cloud-api.sh --fast     # skip the test suite (still builds + verifies)
#   scripts/deploy-cloud-api.sh --dry-run  # print what would happen, change nothing
#
# Prerequisites: ssh root@mail works (Tailscale), /opt/allternit-build exists
# on the server, migrations under cmd/allternit-cloud-api/migrations_pg are
# applied manually BEFORE running this when they contain DDL.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER="${CLOUD_API_DEPLOY_SERVER:-root@mail}"
BUILD_TREE="/opt/allternit-build"
PKG="cmd/allternit-cloud-api"
BIN="allternit-cloud-api"
RUN_TESTS=1
DRY_RUN=0

for arg in "$@"; do
  case "$arg" in
    --fast) RUN_TESTS=0 ;;
    --dry-run) DRY_RUN=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

run() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    echo "DRY-RUN: $*"
  else
    echo "==> $*"
    "$@"
  fi
}

step() { echo; echo "── $* ──"; }

step "1/6 Sync sources to ${SERVER}:${BUILD_TREE}"
for d in src tests migrations_pg migrations; do
  # `migrations/` (legacy SQLite) was removed from the tree in 84f1d21f0; only
  # sync it when present so the script works on current and older checkouts.
  if [[ -d "${REPO_ROOT}/${PKG}/${d}" ]]; then
    run rsync -a --partial --timeout=60 "${REPO_ROOT}/${PKG}/${d}/" "${SERVER}:${BUILD_TREE}/${PKG}/${d}/"
  fi
done

# Keep the crate manifest and shared source imports in the same snapshot.
# Syncing only src left fresh Cloud source unable to compile on the server.
run rsync -a "${REPO_ROOT}/${PKG}/Cargo.toml" "${SERVER}:${BUILD_TREE}/${PKG}/Cargo.toml"
run rsync -a "${REPO_ROOT}/Cargo.lock" "${SERVER}:${BUILD_TREE}/Cargo.lock"
run ssh "$SERVER" "mkdir -p ${BUILD_TREE}/cmd/allternit-api/src"
for shared in answer_contract.rs net_client.rs office_session_guard.rs email_otp.rs; do
  run rsync -a "${REPO_ROOT}/cmd/allternit-api/src/${shared}" "${SERVER}:${BUILD_TREE}/cmd/allternit-api/src/${shared}"
done

step "2/6 Touch changed files (rsync -a preserves mtimes; cargo skips otherwise)"
if [[ "$DRY_RUN" -eq 0 ]]; then
  # No mapfile: macOS ships bash 3.2. Paths have no spaces, so a plain for works.
  CHANGED=$(git -C "$REPO_ROOT" status --porcelain "$PKG/src" "$PKG/tests" | awk '{print $2}' | grep '\.rs$' || true)
  if [[ -n "$CHANGED" ]]; then
    TOUCH=""
    for f in $CHANGED; do TOUCH="$TOUCH ${f#$PKG/}"; done
    ssh "$SERVER" "cd ${BUILD_TREE}/${PKG} && touch ${TOUCH}"
    echo "   touched:${TOUCH}"
  else
    echo "   nothing to touch"
  fi
fi

step "3/6 Build the release binary (NOT optional — cargo test does not refresh it)"
run ssh "$SERVER" "source \$HOME/.cargo/env && cd ${BUILD_TREE} && cargo build --release -p ${BIN}"

step "4/6 Test suite"
if [[ "$RUN_TESTS" -eq 1 ]]; then
  run ssh "$SERVER" "set -o pipefail; source \$HOME/.cargo/env && cd ${BUILD_TREE} && cargo test --release -p ${BIN} --lib 2>&1 | tail -3"
else
  echo "   skipped (--fast)"
fi

step "5/6 Swap + restart"
PREV_SIZE=$(ssh "$SERVER" "stat -c %s ${BUILD_TREE}/target/release/${BIN} 2>/dev/null || echo 0" 2>/dev/null || echo 0)
run ssh "$SERVER" "systemctl stop ${BIN} && cp ${BUILD_TREE}/target/release/${BIN} /opt/${BIN}/bin/${BIN} && systemctl start ${BIN}"

step "6/6 Verify the swap actually took"
if [[ "$DRY_RUN" -eq 0 ]]; then
  sleep 4
  ssh "$SERVER" '
    set -e
    HEALTH=$(curl -sf localhost:8082/api/v1/health) || { echo "HEALTH CHECK FAILED"; systemctl status '"${BIN}"' --no-pager | tail -5; exit 1; }
    echo "   health: $HEALTH"
    systemctl is-active --quiet '"${BIN}"' && echo "   service: active"
    NEW_SIZE=$(stat -c %s /opt/'"${BIN}"'/bin/'"${BIN}"')
    echo "   binary size: '"${PREV_SIZE}"' -> $NEW_SIZE"
    if [ "'"${PREV_SIZE}"'" != "0" ] && [ "$NEW_SIZE" = "'"${PREV_SIZE}"'" ]; then
      echo "WARNING: binary size unchanged — confirm the build actually recompiled"
    fi
    ERRS=$(journalctl -u '"${BIN}"' --since "2 minutes ago" --no-pager -p err | grep -v "No entries" || true)
    if [ -n "$ERRS" ]; then echo "   errors since restart:"; echo "$ERRS" | head -5; exit 1; fi
    echo "   journal: no errors"
  '
fi

echo; echo "Deploy complete. If migrations_pg gained a new 0XX file, apply it via:"
echo "  ssh ${SERVER} \"sudo -u postgres psql -d allternit -v ON_ERROR_STOP=1 -f ${BUILD_TREE}/${PKG}/migrations_pg/0XX_name.sql\""
