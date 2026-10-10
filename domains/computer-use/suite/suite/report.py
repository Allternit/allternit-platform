"""Suite report: success, pass^3, planner calls, $ and time per mode and
model against the §8.1 bars, plus the determinism report for repeats."""
from __future__ import annotations

import json
import statistics
from collections import defaultdict
from pathlib import Path

BARS = {"success": 90.0, "pass3": 80.0, "calls": 3, "usd": 0.05, "seconds": 120}


def _median(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def signature(steps: list) -> list:
    """The step sequence with volatile parts (versions, coordinates jitter,
    timings) dropped: tool, member, op/key, and the target element or app."""
    sig = []
    for s in steps:
        i = s.get("input") or {}
        tgt = i.get("id") or i.get("app") or i.get("name") or ""
        op = i.get("op") or i.get("key") or ""
        if s.get("member") == "run_batch":
            op = ",".join((st.get("act") or {}).get("op", "") or ("pixel" if st.get("pixel") else "wait") for st in i.get("steps", []))
        sig.append(f"{s.get('tool')}.{s.get('member')}:{op}:{tgt}")
    return sig


def summarize(attempts: list) -> dict:
    groups = defaultdict(list)
    for a in attempts:
        groups[(a["mode"], a["model"])].append(a)
    rows, determinism = [], []
    for (mode, model), items in sorted(groups.items()):
        by_task = defaultdict(list)
        for a in items:
            by_task[a["task"]].append(a)
        ran = [a for a in items if not a.get("skipped")]
        if not ran:
            continue
        repeated = {t: xs for t, xs in by_task.items() if len([x for x in xs if not x.get("skipped")]) >= 3}
        pass3 = (100.0 * sum(all(x["passed"] for x in xs[:3]) for xs in repeated.values()) / len(repeated)) if repeated else None
        row = {
            "mode": mode,
            "model": model,
            "attempts": len(ran),
            "tasks": len({a["task"] for a in ran}),
            "success_pct": round(100.0 * sum(a["passed"] for a in ran) / len(ran), 1),
            "pass3_pct": None if pass3 is None else round(pass3, 1),
            "pass3_tasks": len(repeated),
            "median_planner_calls": _median([a["planner_calls"] for a in ran]),
            "median_usd": _median([a["cost_usd"] for a in ran]),
            "total_usd": round(sum(a["cost_usd"] for a in ran), 4),
            "median_seconds": _median([a["wall_s"] for a in ran]),
        }
        row["bars"] = {
            "success": row["success_pct"] >= BARS["success"],
            "pass3": None if pass3 is None else pass3 >= BARS["pass3"],
            "calls": row["median_planner_calls"] is not None and row["median_planner_calls"] <= BARS["calls"],
            "usd": row["median_usd"] is not None and row["median_usd"] <= BARS["usd"],
            "seconds": row["median_seconds"] is not None and row["median_seconds"] < BARS["seconds"],
        }
        rows.append(row)
        for task, xs in sorted(repeated.items()):
            sigs = [signature(x["steps"]) for x in xs if not x.get("skipped")]
            same = all(s == sigs[0] for s in sigs)
            div = None
            if not same:
                n = min(len(s) for s in sigs)
                div = next((i for i in range(n) if len({s[i] for s in sigs}) > 1), n)
            determinism.append({
                "mode": mode, "model": model, "task": task, "runs": len(sigs),
                "same_sequence": same, "diverged_at_step": div,
                "at_divergence": None if same else sorted({s[div] if div < len(s) else "<end>" for s in sigs}),
                "passes": [x["passed"] for x in xs],
            })
    failures = defaultdict(int)
    for a in attempts:
        if not a.get("skipped") and not a["passed"]:
            failures[a.get("failure_cause", "unknown")] += 1
    return {"rows": rows, "determinism": determinism, "failure_causes": dict(sorted(failures.items(), key=lambda x: -x[1]))}


def _fmt(v, suffix=""):
    return "–" if v is None else f"{v}{suffix}"


def render(summary: dict, title: str) -> str:
    mark = lambda ok: "–" if ok is None else ("✓" if ok else "✗")
    lines = [f"# {title}", "",
             f"Bars (§8.1): success ≥ {BARS['success']:.0f}%, pass^3 ≥ {BARS['pass3']:.0f}%, "
             f"≤ {BARS['calls']} planner calls, ≤ ${BARS['usd']}, < {BARS['seconds']} s per task (medians).", "",
             "| Mode | Model | Tasks | Runs | Success | pass^3 | Planner calls | $/task | Time/task | Total $ |",
             "|---|---|---|---|---|---|---|---|---|---|"]
    for r in summary["rows"]:
        b = r["bars"]
        lines.append(
            f"| {r['mode']} | {r['model']} | {r['tasks']} | {r['attempts']} | {r['success_pct']}% {mark(b['success'])} | "
            f"{_fmt(r['pass3_pct'], '%')} ({r['pass3_tasks']}) {mark(b['pass3'])} | {_fmt(r['median_planner_calls'])} {mark(b['calls'])} | "
            f"{_fmt(r['median_usd'] if r['median_usd'] is None else round(r['median_usd'], 4))} {mark(b['usd'])} | "
            f"{_fmt(r['median_seconds'], ' s')} {mark(b['seconds'])} | {r['total_usd']} |")
    if summary["determinism"]:
        d = summary["determinism"]
        same = sum(x["same_sequence"] for x in d)
        lines += ["", f"## Determinism: {same}/{len(d)} repeated tasks ran the same step sequence", "",
                  "| Mode | Model | Task | Same sequence | Diverged at | Steps there | Passes |", "|---|---|---|---|---|---|---|"]
        for x in d:
            lines.append(f"| {x['mode']} | {x['model']} | {x['task']} | {'yes' if x['same_sequence'] else 'no'} | "
                         f"{_fmt(x['diverged_at_step'])} | {'; '.join(x['at_divergence'] or [])[:160]} | {x['passes']} |")
    if summary["failure_causes"]:
        lines += ["", "## Failure causes", ""] + [f"- {k}: {v}" for k, v in summary["failure_causes"].items()]
    return "\n".join(lines) + "\n"


def load_attempts(run_dirs: list) -> list:
    out = []
    for d in run_dirs:
        for f in sorted(Path(d).glob("attempts/*/*/attempt.json")):
            out.append(json.loads(f.read_text()))
    return out
