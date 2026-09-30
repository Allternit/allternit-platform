#!/usr/bin/env bash
# Regression for the coordinator -> ao-consult shim -> coordinator cycle.
set -euo pipefail
repo_root=$(cd "$(dirname "$0")/../.." && pwd)
scratch=$(mktemp -d "$repo_root/.tmp-wp13-steering-XXXXXX")
trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/bin"
cat > "$scratch/bin/allternit-rails" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "${STEER_CONSULT_CMD:-}" == 'claude --dangerously-skip-permissions -p' ]]
printf 'direct review backend selected\n'
SH
cat > "$scratch/bin/claude" <<'SH'
#!/usr/bin/env bash
exit 0
SH
cat > "$scratch/bin/ao-consult" <<'SH'
#!/usr/bin/env bash
echo 'recursive shim must not be selected' >&2
exit 99
SH
chmod +x "$scratch/bin/"*
printf 'read-only review request\n' > "$scratch/request"
. "$repo_root/.steering/bin/steer-common.sh"
unset STEER_CONSULT_CMD
result=$(PATH="$scratch/bin:$PATH" steer_consult "$repo_root" "$scratch/request")
[[ "$result" == 'direct review backend selected' ]]
STEER_CONSULT_CMD='cat' result=$(STEER_CONSULT_CMD='cat' steer_consult "$repo_root" "$scratch/request")
[[ "$result" == 'read-only review request' ]]
printf 'PASS: coordinator pins a direct reviewer; explicit overrides remain supported\n'
