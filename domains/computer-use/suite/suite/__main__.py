"""python -m suite {run,validate,list,report}

  run       reset → run → check → teardown for each selected task, recording
            the full trace under results/<run-id>/ (gitignored)
  validate  schema-check every task file
  list      the tasks and their status
  report    re-render the report from one or more result dirs
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

from . import actions, report
from .checkers import run_checker
from .planner import run_planner
from .stack import Stack
from .tasks import load_all, select, validate
from .toolset import Toolset
from .web import WebLog, WebServer

ROOT = Path(__file__).resolve().parent.parent
RESULTS = ROOT / "results"
SCRATCH_ROOT = Path.home() / "AllternitSuite"
V2_MEMBERS = {"read_ui", "act", "run_batch", "verify", "use_credential", "request_human"}
REPLAY_SKIP = {"screenshot", "zoom", "cursor_position", "read_ui"}


def openrouter_spent():
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        return None
    try:
        req = urllib.request.Request("https://openrouter.ai/api/v1/credits", headers={"authorization": f"Bearer {key}"})
        with urllib.request.urlopen(req, timeout=15) as r:
            return float(json.loads(r.read())["data"]["total_usage"])
    except Exception:
        return None


def latest_passing_trace(task_id: str, source: str):
    dirs = [Path(source)] if source else sorted(RESULTS.glob("*/"), reverse=True)
    for d in dirs:
        for f in sorted(d.glob(f"attempts/{task_id}/*/attempt.json"), reverse=True):
            a = json.loads(f.read_text())
            if a.get("passed") and a["mode"] != "replay" and a.get("steps"):
                return a
    return None


def replay(task: dict, ctx, source: str) -> dict:
    """Re-issue a passing run's computer steps through the toolset with zero
    model calls. Map versions are dropped so the executor checks freshness
    against the live map; a step that fails stops the replay."""
    t0 = time.monotonic()
    src = latest_passing_trace(task["id"], source)
    if not src:
        return {"steps": [], "planner_calls": 0, "tokens": {}, "cost_usd": 0.0, "wall_s": 0.0,
                "timed_out": False, "exit_code": None, "errors": ["no passing trace to replay"], "final_text": ""}
    steps, errors = [], []
    for s in src["steps"]:
        if s.get("status") != "completed" or s.get("tool") not in ("computer", "computer_v2") or s.get("member") in REPLAY_SKIP:
            continue
        inp = json.loads(json.dumps(s.get("input") or {}))
        inp.pop("version", None)
        body = ctx.toolset.call(s["member"], inp)
        ok = Toolset.ok(body)
        steps.append({"tool": s["tool"], "member": s["member"], "input": inp, "status": "completed" if ok else "error",
                      "ms": body.get("_ms"), "output": Toolset.text(body)[:2000]})
        if not ok:
            errors.append(f"replay step {s['member']} failed: {Toolset.text(body)[:200]}")
            break
    return {"steps": steps, "planner_calls": 0, "tokens": {}, "cost_usd": 0.0, "wall_s": round(time.monotonic() - t0, 2),
            "timed_out": False, "exit_code": 0, "errors": errors, "final_text": "", "replayed_from": src.get("dir")}


def _last_stderr_line(attempt_dir) -> str:
    try:
        lines = (Path(attempt_dir) / "planner.stderr.log").read_text(errors="replace").splitlines()
    except (OSError, TypeError):
        return "no stderr"
    return next((l.strip() for l in reversed(lines) if l.strip() and not l.startswith("Bun v")), "no stderr")


def failure_cause(result: dict, checks: list, setup_error) -> str:
    if setup_error:
        return f"setup: {setup_error[:80]}"
    if result.get("timed_out"):
        return "timeout"
    if result.get("errors"):
        return "planner error: " + result["errors"][0][:100]
    if not result.get("steps"):
        if result.get("exit_code") and not result.get("planner_calls"):
            return "planner failed to start: " + _last_stderr_line(result.get("dir"))[:100]
        return "no computer actions"
    failed_tools = [s for s in result["steps"] if s.get("status") == "error"]
    first_bad = next((c for c in checks if not c["ok"]), None)
    cause = f"outcome wrong ({first_bad['kind']}: {first_bad['detail'][:60]})" if first_bad else "unknown"
    if failed_tools:
        cause += f"; last tool error: {failed_tools[-1].get('member')}"
    return cause


def run_attempt(task, mode, model, k, stack, web, run_dir, args) -> dict:
    slug = model.replace("/", "_") if mode != "replay" else "none"
    attempt_dir = run_dir / "attempts" / task["id"] / f"{mode}-{slug}-{k}"
    attempt_dir.mkdir(parents=True, exist_ok=True)
    rec = {"task": task["id"], "mode": mode, "model": model if mode != "replay" else "none", "repeat": k,
           "dir": str(attempt_dir), "started": dt.datetime.now().isoformat(timespec="seconds")}
    needs = set(task.get("requires", []))
    if mode == "pixel" and needs & V2_MEMBERS:
        rec.update(skipped=True, reason=f"needs {sorted(needs & V2_MEMBERS)} (not in pixel mode)", passed=False, steps=[])
        (attempt_dir / "attempt.json").write_text(json.dumps(rec, indent=2))
        return rec
    toolset = Toolset(stack, run_id=f"suite-{run_dir.name}-{task['id']}-{mode}-{k}")
    ctx = actions.Context(task, SCRATCH_ROOT / task["id"], web, toolset, attempt_dir)
    setup_error, result = None, {}
    try:
        actions.setup(ctx)
    except Exception as e:
        setup_error = str(e)
    if not setup_error:
        subprocess.run(["screencapture", "-x", str(attempt_dir / "before.png")], capture_output=True)
        result = replay(task, ctx, args.replay_from) if mode == "replay" else run_planner(task, ctx, stack, model, mode, attempt_dir)
        subprocess.run(["screencapture", "-x", str(attempt_dir / "after.png")], capture_output=True)
        time.sleep(float(task.get("settle_s", 0.5)))
    checks = []
    if not setup_error:
        for c in task["checker"]:
            ok, detail = run_checker(ctx, c)
            checks.append({"kind": c["kind"], "ok": ok, "detail": detail})
    teardown_problems = actions.teardown(ctx)
    passed = bool(checks) and all(c["ok"] for c in checks) and not setup_error
    rec.update(result)
    rec.update(passed=passed, checks=checks, setup_error=setup_error, teardown_problems=teardown_problems)
    rec.setdefault("steps", [])
    rec.setdefault("planner_calls", 0)
    rec.setdefault("cost_usd", 0.0)
    rec.setdefault("wall_s", 0.0)
    if not passed:
        rec["failure_cause"] = failure_cause(rec, checks, setup_error)
    (attempt_dir / "attempt.json").write_text(json.dumps(rec, indent=2))
    return rec


def cmd_run(args) -> int:
    tasks = select(load_all(), args.tasks, args.os, args.tags)
    runnable = [t for t in tasks if t.get("status", "ready") == "ready"]
    for t in tasks:
        if t not in runnable:
            print(f"skip {t['id']}: {t.get('status')}")
    if not runnable:
        print("no runnable tasks selected")
        return 1
    run_id = args.run_id or dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    run_dir = RESULTS / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    stack = Stack(args.api, run_dir / "stack", args.api_bin)
    log = WebLog(run_dir / "web.jsonl")
    web = WebServer(log)
    attempts = []
    spent0 = openrouter_spent()
    try:
        members = stack.members()
        modes = [m.strip() for m in args.mode.split(",")]
        models = [m.strip() for m in args.model.split(",")] if args.model else []
        meta = {"run_id": run_id, "api": stack.api, "booted": stack.booted, "members": sorted(members), "modes": modes,
                "models": models, "repeat": args.repeat, "tasks": [t["id"] for t in runnable]}
        (run_dir / "run.json").write_text(json.dumps(meta, indent=2))
        for mode in modes:
            if mode == "subtask" and "run_subtask" not in members:
                print("mode subtask: this allternit-api has no run_subtask member yet (E2); skipped")
                continue
            if mode != "replay" and not models:
                raise SystemExit(f"mode {mode} needs --model provider/model")
            for model in (models if mode != "replay" else ["none"]):
                for t in runnable:
                    for k in range(1, args.repeat + 1):
                        rec = run_attempt(t, mode, model, k, stack, web, run_dir, args)
                        attempts.append(rec)
                        status = "SKIP" if rec.get("skipped") else ("PASS" if rec["passed"] else "FAIL")
                        print(f"{status} {t['id']} [{mode} {model} #{k}] calls={rec.get('planner_calls')} "
                              f"${rec.get('cost_usd', 0):.4f} {rec.get('wall_s')}s {rec.get('failure_cause', '')}", flush=True)
    finally:
        web.close()
        stack.close()
        spent1 = openrouter_spent()
        summary = report.summarize(attempts)
        summary["openrouter_spend_usd"] = None if spent0 is None or spent1 is None else round(spent1 - spent0, 4)
        (run_dir / "summary.json").write_text(json.dumps(summary, indent=2))
        md = report.render(summary, f"Computer-use suite run {run_id}")
        if summary["openrouter_spend_usd"] is not None:
            md += f"\nOpenRouter account spend during the run: ${summary['openrouter_spend_usd']}\n"
        (run_dir / "report.md").write_text(md)
        print(md)
        print(f"results: {run_dir}")
    return 0


def cmd_validate(_args) -> int:
    tasks = load_all()
    problems = validate(tasks)
    by_os = {}
    for t in tasks:
        key = f"{t['os']}/{t.get('status', 'ready')}"
        by_os[key] = by_os.get(key, 0) + 1
    print(f"{len(tasks)} tasks: {by_os}")
    for p in problems:
        print("PROBLEM", p)
    return 1 if problems else 0


def cmd_list(args) -> int:
    for t in select(load_all(), args.tasks, args.os, args.tags):
        print(f"{t['id']:40} {t['os']:8} {t.get('status', 'ready'):19} {','.join(t['tags'])}")
    return 0


def cmd_report(args) -> int:
    summary = report.summarize(report.load_attempts(args.dirs))
    print(report.render(summary, "Computer-use suite report"))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(prog="python -m suite")
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--tasks", default="", help="comma-separated ids or globs (default: all)")
    r.add_argument("--tags", default="")
    r.add_argument("--os", default="mac")
    r.add_argument("--model", default="", help="provider/model through gizzi; comma-separate for several")
    r.add_argument("--mode", default="read_ui", help="pixel, read_ui, subtask, replay (comma-separate for several)")
    r.add_argument("--repeat", type=int, default=1, help="3 for pass^3")
    r.add_argument("--api", default=os.environ.get("ALLTERNIT_SUITE_API"), help="a running allternit-api; default boots a private local stack")
    r.add_argument("--api-bin", default=None)
    r.add_argument("--replay-from", default="", help="results dir with the passing traces to replay (default: newest)")
    r.add_argument("--run-id", default="")
    sub.add_parser("validate")
    li = sub.add_parser("list")
    li.add_argument("--tasks", default="")
    li.add_argument("--tags", default="")
    li.add_argument("--os", default="")
    rp = sub.add_parser("report")
    rp.add_argument("dirs", nargs="+")
    args = ap.parse_args()
    return {"run": cmd_run, "validate": cmd_validate, "list": cmd_list, "report": cmd_report}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
