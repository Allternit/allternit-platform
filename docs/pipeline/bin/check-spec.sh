#!/usr/bin/env bash
# check-spec.sh — independent spec-checker loop (Phase 3 + charter layer).
#
# For each docs/pipeline/specs/*.md without a READY/STALLED/REJECT verdict in
# docs/pipeline/verdicts.json: assemble spec-rubric.md + charter.md + taste
# precedents from memory (advisory) + the spec, consult the independent
# reviewer (SPEC_CHECK_CMD test hook, else `allternit-factory orchestration steer consult`), then:
#   READY      -> move spec to docs/pipeline/queue/, record verdict, announce to
#                 wih:pipeline-queue (announce failure = hard error, R4/C3);
#                 then create a rails ticket for the queued spec (B3-R1):
#                 POST /api/rails/tickets with title = spec's first heading,
#                 kind = feature, labels ["pipeline","spec:<slug>"], and the
#                 queue path + brief provenance in `description` (the only
#                 free-text field — there is no note field). Ticket creation
#                 failure = hard error (log + exit 1), but it gates ticket
#                 creation ONLY: the spec stays in queue/ with its READY
#                 verdict and builds via the legacy file path. The ticket_id
#                 is merged into verdicts.json (merge semantics — a later
#                 verdict_set never wipes it). If the spec's frontmatter
#                 declares `blocks: [<slug>, ...]` (B3-R4: the listed slugs
#                 block this spec), dependency edges are posted from each
#                 blocker's ticket to the new ticket ({to, kind:"blocks"});
#                 a 409 cycle rejection is logged and the spec flagged in
#                 errors.log (non-fatal).
#   NEEDS-WORK -> record verdict + round, append findings to <slug>.review.md;
#                 2nd NEEDS-WORK ingests the rejection pattern to memory
#                 (:3201, advisory — failure logged, run continues);
#                 3rd round marks the spec STALLED and skips it thereafter
#   REJECT     -> charter violation: move spec to docs/pipeline/rejected/, record
#                 verdict, ingest the violation to memory immediately (taste
#                 signal). REJECT is final — never retried.
# Empty/unparseable consult answer or transport failure: record nothing,
# continue to the next spec (fail open per-spec, never wedge the run).
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
PIPELINE_DIR="${CHECK_SPEC_DIR:-$ROOT/.pipeline}"
SPECS_DIR="$PIPELINE_DIR/specs"
QUEUE_DIR="$PIPELINE_DIR/queue"
VERDICTS_FILE="$PIPELINE_DIR/verdicts.json"
ERRORS_LOG="$PIPELINE_DIR/errors.log"
RUBRIC="${SPEC_RUBRIC:-$PIPELINE_DIR/spec-rubric.md}"
CHARTER="${PIPELINE_CHARTER:-$PIPELINE_DIR/charter.md}"
REJECTED_DIR="$PIPELINE_DIR/rejected"
RAILS_ENSURE="${RAILS_ENSURE:-$PIPELINE_DIR/bin/rails-ensure.sh}"
RAILS_SHARE_URL="http://localhost:8013/api/rails/mail/share"
RAILS_TICKETS_URL="http://localhost:8013/api/rails/tickets"
QUEUE_THREAD="wih:pipeline-queue"
MEMORY_URL="http://localhost:3201/api/ingest"
MEMORY_QUERY_URL="http://localhost:3201/api/search"
MAX_ROUNDS=3

log_error() {
  printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$1" >> "$ERRORS_LOG"
}

# ─── learning loop (M1, advisory) ───────────────────────────────────────────

LEARN_EVENT="${LEARN_EVENT:-$PIPELINE_DIR/bin/learn-event.sh}"
LEARN_PLAYBOOK="${LEARN_PLAYBOOK:-$PIPELINE_DIR/bin/learn-playbook.sh}"
LEARN_REFLECT="${LEARN_REFLECT:-$PIPELINE_DIR/bin/learn-reflect.sh}"

learn_event() { # learn_event <kind> <refs> <summary> — capture, never fatal
  [ -x "$LEARN_EVENT" ] || return 0
  LEARN_PIPELINE_DIR="$PIPELINE_DIR" "$LEARN_EVENT" "$1" "$2" "$3" >/dev/null 2>&1 || true
}

# ─── verdicts.json helpers (python3, like .steering/bin/) ───────────────────

verdict_get() { # verdict_get <slug> -> "verdict rounds" (empty if none)
  python3 - "$VERDICTS_FILE" "$1" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    d = {}
v = d.get(sys.argv[2])
if v:
    print(f"{v.get('verdict','')} {v.get('rounds',0)}")
PY
}

verdict_set() { # verdict_set <slug> <verdict> <rounds> — MERGE semantics
  # (B3-R1): the per-slug dict is merged, never replaced, so a ticket_id
  # recorded by an earlier READY survives any later verdict_set.
  python3 - "$VERDICTS_FILE" "$1" "$2" "$3" <<'PY'
import json, sys, datetime
path, slug, verdict, rounds = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
try:
    d = json.load(open(path))
except Exception:
    d = {}
rec = d.get(slug, {})
rec.update({
    "verdict": verdict,
    "rounds": rounds,
    "updated": datetime.datetime.now(datetime.timezone.utc).isoformat(),
})
d[slug] = rec
with open(path, "w") as f:
    json.dump(d, f, indent=2, sort_keys=True)
    f.write("\n")
PY
}

verdict_merge() { # verdict_merge <slug> <extra-json-object> — merge extra keys
  python3 - "$VERDICTS_FILE" "$1" "$2" <<'PY'
import json, sys, datetime
path, slug, extra = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    d = json.load(open(path))
except Exception:
    d = {}
rec = d.get(slug, {})
rec.update(json.loads(extra))
rec["updated"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
d[slug] = rec
with open(path, "w") as f:
    json.dump(d, f, indent=2, sort_keys=True)
    f.write("\n")
PY
}

# ─── rails announce (hard dependency) ───────────────────────────────────────

announce() { # announce <asset_ref> <note> -> 0 on HTTP 2xx
  local payload code
  payload=$(python3 -c 'import json,sys; print(json.dumps({"thread":"wih:pipeline-queue","asset_ref":sys.argv[1],"note":sys.argv[2]}))' "$1" "$2")
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -X POST "$RAILS_SHARE_URL" \
    -H 'Content-Type: application/json' -d "$payload" 2>/dev/null)
  [[ "$code" == 2* ]]
}

# ─── rails tickets (B3-R1: create is a hard error; B3-R4: edges advisory) ───

# spec_frontmatter_field <spec-file> <field> -> list items, one per line.
# Minimal YAML-subset reader: inline "field: [a, b]" and dashed-list forms.
spec_frontmatter_field() {
  python3 - "$1" "$2" <<'PY'
import sys
path, field = sys.argv[1], sys.argv[2]
try:
    lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
except Exception:
    sys.exit(0)
if not lines or lines[0].strip() != "---":
    sys.exit(0)
fm = []
for line in lines[1:]:
    if line.strip() == "---":
        break
    fm.append(line)
items = []
for i, l in enumerate(fm):
    if l.startswith(field + ":"):
        rest = l[len(field) + 1:].strip()
        if rest.startswith("[") and rest.endswith("]"):
            items = [x.strip().strip("'\"") for x in rest[1:-1].split(",") if x.strip()]
        elif rest:
            items = [rest.strip("'\"")]
        else:
            j = i + 1
            while j < len(fm) and fm[j].startswith(("  - ", "- ")):
                items.append(fm[j].split("- ", 1)[1].strip().strip("'\""))
                j += 1
        break
print("\n".join(items))
PY
}

# split_resp <resp> -> sets globals RESP_CODE / RESP_BODY. Tolerates stubs
# that emit only a body (no -w status line): RESP_CODE is left empty.
split_resp() {
  case "$1" in
    *$'\n'*)
      RESP_CODE="${1##*$'\n'}"
      RESP_BODY="${1%$'\n'*}"
      ;;
    *)
      RESP_CODE=""
      RESP_BODY="$1"
      ;;
  esac
}

# ticket_create <slug> <queue-path> -> ticket id on stdout; non-zero on
# transport/HTTP/parse failure (B3-R1: the ONLY free-text field is
# `description` — TicketCreateRequest has no note field).
ticket_create() {
  local payload resp
  payload=$(python3 - "$1" "$2" <<'PY'
import json, sys
slug, qpath = sys.argv[1], sys.argv[2]
try:
    lines = open(qpath, encoding="utf-8", errors="replace").read().splitlines()
except Exception:
    lines = []
title = ""
for line in lines:
    s = line.strip()
    if s.startswith("# "):
        title = s[2:].strip()
        break
if not title:
    title = slug
prov = []
if lines and lines[0].strip() == "---":
    fm = []
    for line in lines[1:]:
        if line.strip() == "---":
            break
        fm.append(line)
    for i, l in enumerate(fm):
        if l.startswith("provenance_refs:"):
            j = i + 1
            while j < len(fm) and fm[j].startswith(("  - ", "- ")):
                prov.append(fm[j].split("- ", 1)[1].strip())
                j += 1
            break
desc = "queue: %s\nbrief provenance: %s" % (qpath, ", ".join(prov) if prov else "n/a")
print(json.dumps({
    "title": title,
    "description": desc,
    "kind": "feature",
    "labels": ["pipeline", "spec:" + slug],
}))
PY
)
  resp=$(curl -s --max-time 5 -w '\n%{http_code}' -X POST "$RAILS_TICKETS_URL" \
    -H 'Content-Type: application/json' -d "$payload" 2>/dev/null)
  split_resp "$resp"
  if [ -n "$RESP_CODE" ] && [[ "$RESP_CODE" != 2* ]]; then
    return 1
  fi
  python3 -c 'import json,sys; print(json.loads(sys.argv[1])["ticket"]["id"])' "$RESP_BODY"
}

# ticket_find_by_label <label> -> first matching ticket id (empty if none or
# the endpoint is unreachable — the caller logs either way).
ticket_find_by_label() {
  local resp
  resp=$(curl -s --max-time 5 "$RAILS_TICKETS_URL?label=$1" 2>/dev/null)
  python3 -c 'import json,sys
try:
    d = json.loads(sys.argv[1])
except Exception:
    sys.exit(0)
ts = d.get("tickets") or []
if ts:
    print(ts[0].get("id", ""))' "$resp" 2>/dev/null
}

# ticket_add_dependency <from-id> <to-id> -> 0 on 2xx (edge from blocks to).
ticket_add_dependency() {
  local payload resp
  payload=$(python3 -c 'import json,sys; print(json.dumps({"to":sys.argv[1],"kind":"blocks"}))' "$2")
  resp=$(curl -s --max-time 5 -w '\n%{http_code}' -X POST "$RAILS_TICKETS_URL/$1/dependencies" \
    -H 'Content-Type: application/json' -d "$payload" 2>/dev/null)
  split_resp "$resp"
  if [ -n "$RESP_CODE" ]; then
    [[ "$RESP_CODE" == 2* ]]
  else
    # Stub without a status line: a JSON error body (e.g. 409 cycle) fails.
    case "$RESP_BODY" in *'"error"'*) return 1 ;; *) return 0 ;; esac
  fi
}

# ─── memory ingest (advisory) ───────────────────────────────────────────────

ingest_lesson() { # ingest_lesson <slug> <findings>
  local payload code
  payload=$(python3 -c 'import json,sys; print(json.dumps({"content":sys.argv[1]+"\n\n"+sys.argv[2],"source":"pipeline-spec-checker"}))' "$1" "$2")
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -X POST "$MEMORY_URL" \
    -H 'Content-Type: application/json' -d "$payload" 2>/dev/null)
  if [[ "$code" != 2* ]]; then
    log_error "memory ingest failed for spec $1 (HTTP ${code:-000}) — continuing (lessons are advisory)"
    echo "check-spec: memory unreachable for $1 — logged, continuing (advisory)"
  fi
}

# ─── taste precedents (advisory) ────────────────────────────────────────────

query_precedents() { # -> past rejection/lesson text on stdout (empty if memory down)
  local payload
  curl -s --max-time 15 -X GET "$MEMORY_QUERY_URL?q=taste+precedents&limit=5" \
    2>/dev/null \
    | python3 -c 'import json,sys,datetime
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
# C4-R2: precedents older than 90 days are marked [stale] instead of being
# presented as current. Items without a parseable timestamp degrade to current.
# C1-R2: failed-tier content (reverted/rejected/failed attempts) must never
# read as evidence — it stays in the assembled text but labeled [pitfall].
STALE_DAYS = 90
now = datetime.datetime.now(datetime.timezone.utc)

def ts_of(it):
    for k in ("ingested_at", "created_at", "timestamp", "ts", "updated_at"):
        v = it.get(k)
        if v is None:
            continue
        if isinstance(v, (int, float)):
            try:
                return datetime.datetime.fromtimestamp(v, datetime.timezone.utc)
            except Exception:
                return None
        if isinstance(v, str):
            s = v.strip()
            if s.isdigit():
                try:
                    return datetime.datetime.fromtimestamp(int(s), datetime.timezone.utc)
                except Exception:
                    return None
            try:
                return datetime.datetime.fromisoformat(s.replace("Z", "+00:00"))
            except Exception:
                return None
    return None

def is_stale(it):
    t = ts_of(it)
    if t is None:
        return False
    if t.tzinfo is None:
        t = t.replace(tzinfo=datetime.timezone.utc)
    return (now - t).days > STALE_DAYS

def tier_of(it):
    md = it.get("metadata")
    if isinstance(md, dict) and md.get("trust_tier"):
        return md["trust_tier"]
    return it.get("trust_tier", "")

parts = []
if isinstance(d, dict):
    if d.get("answer"): parts.append(str(d["answer"]))
    for k in ("insights","memories"):
        for it in d.get(k) or []:
            if isinstance(it, dict) and it.get("content"):
                c = str(it["content"])
                if is_stale(it): c = "[stale] " + c
                if tier_of(it) == "failed": c = "[pitfall] " + c
                parts.append(c)
print("\n---\n".join(parts)[:3000])' 2>/dev/null
}

# ─── consult ────────────────────────────────────────────────────────────────

consult() { # consult <request-file> -> answer on stdout (may be empty)
  if [ -n "${SPEC_CHECK_CMD:-}" ]; then
    $SPEC_CHECK_CMD < "$1"
  else
    allternit-factory orchestration steer consult --cwd "$PWD" --prompt-file "$1"
  fi
}

# First line, bullet-stripped (the consult prefixes "• "), CR-free, uppercase.
verdict_of() {
  printf '%s\n' "$1" | sed 's/^• //' | head -1 | tr -d '\r' | tr '[:lower:]' '[:upper:]'
}

# ─── main ───────────────────────────────────────────────────────────────────

# Rails first (R0/R1 semantics): abort non-zero if it cannot be made to work.
if ! bash "$RAILS_ENSURE"; then
  echo "check-spec: rails-ensure failed — aborting before any spec check" >&2
  exit 1
fi

if [ ! -d "$SPECS_DIR" ] || ! ls "$SPECS_DIR"/*.md >/dev/null 2>&1; then
  echo "check-spec: no specs in $SPECS_DIR; nothing to do"
  exit 0
fi

mkdir -p "$QUEUE_DIR"
checked=0
for spec in "$SPECS_DIR"/*.md; do
  slug="$(basename "$spec" .md)"
  case "$slug" in *.review) continue ;; esac

  state="$(verdict_get "$slug")"
  verdict="${state%% *}"
  rounds="${state##* }"
  [ -n "$state" ] || rounds=0

  if [ "$verdict" = "READY" ] || [ "$verdict" = "STALLED" ] || [ "$verdict" = "REJECT" ]; then
    continue
  fi

  checked=$((checked + 1))
  request="$(mktemp -t check-spec)"
  {
    cat "$RUBRIC"
    if [ -f "$CHARTER" ]; then
      printf '\n\n=== PIPELINE CHARTER (docs/pipeline/charter.md) ===\n'
      cat "$CHARTER"
    fi
    precedents="$(query_precedents)"
    if [ -n "${precedents// /}" ]; then
      printf '\n\n=== TASTE PRECEDENTS (from memory — past rejections/lessons) ===\n%s\n' "$precedents"
    fi
    # M1-R3: learned playbook rules (advisory, 4KB-capped, [stale] at 90+ days).
    playbook="$([ -x "$LEARN_PLAYBOOK" ] && "$LEARN_PLAYBOOK" "$PIPELINE_DIR/playbook.md" 2>/dev/null)"
    if [ -n "${playbook// /}" ]; then
      printf '\n\n=== LEARNED PLAYBOOK (docs/pipeline/playbook.md — advisory rules) ===\n%s\n' "$playbook"
    fi
    printf '\n\n=== SPEC UNDER REVIEW: docs/pipeline/specs/%s.md ===\n' "$slug"
    cat "$spec"
  } > "$request"

  answer="$(consult "$request")"
  rm -f "$request"

  if [ -z "${answer// /}" ]; then
    echo "check-spec: $slug — empty consult answer or transport failure; skipping (recorded nothing)"
    continue
  fi

  first="$(verdict_of "$answer")"

  case "$first" in
    READY*)
      # Announce FIRST: on failure, log + exit 1 leaving the spec untouched
      # in specs/ with its prior verdict state, so the next run re-consults
      # and retries the whole READY path (no silent unannounced queue file).
      if ! announce "$QUEUE_DIR/$slug.md" "$slug"; then
        log_error "queue announce failed for $QUEUE_DIR/$slug.md — rails has no fallback (R4)"
        echo "check-spec: rails announcement failed for $slug — recorded in $ERRORS_LOG; aborting (spec left in place for retry)" >&2
        exit 1
      fi
      mv "$spec" "$QUEUE_DIR/$slug.md"
      # Keep the review trail with the spec it documents (if any).
      [ -f "$SPECS_DIR/$slug.review.md" ] && mv "$SPECS_DIR/$slug.review.md" "$QUEUE_DIR/$slug.review.md"
      verdict_set "$slug" "READY" "$rounds"
      learn_event "check-spec" "$slug" "verdict=READY round=$rounds"
      # B3-R1: create the rails ticket AFTER the existing announce + mv to
      # queue/ (B3_TASK build order). Failure is a hard error (log + exit 1)
      # but gates ticket creation ONLY, never the file queue: the spec stays
      # in queue/ with its READY verdict and builds via the legacy path.
      ticket_id="$(ticket_create "$slug" "$QUEUE_DIR/$slug.md")" || {
        log_error "ticket creation failed for $slug ($QUEUE_DIR/$slug.md) — rails has no fallback (B3-R1); spec stays queued with READY verdict and builds legacy"
        echo "check-spec: $slug — READY, moved to queue/ and announced, but rails ticket creation failed; recorded in $ERRORS_LOG; aborting (ticket creation is a hard error)" >&2
        exit 1
      }
      verdict_merge "$slug" "{\"ticket_id\":\"$ticket_id\"}"
      # B3-R4: frontmatter `blocks: [<slug>, ...]` = the listed slugs block
      # this one (acceptance: this spec is absent from ready until each
      # blocker's ticket closes). Edges point blocker -> this ticket
      # (rails: from blocks to). Edge failures are logged, never fatal.
      blocks="$(spec_frontmatter_field "$QUEUE_DIR/$slug.md" blocks)"
      if [ -n "$blocks" ]; then
        while IFS= read -r blocker; do
          [ -n "$blocker" ] || continue
          blocker_id="$(ticket_find_by_label "spec:$blocker")"
          if [ -z "$blocker_id" ]; then
            log_error "B3-R4: $slug blocked by $blocker — no ticket with label spec:$blocker (not READY yet?); edge skipped"
            echo "check-spec: $slug — blocker '$blocker' has no ticket; dependency edge skipped (logged)" >&2
            continue
          fi
          if ! ticket_add_dependency "$blocker_id" "$ticket_id"; then
            log_error "B3-R4: dependency $blocker_id blocks $ticket_id rejected (409 cycle or rails error); spec $slug flagged"
            echo "check-spec: $slug — dependency edge from '$blocker' rejected (cycle?); flagged in $ERRORS_LOG" >&2
          fi
        done <<< "$blocks"
      fi
      echo "check-spec: $slug — READY, moved to queue/ and announced to $QUEUE_THREAD, ticket $ticket_id created"
      ;;
    NEEDS-WORK*)
      rounds=$((rounds + 1))
      findings="$(printf '%s\n' "$answer" | sed 's/^• //' | head -c 4000)"
      # C3-R1 artifact contract: verdict review records carry schema-versioned
      # frontmatter, written once when the record is created (rounds append).
      if [ ! -f "$SPECS_DIR/$slug.review.md" ]; then
        {
          printf '%s\n' '---'
          printf '%s\n' 'schema_version: 1'
          printf '%s\n' 'trust_tier: unverified'
          printf '%s\n' 'provenance_refs:'
          printf '  - docs/pipeline/specs/%s.md\n' "$slug"
          printf '%s\n' 'produced_by: check-spec.sh'
          printf 'produced_at: %s\n' "$(date -u +%FT%TZ)"
          printf '%s\n\n' '---'
        } > "$SPECS_DIR/$slug.review.md"
      fi
      {
        printf '\n## Review round %d (%s)\n\n' "$rounds" "$(date -u +%FT%TZ)"
        printf '%s\n' "$findings"
      } >> "$SPECS_DIR/$slug.review.md"
      if [ "$rounds" -eq 2 ]; then
        ingest_lesson "$slug" "$findings"
      fi
      if [ "$rounds" -ge "$MAX_ROUNDS" ]; then
        verdict_set "$slug" "STALLED" "$rounds"
        learn_event "check-spec" "$slug" "verdict=STALLED round=$rounds"
        echo "check-spec: $slug — NEEDS-WORK round $rounds; STALLED after $MAX_ROUNDS rounds"
      else
        verdict_set "$slug" "NEEDS-WORK" "$rounds"
        learn_event "check-spec" "$slug" "verdict=NEEDS-WORK round=$rounds"
        echo "check-spec: $slug — NEEDS-WORK round $rounds; findings appended to specs/$slug.review.md"
      fi
      ;;
    REJECT*)
      # Charter violation — final. Move the spec aside, record, and ingest the
      # violation immediately: this is the pipeline's taste signal.
      mkdir -p "$REJECTED_DIR"
      findings="$(printf '%s\n' "$answer" | sed 's/^• //' | head -c 4000)"
      mv "$spec" "$REJECTED_DIR/$slug.md"
      [ -f "$SPECS_DIR/$slug.review.md" ] && mv "$SPECS_DIR/$slug.review.md" "$REJECTED_DIR/$slug.review.md"
      verdict_set "$slug" "REJECT" "$rounds"
      learn_event "check-spec" "$slug" "verdict=REJECT round=$rounds (charter violation)"
      ingest_lesson "$slug (REJECTED — charter violation)" "$findings"
      echo "check-spec: $slug — REJECT (charter violation), moved to rejected/ and ingested to memory"
      ;;
    *)
      echo "check-spec: $slug — unparseable verdict line ('$first'); skipping (recorded nothing)"
      ;;
  esac
done

# M1-R2: reflection point — a check-spec run completed (advisory, never fatal).
if [ -x "$LEARN_REFLECT" ]; then
  LEARN_PIPELINE_DIR="$PIPELINE_DIR" "$LEARN_REFLECT" || true
fi

echo "check-spec: done — $checked spec(s) consulted"
