#!/usr/bin/env bash
# Sample CPU% and memory of the pod's processes every N seconds into a CSV.
# CPU% is measured from /proc/<pid>/stat deltas between samples (100 = one
# full core), not ps's lifetime average. Chrome is many processes: RSS is
# summed (overcounts shared pages), PSS (from smaps_rollup) is the honest
# memory number for cost.
#
# Usage:  bash stats.sh <out-dir> [interval-seconds=5]
# Writes: <out-dir>/stats.csv
# Columns: ts_ms, then for each group (chrome, node, ffmpeg, pulse):
#          <g>_procs, <g>_cpu_pct, <g>_rss_mb, <g>_pss_mb; then total_cpu_pct, total_pss_mb, load1

set -euo pipefail

OUT="${1:?usage: stats.sh <out-dir> [interval]}"
INTERVAL="${2:-5}"
mkdir -p "$OUT"
CSV="$OUT/stats.csv"
TCK="$(getconf CLK_TCK)"
GROUPS_LIST="chrome node ffmpeg pulse"

group_of() {
    case "$1" in
        chrome|chrome_crashpad*|google-chrome*) echo chrome ;;
        node) echo node ;;
        ffmpeg|parec) echo ffmpeg ;;
        pulseaudio) echo pulse ;;
        *) echo "" ;;
    esac
}

# Prints "pid group ticks rss_kb pss_kb" for every matching process.
snapshot() {
    local d pid comm g stat ticks rss pss
    for d in /proc/[0-9]*; do
        pid="${d#/proc/}"
        comm="$(cat "$d/comm" 2>/dev/null)" || continue
        g="$(group_of "$comm")"
        [ -n "$g" ] || continue
        stat="$(cat "$d/stat" 2>/dev/null)" || continue
        # Fields after the ")" of comm: utime is 14th, stime 15th overall.
        ticks="$(echo "${stat##*) }" | awk '{print $12 + $13}')"
        rss="$(awk '/^VmRSS:/{print $2}' "$d/status" 2>/dev/null)"
        pss="$(awk '/^Pss:/{print $2}' "$d/smaps_rollup" 2>/dev/null)"
        echo "$pid $g ${ticks:-0} ${rss:-0} ${pss:-0}"
    done
}

header="ts_ms"
for g in $GROUPS_LIST; do header="$header,${g}_procs,${g}_cpu_pct,${g}_rss_mb,${g}_pss_mb"; done
header="$header,total_cpu_pct,total_pss_mb,load1"
[ -s "$CSV" ] || echo "$header" > "$CSV"
echo "[stats] every ${INTERVAL}s -> $CSV (Ctrl-C to stop)"

prev="$(snapshot)"
prev_t="$(date +%s%N)"
while true; do
    sleep "$INTERVAL"
    cur="$(snapshot)"
    cur_t="$(date +%s%N)"
    load1="$(cut -d' ' -f1 /proc/loadavg)"
    row="$(awk -v tck="$TCK" -v dt_ns="$((cur_t - prev_t))" -v groups="$GROUPS_LIST" -v ts="$((cur_t / 1000000))" -v load1="$load1" '
        FNR == NR { pt[$1] = $3; next }               # previous snapshot: pid -> ticks
        {
            g = $2; n[g]++; rss[g] += $4; pss[g] += $5
            if ($1 in pt) d[g] += $3 - pt[$1]          # only pids alive in both samples
        }
        END {
            secs = dt_ns / 1e9; split(groups, gs, " "); out = ts; tc = 0; tp = 0
            for (i = 1; i <= length(gs); i++) {
                k = gs[i]; c = (d[k] / tck) / secs * 100; tc += c; tp += pss[k]
                out = out sprintf(",%d,%.1f,%.1f,%.1f", n[k], c, rss[k] / 1024, pss[k] / 1024)
            }
            print out sprintf(",%.1f,%.1f,%s", tc, tp / 1024, load1)
        }' <(echo "$prev") <(echo "$cur"))"
    echo "$row" >> "$CSV"
    prev="$cur"
    prev_t="$cur_t"
done
