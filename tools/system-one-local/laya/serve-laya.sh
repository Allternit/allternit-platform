#!/usr/bin/env bash
# Install (first run) and serve Laya — convaiinnovations/laya, Apache-2.0 — as the
# local S1 backend "laya_bundled". System One (src/server.ts) forwards decisions
# with backend "laya_bundled" to it over the /v1/systemone protocol.
#
#   tools/system-one-local/laya/serve-laya.sh        # install if needed, then serve
#
# Env: LAYA_HOME (venv + log; default ~/Library/Application Support/Allternit/laya),
#      LAYA_PORT (7718), LAYA_DEVICE (mps on Apple silicon, else auto).
# Weights come from the public Hugging Face repo on first start (~600 MB), then
# inference is fully local.
set -euo pipefail
LAYA_VERSION="0.3.22"
LAYA_HOME="${LAYA_HOME:-$HOME/Library/Application Support/Allternit/laya}"
mkdir -p "$LAYA_HOME"
PY="$LAYA_HOME/.venv/bin/python"
if ! "$PY" -c "import laya, sys; sys.exit(0 if laya.__version__ == '$LAYA_VERSION' else 1)" 2>/dev/null; then
  command -v uv >/dev/null || { echo "serve-laya: needs uv (https://docs.astral.sh/uv/)" >&2; exit 1; }
  uv venv -q --python 3.11 "$LAYA_HOME/.venv"
  uv pip install -q --python "$PY" "laya[serve]==$LAYA_VERSION"
fi
if [ -z "${LAYA_DEVICE:-}" ] && [ "$(uname -sm)" = "Darwin arm64" ]; then export LAYA_DEVICE=mps; fi
export LAYA_HOST=127.0.0.1 LAYA_PORT="${LAYA_PORT:-7718}" LAYA_MODELS=typed-decisions LAYA_MAX_LOADED=1
exec "$PY" -m laya.serve
