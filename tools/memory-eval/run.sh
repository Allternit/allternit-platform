#!/usr/bin/env bash
# Memory retrieve eval (WP-M1c): incumbent hybrid recall vs the S1 pipeline
# on a LoCoMo-style fixture. Reports recall@k, evidence precision, latency.
#
#   tools/memory-eval/run.sh                       # tiny in-repo fixture, mock S1 (unit test)
#   tools/memory-eval/run.sh /tmp/locomo-fixture.json [k] [out.json]
#
# Real runs use the live decision runtime and embedder when set:
#   ALLTERNIT_S1_URL (default http://127.0.0.1:7717), ALLTERNIT_S1_BACKEND (auto)
#   ALLTERNIT_EMBED_URL (unset = hash vectors)
# Without a reachable S1 the pipeline falls back to S0 order (still useful as
# the S0-only baseline). Public LoCoMo: see locomo_to_fixture.py.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
cd "$root"
if [[ $# -eq 0 ]]; then
  exec cargo test -p allternit-api --lib memory_retrieve::tests::eval_harness_runs_on_the_tiny_fixture -- --nocapture
fi
export ALLTERNIT_MEMORY_EVAL_FIXTURE="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
export ALLTERNIT_MEMORY_EVAL_K="${2:-10}"
[[ $# -ge 3 ]] && export ALLTERNIT_MEMORY_EVAL_OUT="$3"
exec cargo test -p allternit-api --lib memory_retrieve::tests::memory_eval_fixture -- --ignored --nocapture
