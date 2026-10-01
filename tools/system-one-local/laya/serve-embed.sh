#!/usr/bin/env bash
# Serve local embeddings for the Allternit memory index (allternit-api reads
# ALLTERNIT_EMBED_URL, default http://127.0.0.1:7719). Runs alongside Laya
# (serve-laya.sh, port 7718) from the same venv; it does not change Laya.
#
#   tools/system-one-local/laya/serve-embed.sh            # install deps if needed, then serve
#
# Model: nomic-ai/modernbert-embed-base (Apache-2.0), pinned revision below.
# Env: LAYA_HOME (shared venv root; default ~/Library/Application Support/Allternit/laya),
#      EMBED_PORT (7719), EMBED_MODEL / EMBED_REVISION (swap model), EMBED_DEVICE.
# Install step: the Laya venv already has torch + transformers + fastapi + uvicorn
# (via laya[serve]); if any is missing we `uv pip install` exactly those into it.
# Weights (~600 MB) download from Hugging Face on first start, then run locally.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LAYA_HOME="${LAYA_HOME:-$HOME/Library/Application Support/Allternit/laya}"
PY="$LAYA_HOME/.venv/bin/python"
if ! "$PY" -c "import torch, transformers, fastapi, uvicorn" 2>/dev/null; then
  command -v uv >/dev/null || { echo "serve-embed: needs uv (https://docs.astral.sh/uv/)" >&2; exit 1; }
  [ -x "$PY" ] || uv venv -q --python 3.11 "$LAYA_HOME/.venv"
  uv pip install -q --python "$PY" torch "transformers>=4.48" fastapi uvicorn
fi
if [ -z "${EMBED_DEVICE:-}" ] && [ "$(uname -sm)" = "Darwin arm64" ]; then export EMBED_DEVICE=mps; fi
export EMBED_MODEL="${EMBED_MODEL:-nomic-ai/modernbert-embed-base}"
export EMBED_REVISION="${EMBED_REVISION:-d556a88e332558790b210f7bdbe87da2fa94a8d8}"
export EMBED_HOST=127.0.0.1 EMBED_PORT="${EMBED_PORT:-7719}"
exec "$PY" "$HERE/serve-embed.py"
