#!/usr/bin/env bash
set -euo pipefail

SOURCE_REV="31b02171634c16b5da0eec6aea075e7489d5fb39"
MODEL_REV="2c24c81b934a658ba5590cf39088ba929985b4a8"
MODEL_REPO="prism-ml/bonsai-image-ternary-4B-mlx-2bit"
MLX_WHEEL_VERSION="0.31.1"
ROOT="${ALLTERNIT_BONSAI_HOME:-$HOME/Library/Application Support/Allternit/bonsai-local}"
SOURCE="$ROOT/image-studio"
MODEL="$ROOT/models/bonsai-image-ternary-4B-mlx-2bit"
export UV_CACHE_DIR="$ROOT/.uv-cache"

[[ "$(uname -m)" == "arm64" ]] || { echo "Bonsai MLX requires Apple Silicon." >&2; exit 1; }
command -v git >/dev/null || { echo "git is required." >&2; exit 1; }
command -v uv >/dev/null || { echo "uv is required: https://docs.astral.sh/uv/" >&2; exit 1; }

available_kb="$(df -Pk "$HOME" | awk 'NR==2 {print $4}')"
(( available_kb >= 8388608 )) || { echo "At least 8 GiB of free disk space is required." >&2; exit 1; }

mkdir -p "$ROOT/models"
if [[ ! -d "$SOURCE/.git" ]]; then
  git clone --filter=blob:none https://github.com/PrismML-Eng/image-studio.git "$SOURCE"
fi
git -C "$SOURCE" fetch --depth 1 origin "$SOURCE_REV"
git -C "$SOURCE" checkout --detach "$SOURCE_REV"
[[ "$(git -C "$SOURCE" rev-parse HEAD)" == "$SOURCE_REV" ]]

# Upstream pins mlx to a PrismML git fork that must be compiled from source
# (requires full Xcode + Metal Toolchain). The fork differs from upstream by a
# single guard in the 1-bit Metal fast path (PrismML-Eng/mlx@b9effaf). Bonsai
# ternary is 2-bit and upstream mlx 0.31.x has no 1-bit path at all, so the
# official PyPI arm64 wheel is equivalent for this model — verified end-to-end
# (deterministic seed-42 PNG generation) on 2026-07-15.
python3 - "$SOURCE/pyproject.toml" "$MLX_WHEEL_VERSION" <<'EOF'
import sys
from pathlib import Path

path, wheel = Path(sys.argv[1]), sys.argv[2]
text = path.read_text()
text = text.replace('    "mlx",\n', f'    "mlx=={wheel}",\n')
text = '\n'.join(
    line for line in text.split('\n')
    if not (line.startswith('mlx = { git = ') and 'PrismML-Eng/mlx' in line)
)
path.write_text(text)
EOF

uv lock --directory "$SOURCE" --python 3.13
uv sync --directory "$SOURCE" --python 3.13 --no-dev

HF_HUB_ENABLE_HF_TRANSFER=1 "$SOURCE/.venv/bin/hf" download "$MODEL_REPO" \
  --revision "$MODEL_REV" --local-dir "$MODEL"

test -f "$SOURCE/LICENSE"
test -f "$MODEL/LICENSE" || test -f "$MODEL/LICENSE.md"
printf '%s\n' "$SOURCE_REV" > "$ROOT/source.revision"
printf '%s\n' "$MODEL_REV" > "$ROOT/model.revision"
printf '%s\n' "$MLX_WHEEL_VERSION" > "$ROOT/mlx-wheel.version"
echo "Bonsai Local installed at $ROOT"
echo "The uv cache at $ROOT/.uv-cache can be deleted to reclaim space."
