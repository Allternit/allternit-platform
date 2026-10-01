#!/usr/bin/env bash
# Local fine-tune of the S1 typed-decisions checkpoint (MPS on Apple silicon).
#
#   tools/system-one-local/laya/finetune/run-finetune.sh <export-dir> <out-checkpoint-dir> [finetune_laya.py flags]
#
# Uses the Laya venv Desktop installs (LAYA_HOME, default ~/Library/Application Support/Allternit/laya).
# Then: bun src/cli.ts calibrate --q26 --tune <out>/scored-tune.jsonl --cert <out>/scored-cert.jsonl
# and only if a bank passes, swap it in with Desktop's SystemOneManager.setCheckpoint({path: <out>}).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ $# -ge 2 ] || { echo "usage: run-finetune.sh <export-dir> <out-dir> [flags]" >&2; exit 2; }
LAYA_HOME="${LAYA_HOME:-$HOME/Library/Application Support/Allternit/laya}"
PY="$LAYA_HOME/.venv/bin/python"
[ -x "$PY" ] || { echo "Laya venv missing: run tools/system-one-local/laya/install-laya.sh" >&2; exit 3; }
DEV=()
if [ "$(uname -sm)" = "Darwin arm64" ]; then DEV=(--device mps); export PYTORCH_ENABLE_MPS_FALLBACK=1; fi
EXPORT="$1"; OUT="$2"; shift 2
exec "$PY" "$HERE/finetune_laya.py" --export-dir "$EXPORT" --out "$OUT" "${DEV[@]}" "$@"
