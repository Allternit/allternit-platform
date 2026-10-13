#!/usr/bin/env bash
# Live check for vendor subscriptions as Allternit bots (one-step connect).
#
# Run on a machine with your real vendor sessions, with the subscription
# gateway and allternit-api running in single-machine mode (see
# docs/gateway/QUICKSTART.md, "The short way"). For every product it:
#   1. calls POST /api/v1/gateway/connect, shows you the terms when asked and
#      waits while you sign in in the window the gateway opens;
#   2. sends one test turn to the first bot connect made ("Reply with the
#      single word: pong") and checks the reply;
#   3. sends a second turn to the same thread (continuation);
#   4. writes a PASS / FAIL / SKIP line per product to the report.
# Nothing is automated on the vendor side beyond what the adapters already do:
# you sign in, you agree to the terms, you clear any verification check.
#
# Usage:
#   scripts/e2e/gateway-live-check.sh                 # every product
#   scripts/e2e/gateway-live-check.sh chatgpt grok-bot
# Env:
#   API=http://127.0.0.1:18013/api/v1   DESK=<ALLTERNIT_DESKTOP_ACCESS_TOKEN>   USER_ID=me
#   ANTHROPIC_API_KEY=...               (claude-managed-agents)
#   MUSE_NUMBER=... WA_ACCESS_TOKEN=... WA_PHONE_NUMBER_ID=... WA_APP_SECRET=...   (muse)
#   REPORT=gateway-live-report.txt
set -uo pipefail
API=${API:-http://127.0.0.1:18013/api/v1}
DESK=${DESK:-dev-desktop-secret}
USER_ID=${USER_ID:-me}
REPORT=${REPORT:-gateway-live-report.txt}
ALL=(chatgpt claude gemini copilot kimi chatgpt-dots grok-bot claude-managed-agents openclaw hermes muse)
[ $# -gt 0 ] && PRODUCTS=("$@") || PRODUCTS=("${ALL[@]}")

command -v jq >/dev/null || { echo "jq is required (brew install jq)"; exit 1; }
api() { curl -sS -m 180 -H "x-allternit-desktop-access-token: $DESK" -H "x-allternit-user-id: $USER_ID" -H 'content-type: application/json' "$@"; }
say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
ask() { local a; read -r -p "$1 " a </dev/tty; printf '%s' "$a"; }
record() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" | tee -a "$REPORT" >/dev/null; printf '  → %s: %s\n' "$2" "$3"; }

api "$API/gateway/connect" >/dev/null || { echo "allternit-api is not reachable at $API"; exit 1; }
{ echo "# gateway live check $(date -u +%Y-%m-%dT%H:%M:%SZ)"; echo "# api: $API"; } >> "$REPORT"

for p in "${PRODUCTS[@]}"; do
  say "== $p"
  extra="{}"
  case "$p" in
    claude-managed-agents)
      [ -n "${ANTHROPIC_API_KEY:-}" ] || { record "$p" SKIP "set ANTHROPIC_API_KEY (and SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID on the gateway)"; continue; }
      extra=$(jq -n --arg k "$ANTHROPIC_API_KEY" '{apiKey:$k}') ;;
    muse)
      [ -n "${MUSE_NUMBER:-}" ] || { record "$p" SKIP "set MUSE_NUMBER and the WA_* WhatsApp Business credentials"; continue; }
      extra=$(jq -n --arg a "$MUSE_NUMBER" --arg t "${WA_ACCESS_TOKEN:-}" --arg n "${WA_PHONE_NUMBER_ID:-}" --arg s "${WA_APP_SECRET:-}" \
        '{agentAddress:$a, accessToken:$t, phoneNumberId:$n, appSecret:$s} | with_entries(select(.value != ""))') ;;
  esac
  accept=false
  res=""
  for attempt in $(seq 1 60); do
    body=$(jq -n --arg p "$p" --argjson e "$extra" --argjson acc "$accept" '{product:$p} + $e + (if $acc then {acceptTerms:true} else {} end)')
    res=$(api -X POST "$API/gateway/connect" -d "$body") || res='{"status":"error","message":"request failed"}'
    status=$(jq -r '.status // "error"' <<<"$res")
    echo "  $status: $(jq -r '.message // .error // ""' <<<"$res")"
    case "$status" in
      ready) break ;;
      needs_consent)
        echo; jq -r '.action.terms' <<<"$res" | fold -s -w 100 | sed 's/^/    /'; echo
        [ "$(ask 'Agree to this? [y/N]')" = y ] || { status=declined; break; }
        accept=true ;;
      needs_sign_in)
        [ "$attempt" = 1 ] && ask "Sign in in the window that opened, then press Enter (connect keeps checking)." >/dev/null
        sleep 3 ;;
      blocked)
        ask "Clear the verification check in the vendor window, then press Enter (or type s to skip)." | grep -q s && break ;;
      needs_setup|needs_key)
        jq -r '.action.steps[]? | "    - " + .' <<<"$res"
        [ "$(ask 'Fix this, then press Enter to retry (or type s to skip).')" = s ] && break ;;
      *) break ;;
    esac
  done
  if [ "$status" != ready ]; then record "$p" FAIL "connect ended at $status: $(jq -r '.message // .error // ""' <<<"$res")"; continue; fi

  bots=$(jq -r '[.bots[] | "\(.name) [\(.state)]"] | join(", ")' <<<"$res")
  echo "  bots: $bots"
  bot=$(jq -r '.bots[0].botId' <<<"$res")
  sid="live-$p-$(date +%s)"
  thread=$(api -X POST "$API/threads" -d "$(jq -n --arg b "$bot" --arg s "$sid" '{botId:$b, title:"Live check", sessionId:$s}')" | jq -r '.thread.id // empty')
  [ -n "$thread" ] || { record "$p" FAIL "connected ($bots) but could not open a thread"; continue; }

  turn() { api -X POST "$API/agent-sessions/$sid/messages" -d "$(jq -n --arg t "$1" --arg c "c-$RANDOM-$(date +%s)" '{text:$t, metadata:{correlationId:$c}}')"; }
  r1=$(turn "Reply with the single word: pong")
  t1=$(jq -r '.content // .error // ""' <<<"$r1")
  if [ "$(jq -r '.metadata.pending // false' <<<"$r1")" = true ]; then
    for _ in $(seq 1 20); do sleep 3; api -X POST "$API/threads/$thread/gateway/sync" >/dev/null; done
    t1=$(api "$API/threads/$thread/events?after=0&limit=200" | jq -r '[.events[] | select(.type=="agent.message.completed") | .payload.text // .payload.content // ""] | last // ""')
  fi
  echo "  reply 1: ${t1:0:200}"
  r2=$(turn "Now reply with the single word: ping")
  t2=$(jq -r '.content // .error // ""' <<<"$r2")
  echo "  reply 2: ${t2:0:200}"
  if grep -qi pong <<<"$t1" && grep -qi ping <<<"$t2"; then
    record "$p" PASS "bots: $bots; two turns answered in thread $thread"
  else
    record "$p" FAIL "bots: $bots; replies did not match (1: ${t1:0:80} | 2: ${t2:0:80})"
  fi
done

say "Report: $REPORT"
cat "$REPORT"
