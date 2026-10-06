#!/usr/bin/env bash
# audit-proposal.sh — artifact audit (M2-R2/R3/R4). The ONLY path by which
# the pipeline modifies its own prompts, rubrics, or playbook. For each
# docs/pipeline/proposals/*.md without a final verdict in
# docs/pipeline/proposals/verdicts.json:
#
#   assemble proposal-rubric.md + charter.md + the proposal, consult the
#   auditor (PROPOSAL_AUDIT_CMD test hook, else LEARN_CONSULT_CMD, else
#   `allternit-factory orchestration steer consult`), then:
#     ADOPT  -> data targets (.steering/prompt.md, docs/pipeline/playbook.md,
#               docs/pipeline/*-rubric.md — COMPUTED from the target, never
#               trusted from the consult): the proposal's fenced change is
#               appended under an `<!-- adopted from proposal <slug> -->`
#               marker and committed with the slug in the message
#               (`learn: adopt proposal <slug>`); code targets instead emit
#               docs/pipeline/proposals/tasks/<slug>-TASK.md for a future
#               executor (code goes through the full build pipeline, never
#               direct edits). Both record the outcome linkage (slug ->
#               commit) in docs/pipeline/outcomes.jsonl (M2-R4).
#     REVISE -> verdict recorded, findings appended to the proposal; nothing
#               applied; re-audited on a later run.
#     REJECT -> verdict recorded (final), rejection ingested to taste memory
#               (:3201, advisory); nothing applied.
# Empty/unparseable answer or transport failure: record nothing, continue —
# never wedge the run. verdicts.json uses MERGE semantics (B3 lesson).
set -uo pipefail

ROOT="${PROPOSAL_ROOT:-$(git rev-parse --show-toplevel)}"
PIPELINE_DIR="${PROPOSAL_AUDIT_DIR:-$ROOT/.pipeline}"
PROPOSALS_DIR="$PIPELINE_DIR/proposals"
TASKS_DIR="$PROPOSALS_DIR/tasks"
VERDICTS_FILE="$PROPOSALS_DIR/verdicts.json"
ERRORS_LOG="$PIPELINE_DIR/errors.log"
RUBRIC="${PROPOSAL_RUBRIC:-$PIPELINE_DIR/proposal-rubric.md}"
CHARTER="${PIPELINE_CHARTER:-$PIPELINE_DIR/charter.md}"
OUTCOMES="$PIPELINE_DIR/outcomes.jsonl"
MEMORY_URL="${TASTE_MEMORY_URL:-http://localhost:3201/api/ingest}"

log_error() {
  printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$1" >> "$ERRORS_LOG"
}

# ─── verdicts.json helpers (MERGE semantics, B3 lesson) ─────────────────────

verdict_get() { # verdict_get <slug> -> "verdict proposal_sha" (empty if none)
  python3 - "$VERDICTS_FILE" "$1" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    d = {}
v = d.get(sys.argv[2])
if v:
    print("%s %s" % (v.get("verdict", ""), v.get("proposal_sha", "")))
PY
}

verdict_set() { # verdict_set <slug> <verdict> <proposal-sha> — merge, never replace
  python3 - "$VERDICTS_FILE" "$1" "$2" "$3" <<'PY'
import json, sys, datetime, os
path, slug, verdict, sha = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
try:
    d = json.load(open(path))
except Exception:
    d = {}
rec = d.get(slug, {})
rec.update({
    "verdict": verdict,
    "proposal_sha": sha,
    "updated": datetime.datetime.now(datetime.timezone.utc).isoformat(),
})
d[slug] = rec
os.makedirs(os.path.dirname(path), exist_ok=True)
with open(path, "w") as f:
    json.dump(d, f, indent=2, sort_keys=True)
    f.write("\n")
PY
}

# ─── proposal parsing ───────────────────────────────────────────────────────

fm_field() { # fm_field <file> <key> -> single frontmatter value
  python3 - "$1" "$2" <<'PY'
import sys
path, key = sys.argv[1], sys.argv[2]
val = ""
try:
    lines = open(path, encoding="utf-8", errors="replace").read().split("\n")
except Exception:
    lines = []
if lines and lines[0].strip() == "---":
    for line in lines[1:]:
        if line.strip() == "---":
            break
        if line.startswith((" ", "\t")):
            continue
        k, sep, v = line.partition(":")
        if sep and k.strip() == key:
            val = v.strip().strip('"').strip("'")
print(val)
PY
}

extract_change() { # extract_change <file> -> fenced block under "## Proposed change"
  python3 - "$1" <<'PY'
import sys
try:
    lines = open(sys.argv[1], encoding="utf-8", errors="replace").read().split("\n")
except Exception:
    sys.exit(0)
in_section = False
in_fence = False
block = []
for line in lines:
    s = line.strip()
    if s.startswith("## "):
        in_section = s[3:].strip().lower() == "proposed change"
        continue
    if not in_section:
        continue
    if s.startswith("```"):
        if in_fence:
            break
        in_fence = True
        continue
    if in_fence:
        block.append(line)
print("\n".join(block))
PY
}

# Data targets are COMPUTED from the target path (M2-R3) — never trusted.
is_data_target() {
  case "$1" in
    .steering/prompt.md|docs/pipeline/playbook.md|docs/pipeline/*-rubric.md) return 0 ;;
    *) return 1 ;;
  esac
}

# ─── consult ────────────────────────────────────────────────────────────────

consult() { # consult <request-file> -> answer on stdout (may be empty)
  if [ -n "${PROPOSAL_AUDIT_CMD:-}" ]; then
    $PROPOSAL_AUDIT_CMD < "$1"
  elif [ -n "${LEARN_CONSULT_CMD:-}" ]; then
    $LEARN_CONSULT_CMD < "$1"
  else
    allternit-factory orchestration steer consult --cwd "$PWD" --prompt-file "$1"
  fi
}

# First line, bullet-stripped, CR-free, uppercase (check-spec convention).
verdict_of() {
  printf '%s\n' "$1" | sed 's/^• //' | head -1 | tr -d '\r' | tr '[:lower:]' '[:upper:]'
}

# ─── memory ingest (REJECT taste signal, advisory) ──────────────────────────

ingest_rejection() { # ingest_rejection <slug> <findings>
  local payload code
  payload=$(python3 -c 'import json,sys; print(json.dumps({"content":sys.argv[1]+"\n\n"+sys.argv[2],"source":"pipeline-proposal-audit","metadata":{"source":"pipeline-proposal-audit","trust_tier":"failed","provenance_ref":sys.argv[1]}}))' "proposal $1 (REJECTED)" "$2")
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 -X POST "$MEMORY_URL" \
    -H 'Content-Type: application/json' -d "$payload" 2>/dev/null)
  if [[ "$code" != 2* ]]; then
    log_error "proposal rejection ingest failed for $1 (HTTP ${code:-000}) — continuing (advisory)"
    echo "audit-proposal: memory unreachable for $1 — logged, continuing (advisory)"
  fi
}

# ─── outcome linkage (M2-R4) ────────────────────────────────────────────────

record_linkage() { # record_linkage <slug> <commit-ref>
  python3 - "$OUTCOMES" "$1" "$2" <<'PY'
import datetime, json, os, sys
path, slug, ref = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(os.path.dirname(path), exist_ok=True)
with open(path, "a") as f:
    f.write(json.dumps({
        "ts": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "slug": slug,
        "outcome": "adopted",
        "note": "commit " + ref,
    }) + "\n")
PY
}

# ─── main ───────────────────────────────────────────────────────────────────

if [ ! -d "$PROPOSALS_DIR" ] || ! ls "$PROPOSALS_DIR"/*.md >/dev/null 2>&1; then
  echo "audit-proposal: no proposals in $PROPOSALS_DIR; nothing to do"
  exit 0
fi

audited=0
for proposal in "$PROPOSALS_DIR"/*.md; do
  slug="$(basename "$proposal" .md)"
  prior="$(verdict_get "$slug")"
  prior_verdict="${prior%% *}"
  prior_sha="${prior##* }"
  cur_sha="$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$proposal")"
  # ADOPT and REJECT are final; REVISE is re-audited only after the proposal
  # actually changed (the verdict records the reviewed content's hash —
  # otherwise every run would re-audit and re-append findings forever).
  if [ "$prior_verdict" = "ADOPT" ] || [ "$prior_verdict" = "REJECT" ]; then
    continue
  fi
  if [ "$prior_verdict" = "REVISE" ] && [ "$prior_sha" = "$cur_sha" ]; then
    continue
  fi

  audited=$((audited + 1))
  request="$(mktemp -t audit-proposal)"
  {
    cat "$RUBRIC"
    if [ -f "$CHARTER" ]; then
      printf '\n\n=== PIPELINE CHARTER (docs/pipeline/charter.md) ===\n'
      cat "$CHARTER"
    fi
    printf '\n\n=== PROPOSAL UNDER AUDIT: docs/pipeline/proposals/%s.md ===\n' "$slug"
    cat "$proposal"
  } > "$request"

  answer="$(consult "$request")"
  rm -f "$request"

  if [ -z "${answer// /}" ]; then
    log_error "audit-proposal: $slug — empty consult answer or transport failure; skipping (recorded nothing)"
    echo "audit-proposal: $slug — empty consult answer or transport failure; skipping (recorded nothing)"
    continue
  fi

  first="$(verdict_of "$answer")"
  findings="$(printf '%s\n' "$answer" | sed 's/^• //' | head -c 4000)"

  case "$first" in
    ADOPT*)
      target="$(fm_field "$proposal" target_artifact)"
      if [ -z "$target" ]; then
        log_error "audit-proposal: $slug — ADOPT but no target_artifact in frontmatter; skipping"
        echo "audit-proposal: $slug — ADOPT but no target_artifact; skipping (logged)" >&2
        continue
      fi
      if is_data_target "$target"; then
        # Data target: append the fenced change under the adoption marker
        # (additive only — never a full-file rewrite).
        marker="<!-- adopted from proposal $slug -->"
        change="$(extract_change "$proposal")"
        if [ -z "${change// /}" ]; then
          log_error "audit-proposal: $slug — ADOPT but no fenced Proposed change block; skipping"
          echo "audit-proposal: $slug — ADOPT but no fenced Proposed change; skipping (logged)" >&2
          continue
        fi
        if [ -f "$ROOT/$target" ] && grep -qF "$marker" "$ROOT/$target"; then
          echo "audit-proposal: $slug — change already present in $target; committing verdict only"
        else
          {
            printf '\n%s (%s)\n\n' "$marker" "$(date -u +%F)"
            printf '%s\n' "$change"
          } >> "$ROOT/$target"
        fi
        msg="learn: adopt proposal $slug"
        git -C "$ROOT" add -- "$target" "docs/pipeline/proposals/$slug.md" >/dev/null 2>&1
        if ! git -C "$ROOT" commit -m "$msg" >/dev/null 2>&1; then
          log_error "audit-proposal: $slug — adopt commit failed; verdict NOT recorded (retry next run)"
          echo "audit-proposal: $slug — adopt commit failed; recorded in $ERRORS_LOG; skipping verdict" >&2
          continue
        fi
        ref="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null)"
        record_linkage "$slug" "$ref"
        verdict_set "$slug" "ADOPT" "$cur_sha"
        echo "audit-proposal: $slug — ADOPT, applied to $target ($msg @ $ref)"
      else
        # Code target: emit a task spec for a future executor — the full
        # build pipeline owns code changes, never this script.
        mkdir -p "$TASKS_DIR"
        task="$TASKS_DIR/$slug-TASK.md"
        {
          printf '# TASK — adopted proposal %s (code target: %s)\n\n' "$slug" "$target"
          printf 'Adopted by audit-proposal.sh (%s). Execute the proposal below through\n' "$(date -u +%FT%TZ)"
          printf 'the standard executor workflow (worktree, NOTES + sentinel, gate-reviewed commit).\n\n'
          printf -- '---\n\n'
          cat "$proposal"
        } > "$task"
        msg="learn: adopt proposal $slug (task spec)"
        git -C "$ROOT" add -- "docs/pipeline/proposals/tasks/$slug-TASK.md" "docs/pipeline/proposals/$slug.md" >/dev/null 2>&1
        if ! git -C "$ROOT" commit -m "$msg" >/dev/null 2>&1; then
          log_error "audit-proposal: $slug — task-spec commit failed; verdict NOT recorded (retry next run)"
          echo "audit-proposal: $slug — task-spec commit failed; recorded in $ERRORS_LOG; skipping verdict" >&2
          continue
        fi
        ref="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null)"
        record_linkage "$slug" "$ref"
        verdict_set "$slug" "ADOPT" "$cur_sha"
        echo "audit-proposal: $slug — ADOPT (code target), task spec at $task ($msg @ $ref)"
      fi
      ;;
    REVISE*)
      # Append findings FIRST, then record the hash of the resulting file —
      # otherwise the appended findings themselves would count as a revision
      # and every run would re-audit forever.
      {
        printf '\n\n## Audit findings (REVISE, %s)\n\n' "$(date -u +%FT%TZ)"
        printf '%s\n' "$findings"
      } >> "$proposal"
      cur_sha="$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$proposal")"
      verdict_set "$slug" "REVISE" "$cur_sha"
      echo "audit-proposal: $slug — REVISE; findings appended to the proposal, nothing applied"
      ;;
    REJECT*)
      verdict_set "$slug" "REJECT" "$cur_sha"
      ingest_rejection "$slug" "$findings"
      echo "audit-proposal: $slug — REJECT (final), recorded and ingested to taste memory"
      ;;
    *)
      echo "audit-proposal: $slug — unparseable verdict line ('$first'); skipping (recorded nothing)"
      ;;
  esac
done

echo "audit-proposal: done — $audited proposal(s) audited"
