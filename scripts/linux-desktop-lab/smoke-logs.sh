#!/usr/bin/env bash
# CI step 4 part 3 (also run by hand in a lab container): fail if the app's
# logs show a failed start. Reads main.log (any main.log under the user's
# config dir) and the app's stdout (/tmp/app.out), which carries the same lines
# when the app quits before electron-log has made the file.
# Usage: smoke-logs.sh [user]
U="${1:-tester}"
HOMEDIR=$(getent passwd "$U" | cut -d: -f6)
FILES=$(find "$HOMEDIR/.config/@allternit" -name main.log 2>/dev/null; echo /tmp/app.out)
# shellcheck disable=SC2086
if grep -HEi "Initialization Error|Failed to initialize bundled mode|Failed to start Allternit Backend|GLIBC.*not found" $FILES 2>/dev/null | cut -c1-300 | head -5 | grep .; then
  echo "FAIL: app logs show a failed start"; exit 1
fi
echo "PASS: no init failure in app logs"
