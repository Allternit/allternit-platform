"""Subagent evals: does a subagent type earn its place?

Runs the 10 golden tasks under three arms, each task N times:
  split   the 11 built-in types plus planner/debugger/docs-writer/scribe added
          back as subagents, with the old "delegate these" hint (the design
          before 2026-10-27)
  folded  the 11 built-in types; planning, debugging, docs and summaries are
          skills in the main loop (the current design)
  single  no subagents at all (the task tool is denied)

Every attempt starts from a fresh copy of the fixture project, is graded by
code (hidden tests and file checks), and records subagent calls, $ (parent and
child sessions), tokens and wall time.

  python3 -m evals --model anthropic/claude-sonnet-5-5 --arms folded,single --repeats 2
  python3 -m evals --tasks debug-from-log --arms split --repeats 1   # one paid attempt
  python3 -m evals --list
"""
from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

from .fixture import FILES
from .tasks import FOLDED_AGENTS, SPLIT_HINT, TASKS

REPO = Path(__file__).resolve().parents[4]
ARMS = ("split", "folded", "single")
RULES = "Work in the current directory. Nobody can answer questions during this run, so make reasonable choices and finish the task."


def gizzi_argv() -> list:
    custom = os.environ.get("ALLTERNIT_EVAL_GIZZI")
    if custom:
        return shlex.split(custom)
    src = REPO / "cmd/gizzi-code"
    if not (REPO / "node_modules/@allternit/gizzi-sdk").exists() or not (src / "packages/sdk/dist").exists():
        raise SystemExit(f"gizzi source at {src} is not set up: run `pnpm install --frozen-lockfile` at the repo root "
                         "and `node scripts/build.mjs` in cmd/gizzi-code/packages/sdk, "
                         "or set ALLTERNIT_EVAL_GIZZI to a gizzi command (it must include the 2026-10-27 skills)")
    return ["bun", str(src / "script/dev.mjs")]


def arm_config(arm: str) -> dict:
    if arm == "single":
        return {"permission": {"task": "deny"}}
    if arm == "folded":
        return {}
    agents = {}
    for name, a in FOLDED_AGENTS.items():
        agents[name] = {"mode": "subagent", "description": a["description"], "model": a["model"],
                        "prompt": a["prompt"], "permission": a["permission"]}
    return {"agent": agents}


def arm_prompt(arm: str, task: dict) -> str:
    # The split arm's tool description lists the four types again but carries
    # the new delegation rule, so the old hint goes in the prompt too.
    hint = f"\n\n{SPLIT_HINT}" if arm == "split" else ""
    return f"{task['prompt']}\n\n{RULES}{hint}"


def write_fixture(root: Path, task: dict) -> None:
    for rel, body in FILES.items():
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(body)
    setup = task.get("setup") or {}
    for rel, body in (setup.get("files") or {}).items():
        (root / rel).write_text(body)
    for r in setup.get("replace") or []:
        f = root / r["file"]
        text = f.read_text()
        assert r["old"] in text, f"setup replace missed in {r['file']}"
        f.write_text(text.replace(r["old"], r["new"]))


def expected(rel: str, task: dict) -> str:
    """The file as the task started it (fixture plus setup)."""
    body = FILES.get(rel, "")
    for r in (task.get("setup") or {}).get("replace") or []:
        if r["file"] == rel:
            body = body.replace(r["old"], r["new"])
    return body


def grade(root: Path, task: dict) -> list[dict]:
    for rel, body in (task.get("hidden") or {}).items():
        (root / rel).write_text(body)
    out = []
    for c in task["checks"]:
        t = c["type"]
        ok, note = False, ""
        if t == "tests":
            if c.get("only") == "hidden":
                mods = [rel[:-3].replace("/", ".") for rel in task.get("hidden") or {}]
                cmd = [sys.executable, "-m", "unittest", "-q", *mods]
            else:
                cmd = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-t", ".", "-q"]
            p = subprocess.run(cmd, cwd=root, capture_output=True, text=True, timeout=120)
            ok = p.returncode == 0
            note = "" if ok else (p.stderr or p.stdout).strip().splitlines()[-1:][0] if (p.stderr or p.stdout).strip() else f"exit {p.returncode}"
        elif t == "exists":
            ok = (root / c["file"]).exists()
        elif t in ("contains", "absent"):
            f = root / c["file"]
            text = f.read_text(errors="replace").lower() if f.exists() else None
            if text is None:
                note = "missing file"
            elif t == "contains":
                missing = [w for w in c.get("all", []) if w.lower() not in text]
                any_ok = not c.get("any") or any(w.lower() in text for w in c["any"])
                ok = not missing and any_ok
                note = f"missing {missing}" if missing else ("none of any" if not any_ok else "")
            else:
                found = [w for w in c["none"] if w.lower() in text]
                ok = not found
                note = f"found {found}" if found else ""
        elif t == "unchanged":
            changed = [rel for rel in c["files"] if (root / rel).read_text() != expected(rel, task)]
            ok = not changed
            note = f"changed {changed}" if changed else ""
        out.append({"type": t, "ok": ok, "note": note})
    return out


def export_cost(session_id: str, env: dict) -> tuple[float, dict]:
    p = subprocess.run(gizzi_argv() + ["export", session_id], env=env, capture_output=True, text=True, timeout=120)
    cost, tokens = 0.0, {"input": 0, "output": 0, "cache_read": 0}
    try:
        data = json.loads(p.stdout)
    except ValueError:
        return cost, tokens
    for m in data.get("messages", []):
        info = m.get("info") or {}
        if info.get("role") != "assistant":
            continue
        cost += float(info.get("cost") or 0)
        t = info.get("tokens") or {}
        tokens["input"] += t.get("input", 0)
        tokens["output"] += t.get("output", 0)
        tokens["cache_read"] += (t.get("cache") or {}).get("read", 0)
    return cost, tokens


def run_attempt(task: dict, arm: str, model: str, attempt_dir: Path, max_minutes: float) -> dict:
    root = attempt_dir / "work"
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    write_fixture(root, task)
    env = dict(os.environ, GIZZI_CONFIG_CONTENT=json.dumps(arm_config(arm)), GIZZI_SUBAGENT_SYNC_OFF="1", GIZZI_NO_BRAIN_AGENTS="1")
    argv = gizzi_argv() + ["run", "--format", "json", "--dir", str(root)]
    if model:
        argv += ["--model", model]
    argv.append(arm_prompt(arm, task))
    events = attempt_dir / "events.jsonl"
    t0 = time.monotonic()
    timed_out = False
    with open(events, "w") as out, open(attempt_dir / "stderr.log", "w") as err:
        p = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=err, cwd=str(root))
        try:
            p.wait(timeout=max_minutes * 60)
        except subprocess.TimeoutExpired:
            timed_out = True
            p.terminate()
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()
    wall = time.monotonic() - t0

    session_id, children, subagents, errors = None, [], [], []
    for line in events.read_text(errors="replace").splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        session_id = session_id or ev.get("sessionID")
        part = ev.get("part") or {}
        if ev.get("type") == "tool_use" and part.get("tool") == "task":
            st = part.get("state") or {}
            subagents.append((st.get("input") or {}).get("subagent_type"))
            child = (st.get("metadata") or {}).get("sessionId")
            if child:
                children.append(child)
        elif ev.get("type") == "error":
            errors.append(json.dumps(ev.get("error"))[:300])

    cost, tokens = (0.0, {})
    if session_id:
        cost, tokens = export_cost(session_id, env)
        for child in children:
            c, t = export_cost(child, env)
            cost += c
            for k, v in t.items():
                tokens[k] = tokens.get(k, 0) + v
    checks = grade(root, task)
    return {
        "task": task["id"], "skill": task["skill"], "arm": arm, "pass": all(c["ok"] for c in checks),
        "checks": checks, "subagents": subagents, "cost_usd": round(cost, 6), "tokens": tokens,
        "wall_s": round(wall, 1), "timed_out": timed_out, "exit": p.returncode, "errors": errors, "session_id": session_id,
    }


def report(rows: list[dict]) -> str:
    lines = ["| Task | Skill | " + " | ".join(ARMS) + " |", "|---|---|" + "---|" * len(ARMS)]
    for task in TASKS:
        cells = []
        for arm in ARMS:
            got = [r for r in rows if r["task"] == task["id"] and r["arm"] == arm]
            if not got:
                cells.append("–")
                continue
            passed = sum(r["pass"] for r in got)
            usd = sum(r["cost_usd"] for r in got) / len(got)
            subs = sum(len(r["subagents"]) for r in got) / len(got)
            cells.append(f"{passed}/{len(got)} · ${usd:.3f} · {subs:.1f} sub")
        lines.append(f"| {task['id']} | {task['skill']} | " + " | ".join(cells) + " |")
    lines += ["", "| Arm | Pass | Mean $ | Mean s | Subagent calls |", "|---|---|---|---|---|"]
    for arm in ARMS:
        got = [r for r in rows if r["arm"] == arm]
        if not got:
            continue
        lines.append(f"| {arm} | {sum(r['pass'] for r in got)}/{len(got)} | {sum(r['cost_usd'] for r in got) / len(got):.3f} | "
                     f"{sum(r['wall_s'] for r in got) / len(got):.0f} | {sum(len(r['subagents']) for r in got)} |")
    lines += ["", "Keep a skill folded when `folded` passes at least as often as `split` at lower or equal $.",
              "A kept subagent earns its place when `folded` beats `single` on pass rate or $ for its tasks."]
    return "\n".join(lines)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="evals", description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model", default="", help="provider/model through gizzi (default: gizzi's default model)")
    ap.add_argument("--arms", default=",".join(ARMS))
    ap.add_argument("--tasks", default="", help="comma-separated task ids (default: all)")
    ap.add_argument("--repeats", type=int, default=2, help="runs per task and arm (default 2: one run hides flaky regressions)")
    ap.add_argument("--max-minutes", type=float, default=8)
    ap.add_argument("--out", default="", help="results directory (default: runs/<timestamp>)")
    ap.add_argument("--list", action="store_true", help="list the tasks and exit")
    ap.add_argument("--report", default="", help="print the report for an existing results.jsonl and exit")
    ap.add_argument("--selftest", action="store_true", help="grade the untouched fixture (every task must fail) and exit; no model calls")
    a = ap.parse_args(argv)

    if a.list:
        for t in TASKS:
            print(f"{t['id']:22} {t['skill']:14} {t['prompt'][:80]}")
        return 0
    if a.selftest:
        import tempfile
        bad = []
        for t in TASKS:
            with tempfile.TemporaryDirectory() as tmp:
                write_fixture(Path(tmp), t)
                if all(c["ok"] for c in grade(Path(tmp), t)):
                    bad.append(t["id"])
        print("selftest: " + (f"these tasks pass with no work done: {bad}" if bad else f"all {len(TASKS)} tasks fail untouched, as they should"))
        return 1 if bad else 0
    if a.report:
        rows = [json.loads(l) for l in Path(a.report).read_text().splitlines() if l.strip()]
        print(report(rows))
        return 0

    arms = [x for x in a.arms.split(",") if x]
    bad = [x for x in arms if x not in ARMS]
    if bad:
        ap.error(f"unknown arm {bad}; pick from {ARMS}")
    want = {x for x in a.tasks.split(",") if x}
    tasks = [t for t in TASKS if not want or t["id"] in want]
    if want - {t["id"] for t in tasks}:
        ap.error(f"unknown task {sorted(want - {t['id'] for t in tasks})}")

    out = Path(a.out) if a.out else Path(__file__).resolve().parent.parent / "runs" / time.strftime("%Y%m%d-%H%M%S")
    out.mkdir(parents=True, exist_ok=True)
    results = out / "results.jsonl"
    total = len(tasks) * len(arms) * a.repeats
    print(f"{total} attempts → {out}", flush=True)
    rows, n = [], 0
    for task in tasks:
        for arm in arms:
            for i in range(a.repeats):
                n += 1
                d = out / task["id"] / f"{arm}-{i + 1}"
                d.mkdir(parents=True, exist_ok=True)
                r = run_attempt(task, arm, a.model, d, a.max_minutes)
                rows.append(r)
                with open(results, "a") as f:
                    f.write(json.dumps(r) + "\n")
                fails = "; ".join(f"{c['type']}: {c['note']}" for c in r["checks"] if not c["ok"])
                print(f"[{n}/{total}] {task['id']} {arm} #{i + 1}: {'PASS' if r['pass'] else 'FAIL'} "
                      f"${r['cost_usd']:.4f} {r['wall_s']}s subagents={r['subagents']} {fails}", flush=True)
    (out / "report.md").write_text(report(rows) + "\n")
    print("\n" + report(rows))
    return 0


if __name__ == "__main__":
    sys.exit(main())
