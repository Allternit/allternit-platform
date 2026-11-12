#!/usr/bin/env bash
# Allternit status vantage probe (standby host). One run = one probe round.
# Needs only: bash, curl, openssl, awk, grep, sed (all on a stock Debian/Ubuntu box).
#
# 1. Fetch the target list from $STATUS_BASE/api/v1/targets.json (cached; cache is the fallback).
# 2. Probe every target that lists the "allternit-standby" vantage (status code, body marker, latency, 8 s timeout).
# 3. Sign the ingest JSON with HMAC-SHA256 and POST it to /api/v1/ingest.
# 4. Watch the watcher: GET /api/health. Stale or unreachable on 3 runs in a row -> one email via local
#    sendmail, again once an hour while still bad, and one "back" email when it recovers.
#
# Config (file $ALLTERNIT_STATUS_CONF, default /etc/allternit-status/probe.conf, KEY=value shell syntax;
# environment variables win over the file):
#   STATUS_BASE  default https://status.allternit.com
#   KEY_FILE     default /etc/allternit-status/ingest.key   (shared HMAC secret, one line)
#   STATE_DIR    default /var/lib/allternit-status-probe    (targets cache, alert state)
#   ALERT_TO     email address for watcher alerts (empty = no email, log only)
#   ALERT_FROM   default status-probe@<hostname>
#   SENDMAIL     default sendmail
set -u
export LC_ALL=C

_e_base=${STATUS_BASE-}; _e_key=${KEY_FILE-}; _e_state=${STATE_DIR-}
_e_to=${ALERT_TO-}; _e_from=${ALERT_FROM-}; _e_sm=${SENDMAIL-}
CONF=${ALLTERNIT_STATUS_CONF:-/etc/allternit-status/probe.conf}
# shellcheck disable=SC1090
[ -r "$CONF" ] && . "$CONF"
[ -n "$_e_base" ] && STATUS_BASE=$_e_base
[ -n "$_e_key" ] && KEY_FILE=$_e_key
[ -n "$_e_state" ] && STATE_DIR=$_e_state
[ -n "$_e_to" ] && ALERT_TO=$_e_to
[ -n "$_e_from" ] && ALERT_FROM=$_e_from
[ -n "$_e_sm" ] && SENDMAIL=$_e_sm
STATUS_BASE=${STATUS_BASE:-https://status.allternit.com}
STATUS_BASE=${STATUS_BASE%/}
KEY_FILE=${KEY_FILE:-/etc/allternit-status/ingest.key}
STATE_DIR=${STATE_DIR:-/var/lib/allternit-status-probe}
ALERT_TO=${ALERT_TO:-}
ALERT_FROM=${ALERT_FROM:-status-probe@$(hostname 2>/dev/null || echo localhost)}
SENDMAIL=${SENDMAIL:-sendmail}

UA="allternit-status-probe/1"
FAIL_LIMIT=3
REMIND_SECS=3600
US=$(printf '\037')

log() { printf '%s probe: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >&2; }

mkdir -p "$STATE_DIR" || { log "cannot create $STATE_DIR"; exit 2; }
work=$(mktemp -d "${TMPDIR:-/tmp}/allternit-probe.XXXXXX") || exit 2
trap 'rm -rf "$work"' EXIT

# One run at a time (flock is on the Linux host; skipped where missing).
if command -v flock >/dev/null 2>&1; then
  exec 9>"$STATE_DIR/lock"
  flock -n 9 || { log "previous run still going, skipping"; exit 0; }
fi

# --- JSON helpers (no jq on the host) -----------------------------------------
# Reads targets.json on stdin; prints "#version<US>N" then one line per target that
# lists the allternit-standby vantage: id US url US method US expect US follow US marker US timeout_ms
parse_targets() {
  awk -v US="$US" '
  function hexv(h,   k, v, c) { v = 0; for (k = 1; k <= length(h); k++) { c = index("0123456789abcdef", tolower(substr(h, k, 1))) - 1; if (c < 0) c = 0; v = v * 16 + c } return v }
  function utf8(cp) {
    if (cp < 128) return sprintf("%c", cp)
    if (cp < 2048) return sprintf("%c%c", 192 + int(cp / 64), 128 + cp % 64)
    return sprintf("%c%c%c", 224 + int(cp / 4096), 128 + int(cp / 64) % 64, 128 + cp % 64)
  }
  function emit(t, val,   path, k, np, p, n, f) {
    path = key[1]; for (k = 2; k <= d; k++) path = path "." key[k]
    np = split(path, p, ".")
    if (np == 1 && p[1] == "version") { ver = val; return }
    if (p[1] == "targets" && np >= 3) {
      n = p[2] + 0; if (n + 1 > cnt) cnt = n + 1; f = p[3]
      if (np == 3) { fld[n, f] = val; ty[n, f] = t }
      else if (f == "vantages") vant[n] = vant[n] " " val " "
    }
  }
  { buf = buf $0 "\n" }
  END {
    n = length(buf); i = 1; d = 0; ek = 0
    while (i <= n) {
      c = substr(buf, i, 1)
      if (c == " " || c == "\t" || c == "\r" || c == "\n") { i++; continue }
      if (c == "{") { d++; typ[d] = "o"; key[d] = ""; ek = 1; i++; continue }
      if (c == "[") { d++; typ[d] = "a"; key[d] = 0; ek = 0; i++; continue }
      if (c == "}" || c == "]") { d--; ek = 0; i++; continue }
      if (c == ",") { if (typ[d] == "o") ek = 1; else key[d]++; i++; continue }
      if (c == ":") { ek = 0; i++; continue }
      if (c == "\"") {
        j = i + 1; out = ""
        while (j <= n) {
          ch = substr(buf, j, 1)
          if (ch == "\\") {
            nx = substr(buf, j + 1, 1)
            if (nx == "u") { out = out utf8(hexv(substr(buf, j + 2, 4))); j += 6 }
            else { if (nx == "n" || nx == "t" || nx == "r") nx = " "; out = out nx; j += 2 }
          } else if (ch == "\"") break
          else { out = out ch; j++ }
        }
        i = j + 1
        if (typ[d] == "o" && ek) { key[d] = out; ek = 0 } else emit("S", out)
        continue
      }
      j = i
      while (j <= n && substr(buf, j, 1) !~ /[,}\] \t\r\n]/) j++
      emit("P", substr(buf, i, j - i)); i = j
    }
    print "#version" US ver
    for (n = 0; n < cnt; n++) {
      if (vant[n] != "" && index(vant[n], " allternit-standby ") == 0) continue
      if (fld[n, "id"] == "" || fld[n, "url"] == "") continue
      m = (ty[n, "body_marker"] == "S") ? fld[n, "body_marker"] : ""
      meth = fld[n, "method"]; if (meth == "") meth = "GET"
      ex = fld[n, "expect_status"]; if (ex == "") ex = 200
      fo = fld[n, "follow_redirects"]; if (fo == "") fo = "false"
      to = fld[n, "timeout_ms"]; if (to == "") to = 8000
      print fld[n, "id"] US fld[n, "url"] US meth US ex US fo US m US to
    }
  }'
}

jstr() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | tr -d '\000-\037'; }

# probe_one <idx> <id> <url> <method> <expect> <follow> <marker> <timeout_ms>  -> writes $work/res.<idx>
probe_one() {
  local idx=$1 id=$2 url=$3 method=$4 expect=$5 follow=$6 marker=$7 tmo=$8
  local body="$work/body.$idx" out rc code secs sc lat mk err ok t
  t=$(awk -v m="$tmo" 'BEGIN { s = m / 1000; if (s > 8) s = 8; if (s < 1) s = 1; printf "%.1f", s }')
  local args=(-sS -o "$body" -w '%{http_code}|%{time_total}' --max-time "$t" --connect-timeout 5 -A "$UA" -X "$method")
  [ "$follow" = "true" ] && args+=(-L --max-redirs 5)
  [ "$method" = "HEAD" ] && args=("${args[@]}" -I)
  out=$(curl "${args[@]}" "$url" 2>"$work/err.$idx"); rc=$?
  code=${out%%|*}; secs=${out#*|}
  err=""; sc=null; lat=null; mk=null; ok=false
  if [ $rc -ne 0 ]; then
    case $rc in 28) err=timeout ;; 6) err=dns ;; 7) err=connect ;; 35|51|58|60) err=tls ;; *) err="curl_$rc" ;; esac
  else
    sc=$code
    lat=$(awk -v s="$secs" 'BEGIN { printf "%d", s * 1000 + 0.5 }')
    if [ "$code" != "$expect" ]; then err="status_$code"; fi
    if [ -n "$marker" ]; then
      if grep -qF -e "$marker" "$body" 2>/dev/null; then mk=true; else mk=false; [ -z "$err" ] && err="marker_missing"; fi
    fi
    [ -z "$err" ] && ok=true
  fi
  rm -f "$body"
  if [ -n "$err" ]; then err="\"$err\""; else err=null; fi
  printf '{"target_id":"%s","ok":%s,"status_code":%s,"latency_ms":%s,"marker_ok":%s,"error":%s}' \
    "$(jstr "$id")" "$ok" "$sc" "$lat" "$mk" "$err" >"$work/res.$idx"
}

send_mail() { # subject, body
  if [ -z "$ALERT_TO" ]; then log "alert (no ALERT_TO set): $1"; return 0; fi
  if ! command -v "$SENDMAIL" >/dev/null 2>&1; then log "alert not sent, no $SENDMAIL: $1"; return 1; fi
  {
    printf 'To: %s\nFrom: %s\nSubject: %s\nContent-Type: text/plain; charset=UTF-8\n\n%s\n' "$ALERT_TO" "$ALERT_FROM" "$1" "$2"
  } | "$SENDMAIL" -t -f "$ALERT_FROM" 2>/dev/null || { log "sendmail failed for: $1"; return 1; }
  log "alert sent: $1"
}

# --- 1. targets ---------------------------------------------------------------
cache="$STATE_DIR/targets.json"
if curl -sS -m 10 -A "$UA" -o "$work/targets.json" -w '%{http_code}' "$STATUS_BASE/api/v1/targets.json" 2>/dev/null | grep -q '^200$' \
   && grep -q '"targets"' "$work/targets.json"; then
  cp "$work/targets.json" "$work/targets.new" && mv "$work/targets.new" "$cache"
elif [ -s "$cache" ]; then
  log "targets fetch failed, using cached copy"
else
  log "targets fetch failed and no cache"
fi

ingest_rc=0
if [ -s "$cache" ]; then
  parse_targets <"$cache" >"$work/targets.tsv"
  version=$(sed -n '1s/^#version'"$US"'//p' "$work/targets.tsv")
  idx=0
  while IFS="$US" read -r id url method expect follow marker tmo; do
    case $id in '#'*|'') continue ;; esac
    idx=$((idx + 1))
    probe_one "$idx" "$id" "$url" "$method" "$expect" "$follow" "$marker" "$tmo" &
  done <"$work/targets.tsv"
  wait

  # --- 2. payload + signature -------------------------------------------------
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  results=""; n=0
  for f in "$work"/res.*; do
    [ -f "$f" ] || continue
    results="${results:+$results,}$(cat "$f")"; n=$((n + 1))
  done
  vfield=""; case $version in ''|*[!0-9]*) ;; *) vfield="\"targets_version\":$version," ;; esac
  payload="{\"schema\":\"allternit.ingest/v1\",\"vantage\":\"allternit-standby\",\"ts\":\"$ts\",$vfield\"results\":[$results]}"

  if [ ! -r "$KEY_FILE" ]; then
    log "cannot read $KEY_FILE"; ingest_rc=2
  elif [ "$n" -eq 0 ]; then
    log "no targets for this vantage"
  else
    secret=$(tr -d '\r\n' <"$KEY_FILE")
    # The key is briefly visible in this host's process list (openssl -hmac). Single-user box.
    sig=$(printf '%s' "$payload" | openssl dgst -sha256 -hmac "$secret" | awk '{ print $NF }')
    # --- 3. POST --------------------------------------------------------------
    code=$(curl -sS -m 15 -A "$UA" -o "$work/ingest.out" -w '%{http_code}' -X POST \
      -H 'Content-Type: application/json' -H "X-Allternit-Signature: sha256=$sig" \
      --data-binary "$payload" "$STATUS_BASE/api/v1/ingest" 2>"$work/ingest.err") || code=000
    if [ "$code" = 200 ] || [ "$code" = 202 ] || [ "$code" = 204 ]; then
      log "ingest ok ($n results, HTTP $code)"
    else
      log "ingest failed: HTTP $code $(head -c 200 "$work/ingest.out" 2>/dev/null) $(head -c 200 "$work/ingest.err" 2>/dev/null)"
      ingest_rc=1
    fi
  fi
fi

# --- 4. watch the watcher -------------------------------------------------------
hcode=$(curl -sS -m 8 -A "$UA" -o "$work/health.out" -w '%{http_code}' "$STATUS_BASE/api/health" 2>"$work/health.err") || hcode=000
if [ "$hcode" = 200 ] && grep -q '"status" *: *"ok"' "$work/health.out"; then healthy=1; else healthy=0; fi

st="$STATE_DIR/watch.state"   # lines: fails=N alerting=0|1 last_alert=epoch
fails=0; alerting=0; last=0
if [ -r "$st" ]; then
  fails=$(sed -n 's/^fails=//p' "$st"); alerting=$(sed -n 's/^alerting=//p' "$st"); last=$(sed -n 's/^last_alert=//p' "$st")
fi
fails=${fails:-0}; alerting=${alerting:-0}; last=${last:-0}
now=$(date +%s)

if [ "$healthy" = 1 ]; then
  if [ "$alerting" = 1 ]; then
    send_mail "[Allternit status] status page is back" "The status page health check passes again ($STATUS_BASE/api/health, checked from $(hostname 2>/dev/null))."
  fi
  fails=0; alerting=0
else
  fails=$((fails + 1))
  reason="HTTP $hcode $(head -c 200 "$work/health.out" 2>/dev/null)"
  log "health check bad ($fails in a row): $reason"
  if [ "$fails" -ge "$FAIL_LIMIT" ]; then
    if [ "$alerting" != 1 ]; then
      if send_mail "[Allternit status] status page is stale or unreachable" "$STATUS_BASE/api/health has failed $fails runs in a row.
Last answer: $reason
Checked from $(hostname 2>/dev/null). This host also keeps probing. Next reminder in an hour if it is still bad."; then
        alerting=1; last=$now
      fi
    elif [ $((now - last)) -ge "$REMIND_SECS" ]; then
      send_mail "[Allternit status] still stale or unreachable" "$STATUS_BASE/api/health has failed $fails runs in a row.
Last answer: $reason" && last=$now
    fi
  fi
fi
printf 'fails=%s\nalerting=%s\nlast_alert=%s\n' "$fails" "$alerting" "$last" >"$st.tmp" && mv "$st.tmp" "$st"
exit "$ingest_rc"
