#!/bin/bash
# Weekly photo-edition job: runs the photo-edition generator as a Codex
# Terminal bot in the Allternit Factory (local ChatGPT auth), which commits and
# pushes the artifacts. Scheduled via launchd (com.allternit.weekly-photo-editions).
#
# Flow: gizzi agents up (one Codex pane) → gizzi orchestration send (the job
# brief) → wait for the sentinel the brief asks the bot to write → gizzi
# orchestration capture (last lines into this job's log) → gizzi agents down.
#
# Logs:  ~/.allternit/factory/logs/weekly-photo-editions.log (+ pane capture)
# Notes: ~/.allternit/factory/evidence/weekly-photo-editions/NOTES.md
#
# Exit codes: 0 done, 3 pane gone or engine unreachable, 4 timeout after 2h.
set -uo pipefail

REPO=$HOME/Desktop/allternit-workspace/allternit
TEAM=weekly-photo-editions
BOT="photo@$TEAM"
ENV_FILE="$HOME/.config/allternit/photo-editions.env"
EVIDENCE_DIR="$HOME/.allternit/factory/evidence/$TEAM"
LOG_DIR="$HOME/.allternit/factory/logs"
LOG="$LOG_DIR/$TEAM.log"
SENTINEL="$EVIDENCE_DIR/done.sentinel"
TIMEOUT_S=7200
POLL_S=30

mkdir -p "$EVIDENCE_DIR" "$LOG_DIR"

# KIMI_API_KEY enables article-grounded photo briefs; without it the
# generator falls back to deterministic templates.
if [ -f "$ENV_FILE" ]; then
  set -a
  . "$ENV_FILE"
  set +a
fi

cd "$REPO" || exit 1
git pull --ff-only origin main || echo "$(date -Iseconds) WARN: git pull failed, continuing with local state"

# The team: one Codex Terminal bot. Written under the workspace's .allternit/
# (local data, git-ignored) so `gizzi agents up` finds it.
TEAM_DIR="$REPO/.allternit/teams/$TEAM"
if [ ! -f "$TEAM_DIR/team.yaml" ]; then
  mkdir -p "$TEAM_DIR"
  cat > "$TEAM_DIR/team.yaml" <<'YAML'
bots:
  - { bot: photo, role: build, binding: terminal, harness: codex }
YAML
fi

rm -f "$SENTINEL"

finish() {
  gizzi orchestration capture "$BOT" 200 >> "$LOG" 2>&1 || true
  gizzi agents down "$TEAM" >/dev/null 2>&1 || true
}

if ! gizzi agents up "$TEAM" >> "$LOG" 2>&1; then
  echo "$(date -Iseconds) weekly photo editions: could not start the team — see $LOG"
  exit 3
fi

BRIEF="$(cat docs/jobs/weekly-photo-editions.md)

When you have finished (committed and pushed, or stopped for a reason you wrote
in $EVIDENCE_DIR/NOTES.md), run: touch \"$SENTINEL\""

if ! gizzi orchestration send "$BOT" "$BRIEF" >> "$LOG" 2>&1; then
  echo "$(date -Iseconds) weekly photo editions: send failed — see $LOG"
  finish
  exit 3
fi

RC=4
waited=0
while [ "$waited" -lt "$TIMEOUT_S" ]; do
  if [ -f "$SENTINEL" ]; then RC=0; break; fi
  # The pane is gone when the bot no longer has a live pane (or the engine is down).
  if ! gizzi orchestration capture "$BOT" 1 >/dev/null 2>&1; then RC=3; break; fi
  sleep "$POLL_S"
  waited=$((waited + POLL_S))
done

finish

case $RC in
  0) echo "$(date -Iseconds) weekly photo editions: DONE" ;;
  3) echo "$(date -Iseconds) weekly photo editions: PANE-GONE — see $LOG" ;;
  4) echo "$(date -Iseconds) weekly photo editions: TIMEOUT after 2h — see $LOG" ;;
esac
exit $RC
