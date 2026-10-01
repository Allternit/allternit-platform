#!/usr/bin/env bash
# Install (first run) and serve Laya — convaiinnovations/laya, Apache-2.0 — as the
# local S1 backend "laya_bundled". System One (src/server.ts) forwards decisions
# with backend "laya_bundled" to it over the /v1/systemone protocol.
#
#   tools/system-one-local/laya/serve-laya.sh        # install if needed, then serve
#
# Env: LAYA_HOME (venv + log; default ~/Library/Application Support/Allternit/laya),
#      LAYA_PORT (7718), LAYA_DEVICE (mps on Apple silicon, else auto),
#      LAYA_REVISION (checkpoint revision; default the pinned base, Q29),
#      LAYA_CHECKPOINT_PATH (a local typed-decisions checkpoint dir; overrides the hub one),
#      UV (path to uv, for the first-run install).
# Weights come from the public Hugging Face repo on first start (~600 MB), then
# inference is fully local.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LAYA_VERSION="0.3.22"
# Q29: base checkpoint pinned (convaiinnovations/laya, typed-decisions) until our
# fine-tuned revision passes Q26. Every revision is a new backend identity.
LAYA_PINNED_REVISION="55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851"
export LAYA_HOME="${LAYA_HOME:-$HOME/Library/Application Support/Allternit/laya}"
mkdir -p "$LAYA_HOME"
PY="$LAYA_HOME/.venv/bin/python"
if ! "$PY" -c "import laya, sys; sys.exit(0 if laya.__version__ == '$LAYA_VERSION' else 1)" 2>/dev/null; then
  "$HERE/install-laya.sh"
fi
if [ -z "${LAYA_DEVICE:-}" ] && [ "$(uname -sm)" = "Darwin arm64" ]; then export LAYA_DEVICE=mps; fi
export LAYA_HOST=127.0.0.1 LAYA_PORT="${LAYA_PORT:-7718}" LAYA_MODELS=typed-decisions LAYA_MAX_LOADED=1
export LAYA_REVISION="${LAYA_REVISION:-$LAYA_PINNED_REVISION}"
if [ -n "${LAYA_CHECKPOINT_PATH:-}" ]; then
  [ -d "$LAYA_CHECKPOINT_PATH" ] || { echo "serve-laya: LAYA_CHECKPOINT_PATH is not a directory: $LAYA_CHECKPOINT_PATH" >&2; exit 1; }
  # A local checkpoint has no hub revision; point the typed-decisions slot at it.
  unset LAYA_REVISION
  exec "$PY" -c 'import os, laya.router as r, laya.serve as s; r.DEFAULT_MODELS["typed-decisions"] = (os.environ["LAYA_CHECKPOINT_PATH"], None); s.main()'
fi
exec "$PY" -m laya.serve
