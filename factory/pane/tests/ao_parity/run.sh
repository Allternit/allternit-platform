#!/usr/bin/env bash
# Golden side-by-side parity test: ao binary vs the ao-* bash scripts.
#
# Runs a scripted spawn -> send -> watch -> status -> kill scenario (plus guard
# cases) through BOTH implementations and asserts byte-identical stdout/stderr
# and exit codes, modulo the documented normalizations:
#   1. transcript log timestamps (ao-<slug>-<YYYYMMDD-HHMMSS>.log -> TS)
#   2. ao-doctor's extra `ao-engine:` transport line (asserted separately)
#   3. PANE-DEAD timing tolerance = watch poll interval (both worlds poll;
#      asserted as exit code + identical message, not wall-clock instant)
#   4. spawn-exit tolerance: the instant-exit probe fires at +0.5s in both
#      worlds; the script world's script(1)+tmux teardown can keep the session
#      observable past the probe while the engine's pane is already gone.
#      Asserted as: both agree strictly, OR script exit 0 + session line AND
#      ao exit 1 + "exited immediately" error whose transcript tail carries the
#      fixture marker. ao is deterministic-strict; the wrapper is timing-flaky.
#
#
# Spawn gate (audit S1): when the script world ships ao-spawn-gate, both worlds
# also spawn a stub claude/codex/sh through the gate (fake allternit-commrails
# via ALLTERNIT_COMMRAILS_BIN) and must agree byte-for-byte on stdout/stderr/
# exit, the gated runner line, the hook settings file and the spawn-gate.log
# label; a missing commrails binary must refuse the claude spawn identically.
# The rewrite table itself is covered by gate_parity.sh.
#
# Usage: run.sh [-v]   (builds the engine first unless AO_BIN is set;
#                       AO_SCRIPTS_DIR overrides the script world's directory)
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORKSPACE_ROOT="$(cd "$REPO_ROOT/.." && pwd)"
AO_BIN="${AO_BIN:-${CARGO_TARGET_DIR:-$WORKSPACE_ROOT/target}/debug/allternit-factory-pane}"
SCRIPTS="${AO_SCRIPTS_DIR:-$HOME/.claude/skills/agent-orchestrator/scripts}"
VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1

if [ ! -x "$AO_BIN" ]; then
  echo "building ao engine (ZIG=$ZIG)..." >&2
  (cd "$WORKSPACE_ROOT" && export ZIG="${ZIG:-/opt/homebrew/opt/zig@0.15/bin/zig}" && cargo build -p allternit-factory-pane --bin allternit-factory-pane) >&2
fi
for s in ao-spawn ao-send ao-watch ao-status ao-kill ao-doctor; do
  [ -x "$SCRIPTS/$s" ] || { echo "SKIP: $SCRIPTS/$s not found"; exit 2; }
done

TDIR="$(mktemp -d /tmp/ao-parity-XXXXXX)"
REPO="$TDIR/repo"
BURST_REPO="$TDIR/burst"
mkdir -p "$REPO" "$BURST_REPO"
# Helper agents as script files: the bash contract reassembles argv into a
# shell runner line ("$*"), so embedded shell syntax would diverge; real
# orchestrator usage passes simple argv (kimi --yolo, codex exec, ...).
printf '#!/bin/sh\necho AGENT-READY\nexec cat\n' > "$TDIR/agent-cat.sh"
EXIT_MARKER="DIED-IMMEDIATELY-LINE"
printf '#!/bin/sh\necho %s\nexit 7\n' "$EXIT_MARKER" > "$TDIR/agent-exit.sh"
printf '#!/bin/sh\necho WT-AGENT-READY\nexec cat\n' > "$TDIR/agent-wt.sh"
printf '#!/bin/sh\ni=0\nwhile [ $i -lt 2000 ]; do echo "burst-line-$i-payload-abcdefghijklmnopqrstuvwxyz0123456789"; i=$((i+1)); done\necho BURST-END\nsleep 300\n' > "$TDIR/agent-burst.sh"
GATE_CASES=0
[ -f "$SCRIPTS/ao-spawn-gate" ] && GATE_CASES=1
GATEBIN="$TDIR/gatebin"
mkdir -p "$GATEBIN"
# Fake allternit-commrails: `hook claude-settings --out F` writes its argv to F.
# The engine world calls the gate as `allternit-factory internal hook --root R
# <verb> …`; the fake rewrites that to the script's `--root R hook <verb> …` so
# both worlds record the same argv.
cat > "$GATEBIN/allternit-commrails" <<'FAKE'
#!/bin/sh
if [ "${1:-} ${2:-}" = "internal hook" ]; then
  shift 2; r1=$1; r2=$2; shift 2; set -- "$r1" "$r2" hook "$@"
fi
out=""; prev=""
for a in "$@"; do [ "$prev" = "--out" ] && out=$a; prev=$a; done
case " $* " in
  *" hook claude-settings "*) printf '{"fake-settings":"%s"}\n' "$*" > "$out" ;;
  *" hook spawn-check "*) exit 0 ;;
  *) exit 64 ;;
esac
FAKE
# Stub harnesses named like the real ones (absolute paths: tmux panes do not
# inherit this shell's PATH).
printf '#!/bin/sh\necho "STUB-ARGS: $*"\nexec cat\n' > "$GATEBIN/claude"
printf '#!/bin/sh\necho "STUB-ARGS: $*"\nexec cat\n' > "$GATEBIN/codex"
chmod +x "$GATEBIN/allternit-commrails" "$GATEBIN/claude" "$GATEBIN/codex"
GATE_LOG="$HOME/.agent-orchestrator/logs/spawn-gate.log"
PASS=0; FAIL=0

cleanup() {
  "$SCRIPTS/ao-kill" gold 2>/dev/null; "$AO_BIN" kill gold 2>/dev/null
  "$SCRIPTS/ao-kill" goldwt 2>/dev/null; "$AO_BIN" kill goldwt 2>/dev/null
  "$SCRIPTS/ao-kill" golddx 2>/dev/null; "$AO_BIN" kill golddx 2>/dev/null
  for g in gategc gategx gateun gatemiss; do "$SCRIPTS/ao-kill" $g >/dev/null 2>&1; "$AO_BIN" kill $g >/dev/null 2>&1; done
  if [ $FAIL -gt 0 ]; then
    local ev="$HOME/.agent-orchestrator/evidence/ao-parity-$(date +%Y%m%d-%H%M%S)"
    cp -R "$TDIR" "$ev" && echo "evidence preserved: $ev"
  fi
  rm -rf "$TDIR"
}
trap cleanup EXIT

# impl_run <impl:script|ao> <outdir> — executes the scripted scenario.
impl_run() {
  local impl=$1 out=$2
  mkdir -p "$out"
  local spawn send watch status kill doctor
  if [ "$impl" = script ]; then
    spawn="$SCRIPTS/ao-spawn"; send="$SCRIPTS/ao-send"; watch="$SCRIPTS/ao-watch"
    status="$SCRIPTS/ao-status"; kill="$SCRIPTS/ao-kill"; doctor="$SCRIPTS/ao-doctor"
  else
    spawn="$AO_BIN spawn"; send="$AO_BIN send"; watch="$AO_BIN watch"
    status="$AO_BIN status"; kill="$AO_BIN kill"; doctor="$AO_BIN doctor"
  fi
  local SLUG=gold S=$REPO/sentinel-gold

  # 1. spawn (long-lived cat agent)
  $spawn gold "$REPO" sh "$TDIR/agent-cat.sh" > "$out/spawn.out" 2> "$out/spawn.err"; echo $? > "$out/spawn.code"
  # 2. duplicate slug while alive
  $spawn gold "$REPO" /bin/true > "$out/dup.out" 2> "$out/dup.err"; echo $? > "$out/dup.code"
  # 3. send (verified paste)
  $send gold "run the golden parity check marker ALPHA999Zulu" > "$out/send.out" 2> "$out/send.err"; echo $? > "$out/send.code"
  # 4. status summary + tail
  $status > "$out/status.out" 2> "$out/status.err"; echo $? > "$out/status.code"
  $status gold 8 > "$out/status-tail.out" 2> "$out/status-tail.err"; echo $? > "$out/status-tail.code"
  # 5. watch: TIMEOUT (sentinel absent), then DONE (sentinel appears)
  $watch gold "$S-absent" 2 1 > "$out/watch-timeout.out" 2> "$out/watch-timeout.err"; echo $? > "$out/watch-timeout.code"
  touch "$S"
  $watch gold "$S" 30 1 > "$out/watch-done.out" 2> "$out/watch-done.err"; echo $? > "$out/watch-done.code"
  # 6. watch: PANE-DEAD (session killed under the watcher; poll interval 1s)
  $spawn golddx "$REPO" /bin/sh -c 'sleep 300' > /dev/null 2>&1
  $watch golddx "$S-absent" 30 1 > "$out/watch-dead.out" 2> "$out/watch-dead.err" &
  local wpid=$!
  sleep 2
  if [ "$impl" = script ]; then tmux kill-session -t "=ao-golddx:" 2>/dev/null; else "$AO_BIN" kill golddx >/dev/null 2>&1; fi
  wait $wpid; echo $? > "$out/watch-dead.code"
  # 7. kill + status after kill + second kill (already gone)
  $kill gold > "$out/kill.out" 2> "$out/kill.err"; echo $? > "$out/kill.code"
  $status > "$out/status-after.out" 2> "$out/status-after.err"; echo $? > "$out/status-after.code"
  $kill gold > "$out/kill-again.out" 2> "$out/kill-again.err"; echo $? > "$out/kill-again.code"
  # 8. instant-exit detection
  $spawn golddx "$REPO" sh "$TDIR/agent-exit.sh" > "$out/spawn-exit.out" 2> "$out/spawn-exit.err"; echo $? > "$out/spawn-exit.code"
  $kill golddx > /dev/null 2>&1
  # 9. usage errors
  $spawn onlyone > "$out/usage-spawn.out" 2> "$out/usage-spawn.err"; echo $? > "$out/usage-spawn.code"
  $send gold > "$out/usage-send.out" 2> "$out/usage-send.err"; echo $? > "$out/usage-send.code"
  $watch gold > "$out/usage-watch.out" 2> "$out/usage-watch.err"; echo $? > "$out/usage-watch.code"
  $kill > "$out/usage-kill.out" 2> "$out/usage-kill.err"; echo $? > "$out/usage-kill.code"
  # 10. send to missing session
  $send nosuchsession hello marker > "$out/send-missing.out" 2> "$out/send-missing.err"; echo $? > "$out/send-missing.code"
  $status nosuchsession > "$out/status-missing.out" 2> "$out/status-missing.err"; echo $? > "$out/status-missing.code"
  # 11. doctor
  $doctor > "$out/doctor.out" 2> "$out/doctor.err"; echo $? > "$out/doctor.code"
  # 12. --worktree flow (per-world repo: the script keeps the ao/<slug> branch,
  # so a shared repo would false-fail the second world with "branch exists")
  local WTREPO="$TDIR/wtrepo-$impl"
  git init -q "$WTREPO"; git -C "$WTREPO" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init
  $spawn --worktree goldwt "$WTREPO" sh "$TDIR/agent-wt.sh" > "$out/spawn-wt.out" 2> "$out/spawn-wt.err"; echo $? > "$out/spawn-wt.code"
  $kill goldwt --rm-worktree > "$out/kill-wt.out" 2> "$out/kill-wt.err"; echo $? > "$out/kill-wt.code"
  git -C "$WTREPO" worktree list > "$out/wt-list.out" 2>&1
  git -C "$WTREPO" branch --list 'ao/goldwt' > "$out/wt-branch.out" 2>&1
  # 13. burst transcript byte-0 completeness (pty-visible burst; tee/script log
  # must contain every byte from byte 0). Expected stream generated locally.
  $spawn golddx "$BURST_REPO" sh "$TDIR/agent-burst.sh" > "$out/spawn-burst.out" 2> "$out/spawn-burst.err"
  # wait until the transcript holds the full burst before killing the agent
  # (script(1) buffers; killing mid-burst would truncate the script-world log)
  burst_log="$(awk '{print $3}' "$out/spawn-burst.out")"
  for _ in $(seq 1 60); do
    grep -q 'BURST-END' "$burst_log" 2>/dev/null && break
    sleep 0.5
  done
  sleep 0.5
  $kill golddx > /dev/null 2>&1
  { i=0; while [ $i -lt 2000 ]; do echo "burst-line-$i-payload-abcdefghijklmnopqrstuvwxyz0123456789"; i=$((i+1)); done; echo BURST-END; } > "$out/burst-expected.txt"
  # 14. spawn gate (only when the script world has it)
  if [ "$GATE_CASES" = 1 ]; then
    local L="$HOME/.agent-orchestrator/logs" before
    before=$(wc -l < "$GATE_LOG" 2>/dev/null || echo 0)
    ALLTERNIT_COMMRAILS_BIN="$GATEBIN/allternit-commrails" \
      $spawn gategc "$REPO" "$GATEBIN/claude" -p hi --dangerously-skip-permissions > "$out/gate-claude.out" 2> "$out/gate-claude.err"; echo $? > "$out/gate-claude.code"
    cp "$L/ao-gategc.cmd.sh" "$out/gate-claude.runner" 2>/dev/null
    cp "$L/ao-gategc.claude-settings.json" "$out/gate-claude.settings" 2>/dev/null
    sleep 1; $status gategc 5 > "$out/gate-claude.tail" 2>&1
    $kill gategc > /dev/null 2>&1
    ALLTERNIT_COMMRAILS_BIN="$GATEBIN/allternit-commrails" \
      $spawn gategx "$REPO" "$GATEBIN/codex" exec go --dangerously-bypass-approvals-and-sandbox > "$out/gate-codex.out" 2> "$out/gate-codex.err"; echo $? > "$out/gate-codex.code"
    cp "$L/ao-gategx.cmd.sh" "$out/gate-codex.runner" 2>/dev/null
    $kill gategx > /dev/null 2>&1
    $spawn gateun "$REPO" sh "$TDIR/agent-cat.sh" > "$out/gate-ungated.out" 2> "$out/gate-ungated.err"; echo $? > "$out/gate-ungated.code"
    cp "$L/ao-gateun.cmd.sh" "$out/gate-ungated.runner" 2>/dev/null
    $kill gateun > /dev/null 2>&1
    ALLTERNIT_COMMRAILS_BIN="$TDIR/no-such-commrails" \
      $spawn gatemiss "$REPO" "$GATEBIN/claude" --dangerously-skip-permissions > "$out/gate-missing.out" 2> "$out/gate-missing.err"; echo $? > "$out/gate-missing.code"
    $status gatemiss > "$out/gate-missing-status.out" 2>&1; echo $? >> "$out/gate-missing-status.out"
    tail -n +"$((before + 1))" "$GATE_LOG" 2>/dev/null | sed -E 's/^[0-9]{8}-[0-9]{6} //' > "$out/gate-log.out"
  fi
}

normalize() {
  # transcript timestamps, per-world worktree repos, git SHAs, /tmp symlink,
  # and the script-vs-subcommand name (ao-spawn vs `ao spawn`)
  sed -E 's/ao-[a-z0-9]+-[0-9]{8}-[0-9]{6}\.log/ao-TS.log/g; s#/private/tmp#/tmp#g' "$1" \
    | sed -E "s#$TDIR/wtrepo-(script|ao)#WTREPO#g" \
    | sed -E 's# [0-9a-f]{7,40} \[# [#' \
    | sed -E 's/ao-(spawn|send|watch|status|kill|doctor)/ao \1/g'
}

# status summaries list each world's own pre-existing sessions (tmux sessions
# vs engine workspaces); compare only the scenario sessions.
normalize_scenario() {
  normalize "$1" | grep '^ao-gold' || true
}

# tmux pads capture-pane output to the visible screen (24 rows) with trailing
# blanks; the engine viewport is 120x40 and pane.read reports content lines.
# Content lines are byte-identical; trailing blank padding is viewport-sized
# (documented in ALLTERNIT_RUNTIME_P1_NOTES.md), so strip trailing blanks.
normalize_tail() {
  normalize "$1" | perl -0pe 's/\n(\s*\n)*$/\n/'
}

echo "== golden parity: script world =="
impl_run script "$TDIR/script"
echo "== golden parity: ao world =="
impl_run ao "$TDIR/ao"

compare() {
  local name=$1 norm=${2:-normalize}
  local a b
  a="$($norm "$TDIR/script/$name")"; b="$($norm "$TDIR/ao/$name")"
  if [ "$a" = "$b" ]; then PASS=$((PASS+1)); [ $VERBOSE -eq 1 ] && echo "  ok: $name"
  else
    FAIL=$((FAIL+1)); echo "  MISMATCH: $name"
    diff <(printf '%s' "$a") <(printf '%s' "$b") | head -20 | sed 's/^/    /'
  fi
}

echo "== comparing outputs =="
for f in spawn.out spawn.err spawn.code dup.out dup.err dup.code \
         send.out send.err send.code \
         status.err status.code status-tail.err status-tail.code \
         watch-timeout.out watch-timeout.code watch-done.out watch-done.code \
         watch-dead.out watch-dead.err watch-dead.code \
         kill.out kill.err kill.code status-after.err status-after.code \
         kill-again.out kill-again.err kill-again.code \
         usage-spawn.err usage-spawn.code usage-send.err usage-send.code \
         usage-watch.err usage-watch.code usage-kill.err usage-kill.code \
         send-missing.err send-missing.code status-missing.err status-missing.code \
         spawn-wt.out spawn-wt.err spawn-wt.code kill-wt.out kill-wt.err kill-wt.code \
         wt-list.out wt-branch.out spawn-burst.out spawn-burst.err burst-expected.txt; do
  compare "$f"
done
if [ "$GATE_CASES" = 1 ]; then
  for f in gate-claude.out gate-claude.err gate-claude.code gate-claude.runner gate-claude.settings \
           gate-codex.out gate-codex.err gate-codex.code gate-codex.runner \
           gate-ungated.out gate-ungated.err gate-ungated.code gate-ungated.runner \
           gate-missing.out gate-missing.err gate-missing.code gate-missing-status.out gate-log.out; do
    compare "$f"
  done
  # The stub saw the gated argv, not the bypass flag (both worlds).
  for impl in script ao; do
    # (pane lines wrap, so join them before matching)
    if tr -d '\n' < "$TDIR/$impl/gate-claude.tail" | grep -q 'STUB-ARGS: -p hi --permission-mode acceptEdits --settings .*ao-gategc.claude-settings.json' \
       && ! grep -q 'dangerously' "$TDIR/$impl/gate-claude.runner" "$TDIR/$impl/gate-codex.runner" \
       && grep -q 'gate=ungated harness=sh' "$TDIR/$impl/gate-log.out" \
       && [ "$(cat "$TDIR/$impl/gate-missing.code")" = 1 ]; then
      PASS=$((PASS+1)); [ $VERBOSE -eq 1 ] && echo "  ok: spawn gate enforced ($impl)"
    else
      FAIL=$((FAIL+1)); echo "  MISMATCH: spawn gate not enforced ($impl)"
      cat "$TDIR/$impl/gate-claude.tail" "$TDIR/$impl/gate-claude.runner" "$TDIR/$impl/gate-log.out" 2>&1 | sed 's/^/    /' | head -20
    fi
  done
else
  echo "  SKIP: spawn-gate cases ($SCRIPTS has no ao-spawn-gate)"
fi
compare status.out normalize_scenario
compare status-after.out normalize_scenario
compare status-tail.out normalize_tail

# spawn-exit (instant-exit detection): strict parity if both worlds agree
# exactly; otherwise accept ONLY the documented timing divergence — script
# world exited 0 with its normal session-line stdout (tmux+script(1) teardown
# lingering past the +0.5s probe), ao world exited 1 with the identical
# "exited immediately" contract error on stderr AND that error's transcript
# tail carries the fixture marker. Anything else is still a hard failure.
compare_spawn_exit() {
  if cmp -s <(normalize "$TDIR/script/spawn-exit.out") <(normalize "$TDIR/ao/spawn-exit.out") \
     && cmp -s <(normalize "$TDIR/script/spawn-exit.err") <(normalize "$TDIR/ao/spawn-exit.err") \
     && cmp -s "$TDIR/script/spawn-exit.code" "$TDIR/ao/spawn-exit.code"; then
    PASS=$((PASS+3)); [ $VERBOSE -eq 1 ] && echo "  ok: spawn-exit (strict parity)"
    return
  fi
  local s_out s_code a_err a_code
  s_out="$(normalize "$TDIR/script/spawn-exit.out")"
  s_code="$(cat "$TDIR/script/spawn-exit.code")"
  a_err="$(normalize "$TDIR/ao/spawn-exit.err")"
  a_code="$(cat "$TDIR/ao/spawn-exit.code")"
  if [ "$s_code" = 0 ] \
     && printf '%s\n' "$s_out" | grep -Eq '^ao-golddx[[:space:]].*\.log$' \
     && [ "$a_code" = 1 ] \
     && printf '%s\n' "$a_err" | grep -q 'agent exited immediately' \
     && printf '%s\n' "$a_err" | grep -qF "$EXIT_MARKER"; then
    PASS=$((PASS+3)); echo "  ok: spawn-exit (documented timing divergence — ao strict, script lingered)"
  else
    FAIL=$((FAIL+1)); echo "  MISMATCH: spawn-exit"
    diff <(normalize "$TDIR/script/spawn-exit.out") <(normalize "$TDIR/ao/spawn-exit.out") | head -10 | sed 's/^/    /'
    diff <(normalize "$TDIR/script/spawn-exit.err") <(normalize "$TDIR/ao/spawn-exit.err") | head -10 | sed 's/^/    /'
    echo "    codes: script=$(cat "$TDIR/script/spawn-exit.code") ao=$(cat "$TDIR/ao/spawn-exit.code")"
  fi
}
compare_spawn_exit

# doctor: ao world carries one extra transport line; verify the rest is identical.
# P7 adds an `ao-doctor: harness` section (managed-dir/install health) — additive
# surface outside the P1 parity contract; strip it (to the next ao-doctor
# section header or EOF), like the ao-engine line.
grep -v 'ao-engine:' "$TDIR/ao/doctor.out" > "$TDIR/ao/doctor.stripped"
awk '/^ao-doctor: harness$/{skip=1;next} /^ao-doctor: /{skip=0} !skip' \
  "$TDIR/ao/doctor.stripped" > "$TDIR/ao/doctor.stripped2" \
  && mv "$TDIR/ao/doctor.stripped2" "$TDIR/ao/doctor.stripped"
if cmp -s <(normalize "$TDIR/script/doctor.out") <(normalize "$TDIR/ao/doctor.stripped") \
   && [ "$(cat "$TDIR/script/doctor.code")" = "$(cat "$TDIR/ao/doctor.code")" ]; then
  PASS=$((PASS+1)); echo "  ok: doctor (engine line stripped)"
else
  FAIL=$((FAIL+1)); echo "  MISMATCH: doctor"; diff <(normalize "$TDIR/script/doctor.out") <(normalize "$TDIR/ao/doctor.stripped") | head -10
fi
engine_line="$(grep 'ao-engine: OK' "$TDIR/ao/doctor.out" || true)"
case "$engine_line" in
  *"protocol 22"*) PASS=$((PASS+1)); echo "  ok: doctor ao-engine line ($engine_line)" ;;
  *) FAIL=$((FAIL+1)); echo "  MISMATCH: doctor ao-engine line: $engine_line" ;;
esac

# burst transcripts: extract each world's log path from its spawn output and
# assert byte-0 completeness against the locally generated expected stream,
# then assert tee-vs-scriptlog parity.
burst_script="$(awk '{print $3}' "$TDIR/script/spawn-burst.out")"
burst_ao="$(awk '{print $3}' "$TDIR/ao/spawn-burst.out")"
for impl in script ao; do
  log_var="burst_$impl"; log_path="${!log_var}"
  if [ -f "$log_path" ] && tr -d '\r' < "$log_path" | cmp -s - "$TDIR/$impl/burst-expected.txt"; then
    PASS=$((PASS+1)); echo "  ok: burst transcript byte-0 complete ($impl)"
  else
    FAIL=$((FAIL+1)); echo "  MISMATCH: burst transcript ($impl): ${log_path:-<no path>}"
  fi
done
if [ -n "$burst_script" ] && [ -n "$burst_ao" ] \
   && cmp -s <(tr -d '\r' < "$burst_script") <(tr -d '\r' < "$burst_ao"); then
  PASS=$((PASS+1)); echo "  ok: burst transcripts identical across worlds"
else
  FAIL=$((FAIL+1)); echo "  MISMATCH: burst transcripts differ across worlds"
fi

echo
echo "golden parity: $PASS passed, $FAIL failed"
[ $FAIL -eq 0 ]
