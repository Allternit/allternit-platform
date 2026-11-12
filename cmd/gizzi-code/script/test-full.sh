#!/usr/bin/env bash
# test-full.sh — run every gizzi-code test file (bun run test:full).
#
# 1. Shipped feature flags. `feature()` from bun:bundle is resolved at
#    transpile time and bunfig.toml cannot set it, so without --feature every
#    gated path is false under bun test and the tests of shipped features
#    (test/tui-feature-flags.test.ts) fail against a build that never ships.
#    The flags come from script/features.mjs, as in script/ci-smoke-test.sh.
#
# 2. Own process for files that legacy mock.module calls break. mock.module
#    replaces a module for the whole process and is never undone, and several
#    session tests (fallback, wrap-up, guardrails-loop, refusal-handoff, ...)
#    replace session/llm and providers/provider. The files below spy on or run
#    the real modules, so they run in a second process (ci-smoke-test.sh does
#    the same for answer-policy). `--isolate` would also fix this, but other
#    files rely on state shared across files and fail under it.
#
# Usage: bash script/test-full.sh               # whole suite, both passes
#        bash script/test-full.sh <bun test args>  # one pass, e.g. a file filter

set -u
cd "$(dirname "$0")/.."

feature_args=()
while IFS= read -r f; do
  [ -n "$f" ] && feature_args+=("--feature=$f")
done < <(bun -e 'import("./script/features.mjs").then(m => console.log(m.resolveFeatures().join("\n")))')

common=("${feature_args[@]}" --timeout 30000 --preload ./test/preload.ts)

if [ "$#" -gt 0 ]; then
  exec bun test "${common[@]}" "$@"
fi

OWN_PROCESS=(
  test/session/answer-policy.test.ts
  test/session/handoff.test.ts
)

ignore_args=()
for f in "${OWN_PROCESS[@]}"; do
  ignore_args+=("--path-ignore-patterns=$f")
done

echo "=== shared process: test/ (minus ${#OWN_PROCESS[@]} own-process files) ==="
bun test "${common[@]}" "${ignore_args[@]}" test/
rc_shared=$?

echo
echo "=== own process: ${OWN_PROCESS[*]} ==="
bun test "${common[@]}" --tsconfig-override=tsconfig.json "${OWN_PROCESS[@]}"
rc_own=$?

if [ "$rc_shared" -ne 0 ] || [ "$rc_own" -ne 0 ]; then
  echo "FULL SUITE FAIL: shared exit $rc_shared, own-process exit $rc_own" >&2
  exit 1
fi
echo "FULL SUITE PASS"
