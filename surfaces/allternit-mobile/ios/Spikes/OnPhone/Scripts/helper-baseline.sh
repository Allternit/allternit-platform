#!/usr/bin/env bash
# Today's helper baseline for S0: time screen reads and screenshots through the
# runtime's per-tool route (POST /api/v1/phones/:serial/tool {name, arguments}),
# the same path phone_task's loop and a bot's direct tool call use.
#
#   RUNTIME_URL=http://127.0.0.1:<port> RUNTIME_TOKEN=... SERIAL=<udid> \
#     Scripts/helper-baseline.sh [runs] > helper-baseline.json
#
# Open the target app on the phone first; set APP=<name> to label the run.
set -euo pipefail
RUNS="${1:-20}"
: "${RUNTIME_URL:?set RUNTIME_URL}" "${RUNTIME_TOKEN:?set RUNTIME_TOKEN}" "${SERIAL:?set SERIAL}"
APP="${APP:-unlabelled}"

time_tool() { # name -> one line per run: ms or "err <code>"
  local name="$1"
  for _ in $(seq "$RUNS"); do
    out=$(curl -s -o /dev/null -w '%{http_code} %{time_total}' -m 90 \
      -H "Authorization: Bearer ${RUNTIME_TOKEN}" -H 'Content-Type: application/json' \
      -X POST "${RUNTIME_URL}/api/v1/phones/${SERIAL}/tool" \
      -d "{\"name\":\"${name}\",\"arguments\":{}}" || echo "000 0")
    code=${out%% *}; secs=${out##* }
    if [ "$code" = "200" ]; then python3 -c "print(round($secs*1000,1))"; else echo "err $code"; fi
  done
}

read_ms=$(time_tool phone.screen_text | paste -sd, -)
shot_ms=$(time_tool phone.screenshot | paste -sd, -)
python3 - "$APP" "$read_ms" "$shot_ms" <<'EOF'
import json, statistics, sys
app, r, s = sys.argv[1:4]
def parse(x):
    ok = [float(v) for v in x.split(",") if v and not v.startswith("err")]
    errs = [v for v in x.split(",") if v.startswith("err")]
    return {"samples": ok, "median": statistics.median(ok) if ok else None, "errors": errs}
print(json.dumps({"helper": "desktop usb/wda via /api/v1/phones/:serial/tool", "app": app,
                  "read": parse(r), "screenshot": parse(s)}, indent=2))
EOF
