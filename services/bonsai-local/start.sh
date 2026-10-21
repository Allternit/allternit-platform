#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="${ALLTERNIT_BONSAI_HOME:-$HOME/Library/Application Support/Allternit/bonsai-local}"
SOURCE="$ROOT/image-studio"
MODEL="$ROOT/models/bonsai-image-ternary-4B-mlx-2bit"

test -x "$SOURCE/.venv/bin/uvicorn" || { echo "Run $HERE/install.sh first." >&2; exit 1; }
test -d "$MODEL/transformer-packed-mflux" || { echo "Bonsai model is incomplete; rerun install.sh." >&2; exit 1; }

export PYTHONPATH="$HERE:$SOURCE"
export MFLUX_STUDIO_BAKED_MODEL_PATH="$MODEL"
export MFLUX_STUDIO_DEFAULT_BACKEND="bonsai-ternary-mlx"
exec "$SOURCE/.venv/bin/uvicorn" app:app --host 127.0.0.1 --port 8000
