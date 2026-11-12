#!/usr/bin/env bash
# Install the Allternit decisions scorer on the cloud VPS. Run as root.
#   ./install.sh /path/to/decision-runtime   (the scp'd domains/decision-runtime dir)
# Idempotent: safe to re-run (updates code, keeps token and weights).
# Optional: ALLTERNIT_WEIGHTS_SHA256=<hex> verifies the GGUF after download.
set -euo pipefail

SRC="${1:-}"
[ -n "$SRC" ] && [ -f "$SRC/allternit_decisions/__main__.py" ] || { echo "usage: $0 <decision-runtime source dir>" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 2; }
HERE="$(cd "$(dirname "$0")" && pwd)"

USER_NAME=allternit-scorer
BASE=/opt/allternit-decision-scorer
STATE=/var/lib/allternit-decision-scorer
ENVF=/etc/allternit-decision-scorer.env
UNIT=allternit-decision-scorer.service
PORT=8020
GGUF="$STATE/models/unsloth--Qwen3.5-4B-GGUF/Qwen3.5-4B-Q4_K_M.gguf"

echo "==> user and directories"
id "$USER_NAME" >/dev/null 2>&1 || useradd --system --home-dir "$STATE" --shell /usr/sbin/nologin "$USER_NAME"
mkdir -p "$BASE/app" "$STATE"
chown "$USER_NAME:$USER_NAME" "$STATE"; chmod 700 "$STATE"

echo "==> code"
if command -v rsync >/dev/null; then
  rsync -a --delete --exclude '__pycache__' --exclude '*.pyc' "$SRC"/ "$BASE/app"/
else
  rm -rf "$BASE/app"; mkdir -p "$BASE/app"; cp -a "$SRC"/. "$BASE/app"/
fi
chown -R root:root "$BASE/app"; chmod -R go-w "$BASE/app"

echo "==> python venv"
if ! python3 -c 'import venv, ensurepip' 2>/dev/null; then
  apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3-venv python3-pip curl
fi
[ -x "$BASE/venv/bin/python" ] || python3 -m venv "$BASE/venv"
"$BASE/venv/bin/python" -m pip install --disable-pip-version-check -q --upgrade pip
# Prebuilt CPU wheels (the extra index in requirements-llamacpp.txt). The service's
# --prepare step installs the same pinned set into the state dir on first start.
"$BASE/venv/bin/python" -m pip install --disable-pip-version-check -q --only-binary=:all: \
  -r "$BASE/app/requirements-llamacpp.txt"
chown -R root:root "$BASE/venv"

echo "==> env file"
if [ ! -f "$ENVF" ]; then
  umask 077
  cat > "$ENVF" <<ENV
ALLTERNIT_DECISIONS_TOKEN=$(openssl rand -hex 32)
ALLTERNIT_DECISIONS_ENGINE=llamacpp
ALLTERNIT_DECISIONS_MODEL=qwen3.5-4b
HF_HOME=$STATE/hf
PIP_CACHE_DIR=$STATE/pip-cache
HF_HUB_DISABLE_TELEMETRY=1
# Caps llama.cpp threads on this shared host (engine default: cpu_count()-1).
ALLTERNIT_DECISIONS_THREADS=4
OMP_NUM_THREADS=4
ENV
  echo "created $ENVF (token generated, not shown)"
fi
chown root:root "$ENVF"; chmod 600 "$ENVF"

echo "==> unit"
install -m 644 "$HERE/$UNIT" "/etc/systemd/system/$UNIT"
systemctl daemon-reload
systemctl enable "$UNIT" >/dev/null
systemctl restart "$UNIT"

echo "==> waiting for ready (first run installs packages and downloads ~2.7 GB; up to 30 min)"
health() {
  # token goes through curl's config on stdin, never argv
  { printf 'header = "Authorization: Bearer %s"\n' "$(grep -m1 '^ALLTERNIT_DECISIONS_TOKEN=' "$ENVF" | cut -d= -f2-)"; } \
    | curl -fsS -m 5 -K - "http://127.0.0.1:$PORT/health" 2>/dev/null || true
}
for i in $(seq 1 600); do
  h="$(health)"
  case "$h" in
    *'"status": "ready"'*|*'"status":"ready"'*) echo "ready"; break ;;
    *'"status": "error"'*|*'"status":"error"'*) echo "scorer reported an error; see: journalctl -u $UNIT -n 80" >&2; exit 1 ;;
  esac
  systemctl is-active --quiet "$UNIT" || { echo "unit not active; see: journalctl -u $UNIT -n 80" >&2; exit 1; }
  [ $((i % 6)) = 0 ] && echo "  still preparing ($((i*5))s) $(printf '%s' "$h" | grep -o '"status": *"[a-z]*"' | head -1)"
  sleep 5
done
[ -n "${h:-}" ] && case "$h" in *ready*) ;; *) echo "timed out waiting for ready" >&2; exit 1;; esac

if [ -n "${ALLTERNIT_WEIGHTS_SHA256:-}" ] && [ -f "$GGUF" ]; then
  got="$(sha256sum "$GGUF" | cut -d' ' -f1)"
  [ "$got" = "$ALLTERNIT_WEIGHTS_SHA256" ] && echo "weights sha256 ok" || { echo "weights sha256 MISMATCH ($got)" >&2; exit 1; }
elif [ -f "$GGUF" ]; then
  echo "weights sha256 (record this): $(sha256sum "$GGUF" | cut -d' ' -f1)"
fi
echo "listening on 127.0.0.1:$PORT. Next: smoke.sh, then wire-node.sh --restart"
