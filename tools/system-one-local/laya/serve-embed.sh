#!/usr/bin/env bash
# Serve local embeddings for the Allternit memory index (allternit-api reads
# ALLTERNIT_EMBED_URL, default http://127.0.0.1:7719). Runs alongside Laya
# (serve-laya.sh, port 7718) from the same venv; it does not change Laya.
#
#   tools/system-one-local/laya/serve-embed.sh            # install deps if needed, then serve
#
# Model: google/embeddinggemma-2 (Apache-2.0, text-only load), pinned revision below.
# Env: LAYA_HOME (shared venv root; default ~/Library/Application Support/Allternit/laya),
#      EMBED_PORT (7719), EMBED_MODEL / EMBED_REVISION (swap model), EMBED_DEVICE,
#      EMBED_DIM (768 default; 512/256/128 Matryoshka truncation).
# Install step: the Laya venv already has torch + fastapi + uvicorn (via laya[serve]);
# EmbeddingGemma 2 needs sentence-transformers>=6.1, transformers>=5.19, torchvision and pillow
# (its processor imports both even for text), which we
# `uv pip install` into it when missing or too old.
# Weights (~1.5 GB; the text path uses 270M params) download from Hugging Face on
# first start, then run locally.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LAYA_HOME="${LAYA_HOME:-$HOME/Library/Application Support/Allternit/laya}"
PY="$LAYA_HOME/.venv/bin/python"
if ! "$PY" -c "
import torch, torchvision, fastapi, uvicorn, transformers, sentence_transformers, PIL
from packaging.version import Version as V
assert V(transformers.__version__) >= V('5.19') and V(sentence_transformers.__version__) >= V('6.1')
" 2>/dev/null; then
  command -v uv >/dev/null || { echo "serve-embed: needs uv (https://docs.astral.sh/uv/)" >&2; exit 1; }
  [ -x "$PY" ] || uv venv -q --python 3.11 "$LAYA_HOME/.venv"
  uv pip install -q --python "$PY" torch torchvision "transformers>=5.19" "sentence-transformers>=6.1" pillow fastapi uvicorn packaging
fi
if [ -z "${EMBED_DEVICE:-}" ] && [ "$(uname -sm)" = "Darwin arm64" ]; then export EMBED_DEVICE=mps; fi
export EMBED_MODEL="${EMBED_MODEL:-google/embeddinggemma-2}"
export EMBED_REVISION="${EMBED_REVISION:-914f7f89142e33e77833254d9c9b90c3cef7303b}"
export EMBED_HOST=127.0.0.1 EMBED_PORT="${EMBED_PORT:-7719}"
exec "$PY" "$HERE/serve-embed.py"
