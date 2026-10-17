#!/usr/bin/env bash
# Live conformance: drives a phone running Allternit Phone Link with the UNCHANGED
# iCTRL reference client (ictrl-fork/Scripts/ictrl_rpc.py).
#
#   scripts/conformance.sh [HOST:PORT] [--serial SERIAL] [--provision] [--act]
#
# HOST:PORT  default 127.0.0.1:52100 reached through `adb forward` (set up with --provision,
#            which also arms the one-shot pairing). For a LAN address, pairing needs a tap on
#            the phone's Allow notification (the armed grant is loopback-only by design).
# --act      also run screen-changing calls (home, launch Chrome/Settings guard, swipe).
# ICTRL_RPC  path to ictrl_rpc.py (default: ~/Desktop/allternit-workspace/ictrl-fork/Scripts/ictrl_rpc.py)
#
# Uses a throwaway HOME so ~/.ictrl/token.json is never touched.
set -uo pipefail

HOST="127.0.0.1:52100" SERIAL="" PROVISION=false ACT=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --serial) SERIAL="$2"; shift 2 ;;
    --provision) PROVISION=true; shift ;;
    --act) ACT=true; shift ;;
    *:*) HOST="$1"; shift ;;
    *) echo "unknown arg $1" >&2; exit 2 ;;
  esac
done
HERE="$(cd "$(dirname "$0")" && pwd)"
RPC="${ICTRL_RPC:-$HOME/Desktop/allternit-workspace/ictrl-fork/Scripts/ictrl_rpc.py}"
[[ -f "$RPC" ]] || { echo "ictrl_rpc.py not found at $RPC (set ICTRL_RPC)"; exit 2; }
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
export HOME_REAL="$HOME"
run() { HOME="$WORK" python3 -I "$RPC" --host "$HOST" "$@"; }

PASS=0 FAIL=0
ok()   { echo "PASS  $1"; PASS=$((PASS+1)); }
bad()  { echo "FAIL  $1"; [[ -n "${2:-}" ]] && echo "      $2"; FAIL=$((FAIL+1)); }
# expect NAME JSON-PYTHON-EXPR CMD...   (expr is evaluated with `r` = parsed stdout)
expect() {
  local name="$1" expr="$2"; shift 2
  local out; out="$("$@" 2>&1)"
  if python3 -c "import json,sys; r=json.loads(sys.argv[1]); sys.exit(0 if ($expr) else 1)" "$out" 2>/dev/null; then ok "$name"
  else bad "$name" "$(echo "$out" | head -c 600)"; fi
}

if $PROVISION; then
  ARGS=(--forward "${HOST##*:}"); [[ -n "$SERIAL" ]] && ARGS+=(-s "$SERIAL")
  "$HERE/provision.sh" "${ARGS[@]}" || { echo "provisioning failed"; exit 1; }
  sleep 2
fi

echo "== $HOST"
# 1. Unauthenticated identity, with the device header every response carries.
HDRS="$(curl -s -D - -o "$WORK/whoami.json" "http://$HOST/whoami")"
if grep -qi '^X-ICTRL-Device:' <<<"$HDRS" && python3 -c "import json;d=json.load(open('$WORK/whoami.json'));assert d['protocol_version']==1 and d['device_id']" 2>/dev/null
then ok "whoami + X-ICTRL-Device"; else bad "whoami" "$HDRS"; fi

# 2. Unsealed /rpc is refused in the clear.
CODE="$(curl -s -o "$WORK/plain.json" -w '%{http_code}' -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","method":"device_info","id":1}' "http://$HOST/rpc")"
if [[ "$CODE" == 401 ]] && grep -q auth_required "$WORK/plain.json"; then ok "unsealed rpc -> 401 auth_required"; else bad "unsealed rpc" "$CODE $(cat "$WORK/plain.json")"; fi

# 3. Pair with the reference client (X25519; blocks on Allow unless armed over the cable).
if run pair --name conformance >"$WORK/pair.log" 2>&1; then ok "pair (X25519)"; else bad "pair" "$(cat "$WORK/pair.log")"; echo "cannot continue without a key"; exit 1; fi

# 4. Read-only calls.
expect "device_info pixels/protocol 1" "r['result']['coordinate_space']=='pixels' and r['result']['protocol_version']==1 and r['result']['point_width']>0" run call device_info
expect "is_locked" "'locked' in r['result']" run call is_locked
expect "list_apps" "r['result']['count']>0" run call list_apps '{"element_type":"all"}'
expect "config lists sessions" "len(r['result']['sessions'])>=1" run call config
expect "unknown method -> -32601" "r['error']['code']==-32601" run call no_such_method
LOCKED="$(run call is_locked | python3 -c 'import json,sys;print(json.load(sys.stdin)["result"]["locked"])')"
if [[ "$LOCKED" == "True" ]]; then
  expect "locked phone refuses page_source" "r['error']['data']['type']=='screen_locked'" run call page_source
  echo "(phone is locked: unlock it and re-run for the screen checks)"
else
  expect "page_source text" "r['result']['format']=='text' and len(r['result']['source'])>0" run call page_source '{"visible_only":true}'
  expect "page_source structured" "len(r['result']['nodes'])>0" run call page_source '{"structured":true,"max_elements":50}'
  expect "screen_text" "r['result']['source']=='tree'" run call screen_text
  expect "find_element (missing)" "r['result']['exists']==False" run call find_element '{"element_id":"zz-no-such-element-zz"}'
  if run shot "$WORK/shot.jpg" --scale 0.5 >"$WORK/shot.log" 2>&1 && [[ -s "$WORK/shot.jpg" ]]; then ok "screenshot ($(head -c 120 "$WORK/shot.log"))"
  else bad "screenshot (needs Android 11+)" "$(cat "$WORK/shot.log")"; fi
fi

# 5. Envelope rejection rules, using the reference client's own crypto.
HOME="$WORK" python3 -I - "$RPC" "$HOST" <<'PY' && ok "replay -> nonce_replayed, skew -> ts_skew, forged tag -> bad_tag" || bad "replay/skew/tag checks"
import importlib.util, json, os, secrets, sys, time, base64, hashlib
spec = importlib.util.spec_from_file_location("ictrl_rpc", sys.argv[1]); r = importlib.util.module_from_spec(spec); spec.loader.exec_module(r)
host = sys.argv[2]
rec = next(v for v in json.load(open(os.path.expanduser("~/.ictrl/token.json"))).values() if isinstance(v, dict) and "key" in v)
key = bytes.fromhex(rec["key"]); tok = rec["tok"]
def env(ts, nonce, tamper=False):
    body = json.dumps({"jsonrpc": "2.0", "method": "is_locked", "params": {}, "id": 1}).encode()
    ct = r._keystream_xor(r._hmac(key, r.LABEL_ENC_REQ + nonce), nonce, body)
    tag = r._hmac(r._hmac(key, r.LABEL_MAC + nonce), b"v1" + bytes.fromhex(tok) + ts.to_bytes(8, "big") + nonce + ct)
    if tamper: tag = bytes(32)
    return {"v": 1, "tok": tok, "ts": ts, "nonce": nonce.hex(), "ct": base64.b64encode(ct).decode(), "tag": tag.hex()}
n = secrets.token_bytes(16); t = int(time.time())
s1, _ = r._post(f"http://{host}/rpc", env(t, n), 30)
s2, b2 = r._post(f"http://{host}/rpc", env(t, n), 30)
s3, b3 = r._post(f"http://{host}/rpc", env(t - 600, secrets.token_bytes(16)), 30)
s4, b4 = r._post(f"http://{host}/rpc", env(t, secrets.token_bytes(16), tamper=True), 30)
assert s1 == 200, s1
assert s2 == 401 and b2["error"]["code"] == "nonce_replayed", (s2, b2)
assert s3 == 401 and b3["error"]["code"] == "ts_skew" and abs(b3["error"]["server_ts"] - t) < 60, (s3, b3)
assert s4 == 401 and b4["error"]["code"] == "bad_tag", (s4, b4)
PY

# 6. Optional screen-changing checks.
if $ACT && [[ "$LOCKED" != "True" ]]; then
  expect "home" "'foreground_app' in r['result']" run call home
  expect "swipe up" "'message' in r['result']" run call swipe '{"direction":"up"}'
  expect "settings is off limits" "r['error']['data']['type']=='protected_app'" run call launch_app '{"bundle_id":"com.android.settings"}'
  expect "open_url https" "'handler' in r['result']" run call open_url '{"url":"https://allternit.com"}'
  sleep 2
  expect "tap" "'message' in r['result']" run call tap '{"x":10,"y":10,"settle":false}'
  expect "back" "'message' in r['result']" run call back
  run call home >/dev/null
fi

# 7. Revoke this test's key so it does not linger in the phone's paired list.
TOK="$(HOME="$WORK" python3 -c 'import json,os;d=json.load(open(os.path.expanduser("~/.ictrl/token.json")));print(next(v["tok"] for v in d.values() if isinstance(v,dict) and "tok" in v))')"
expect "revoke own session" "True" run call config "{\"revoke_session_id\":\"$TOK\"}"

echo "== $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]]
