#!/usr/bin/env bash
# Start the Rust voice sidecar (sherpa-onnx STT + TTS) on PORT (default 8001).
# Model packs download on first use into ~/.allternit/models/voice/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
export PORT="${PORT:-8001}"
exec cargo run -p voice-service
