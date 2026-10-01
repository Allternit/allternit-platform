#!/usr/bin/env bash
# Install Laya (convaiinnovations/laya, Apache-2.0) into $LAYA_HOME/.venv for the
# local S1 backend "laya_bundled". Idempotent: a matching install is a no-op, a
# broken or older one is rebuilt (that is "repair").
#
# Env: LAYA_HOME (default ~/Library/Application Support/Allternit/laya),
#      UV (path to uv; default: uv on PATH). uv is never bundled; without it this
#      prints "needs uv" and exits 3.
# Prints one progress line per step on stdout; Desktop streams them as progress.
set -euo pipefail
LAYA_VERSION="0.3.22"
LAYA_HOME="${LAYA_HOME:-$HOME/Library/Application Support/Allternit/laya}"
PY="$LAYA_HOME/.venv/bin/python"
mkdir -p "$LAYA_HOME"

if "$PY" -c "import laya, sys; sys.exit(0 if laya.__version__ == '$LAYA_VERSION' else 1)" 2>/dev/null; then
  echo "$LAYA_VERSION" > "$LAYA_HOME/laya.version"
  echo "laya $LAYA_VERSION already installed"
  exit 0
fi

UV_BIN="${UV:-$(command -v uv || true)}"
if [ -z "$UV_BIN" ] || [ ! -x "$UV_BIN" ]; then
  echo "needs uv: install it with 'brew install uv' or 'curl -LsSf https://astral.sh/uv/install.sh | sh'" >&2
  exit 3
fi

rm -f "$LAYA_HOME/laya.version"
echo "creating Python 3.11 environment"
"$UV_BIN" venv -q --allow-existing --python 3.11 "$LAYA_HOME/.venv"
echo "installing laya[serve]==$LAYA_VERSION"
"$UV_BIN" pip install -q --python "$PY" "laya[serve]==$LAYA_VERSION"
"$PY" -c "import laya, sys; sys.exit(0 if laya.__version__ == '$LAYA_VERSION' else 1)"
echo "$LAYA_VERSION" > "$LAYA_HOME/laya.version"
echo "laya $LAYA_VERSION installed"
