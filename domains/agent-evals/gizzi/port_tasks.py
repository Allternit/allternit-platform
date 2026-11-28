#!/usr/bin/env python3
"""Generate the shared Gizzi eval suite (shared.v1).

Reads the 10 subagent tasks and the fixture project by path (no install),
and writes suites/shared.v1/ next to this file. Output is byte-identical on
every run. `--check` regenerates into a temp dir and exits 1 on any difference.

Run it with: python3 -I domains/agent-evals/gizzi/port_tasks.py [--check]
"""
import gzip
import hashlib
import importlib.util
import io
import json
import sys
import tarfile
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVALS = HERE.parent / "subagents" / "evals"
SUITE_ID = "shared.v1"
SUITE_DIR = HERE / "suites" / SUITE_ID
MTIME = 946684800  # 2000-01-01, fixed
DEV_COUNT = 7  # tasks 1-7 dev, 8-10 holdout
UNITTEST = "python3 -m unittest discover -s tests"
HIDDEN_PATTERN = ' -p "test_hidden_*.py"'


def load(name):
    spec = importlib.util.spec_from_file_location("shared_" + name, EVALS / (name + ".py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def case_id(task_id):
    return "case_shared_" + task_id.replace("-", "_")


def workspace_files(fixture, task):
    files = dict(fixture.FILES)
    setup = task.get("setup") or {}
    files.update(setup.get("files", {}))
    for rep in setup.get("replace", []):
        text = files[rep["file"]]
        if rep["old"] not in text:
            raise SystemExit("setup replace target missing in %s: %s" % (rep["file"], task["id"]))
        files[rep["file"]] = text.replace(rep["old"], rep["new"])
    return files


def build_archive(files):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for path in sorted(files):
            data = files[path].encode("utf-8")
            info = tarfile.TarInfo(path)
            info.size = len(data)
            info.mtime = MTIME
            info.mode = 0o644
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            tar.addfile(info, io.BytesIO(data))
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode="wb", mtime=0, compresslevel=9) as gz:
        gz.write(raw.getvalue())
    digest = hashlib.sha1()
    for path in sorted(files):
        digest.update(path.encode() + b"\0" + hashlib.sha1(files[path].encode()).digest())
    return out.getvalue(), digest.hexdigest()


def map_checks(task):
    hidden = task.get("hidden") or {}
    out = []
    for c in task["checks"]:
        t = c["type"]
        if t == "tests":
            cmd = UNITTEST + (HIDDEN_PATTERN if c.get("only") == "hidden" else "")
            check = {"type": "tests", "command": cmd}
            if hidden:
                check["files"] = dict(sorted(hidden.items()))
            out.append(check)
        elif t == "contains":
            for text in c.get("all", []):
                out.append({"type": "contains", "path": c["file"], "text": text, "ignore_case": True})
            if c.get("any"):
                out.append({"type": "contains", "path": c["file"], "text": "", "any": list(c["any"]), "ignore_case": True})
        elif t == "absent":
            for text in c["none"]:
                out.append({"type": "absent", "path": c["file"], "text": text, "ignore_case": True})
        elif t == "unchanged":
            for f in c["files"]:
                out.append({"type": "unchanged", "path": f})
        elif t == "exists":
            out.append({"type": "exists", "path": c["file"]})
        else:
            raise SystemExit("unknown check type %s in %s" % (t, task["id"]))
    return out


def origin(sid):
    return {"session_id": sid, "through_message_id": None, "saved_at": 0, "gizzi_version": SUITE_ID, "surface": "cli"}


NO_REDACTION = {"secrets": 0, "paths": 0, "entries_dropped": 0}


def task_case(fixture, task, index):
    archive, snapshot = build_archive(workspace_files(fixture, task))
    prompt = task["prompt"]
    case = {
        "schema": "allternit.gizzi_eval_case.v1",
        "id": case_id(task["id"]),
        "suite": SUITE_ID,
        "title": prompt[:80],
        "label": "good",
        "note": "Ported from the subagent task '%s' (skill: %s)." % (task["id"], task["skill"]),
        "split": "dev" if index < DEV_COUNT else "holdout",
        "origin": origin(task["id"]),
        "turns": [{"role": "user", "text": prompt}],
        "workspace": {"kind": "archive", "path": "workspace.tar.gz", "bytes": len(archive), "snapshot": snapshot},
        "checks": map_checks(task),
        "effects": "block",
        "graders": [{"type": "status", "equals": "completed"}, {"type": "verifier_pass"}],
        "k": 3,
        "pass_rule": {"n_of_k": 2},
        "recorded": None,
        "needs_check": False,
        "redaction": dict(NO_REDACTION),
    }
    return case, archive


# ── synthetic session cases (hand-written) ──────────────────────────────────
def entry(seq, sid, kind, time, data, msg="msg_a1", part=None):
    e = {"sequence": seq, "sessionID": sid, "kind": kind, "messageID": msg, "data": data, "time": time}
    if part:
        e["partID"] = part
    return e


def tool_part(seq, sid, time, tool, status, pid):
    return entry(seq, sid, "part.updated", time, {"type": "tool", "tool": tool, "state": {"status": status}}, part=pid)


def tokens(inp, out):
    return {"input": inp, "output": out, "reasoning": 0, "cache": {"read": 0, "write": 0}}


def completed(seq, sid, time, inp, out, cost):
    return entry(seq, sid, "request.completed", time, {
        "requestID": "msg_u1", "providerID": "demo", "modelID": "demo-small", "attempt": 1,
        "finish": "stop", "tokens": tokens(inp, out), "cost": cost})


def started(seq, sid, time):
    return entry(seq, sid, "request.started", time, {"requestID": "msg_u1", "agent": "main"})


def session_case(key, label, title, prompt, trace, trajectory, graders, grades, quick=None, note=""):
    sid = "ses_shared_" + key
    passed = all(g["pass"] for g in grades)
    case = {
        "schema": "allternit.gizzi_eval_case.v1",
        "id": "case_shared_session_" + key,
        "suite": SUITE_ID,
        "title": title,
        "label": label,
        "note": note,
        "split": "dev",
        "origin": origin(sid),
        "turns": [{"role": "user", "text": prompt}],
        "workspace": {"kind": "none", "reason": "no_project"},
        "checks": [],
        "effects": "block",
        "graders": graders,
        "k": 1,
        "pass_rule": "all_of_k",
        "recorded": {"trajectory": trajectory, "grades": grades, "passed": passed},
        "needs_check": (label == "good" and not passed) or (label == "bad" and passed),
        "redaction": dict(NO_REDACTION),
    }
    if quick:
        case["quick_check"] = quick
    return case, trace(sid)


def session_cases():
    out = []
    base = [{"type": "status", "equals": "completed"}]

    # 1. good: reads a file and answers; passes.
    def good_trace(sid):
        return [
            started(1, sid, 1000),
            tool_part(2, sid, 1500, "read", "completed", "prt_1"),
            tool_part(3, sid, 2200, "grep", "completed", "prt_2"),
            completed(4, sid, 3000, 1800, 240, 0.004),
        ]
    good_traj = {
        "status": "completed", "verifier": None,
        "steps": [{"node": "main", "primitive": "read", "ok": True}, {"node": "main", "primitive": "grep", "ok": True}],
        "effects": [], "texts": ["load_items is called by summary and low_stock."],
        "cost_usd": 0.004, "tokens": 2040, "latency_ms": 2000,
    }
    out.append(session_case(
        "good", "good", "Which functions call load_items?", "Which functions call load_items?",
        good_trace, good_traj,
        base + [{"type": "cost_ceiling", "max_usd": 0.008}, {"type": "latency_ceiling", "max_ms": 4000}],
        [{"type": "status", "pass": True, "detail": "status completed"},
         {"type": "cost_ceiling", "pass": True, "detail": "cost 0.004 <= 0.008"},
         {"type": "latency_ceiling", "pass": True, "detail": "latency 2000 <= 4000"}],
        note="Synthetic. A short read-only answer that passes every grader."))

    # 2. bad: an edit fails and the run ends failed.
    def bad_trace(sid):
        return [
            started(1, sid, 1000),
            tool_part(2, sid, 1800, "read", "completed", "prt_1"),
            tool_part(3, sid, 2600, "edit", "error", "prt_2"),
            entry(4, sid, "request.failed", 3200, {
                "requestID": "msg_u1", "providerID": "demo", "modelID": "demo-small", "attempt": 1,
                "error": {"name": "ToolError", "message": "edit failed: old text not found"}}),
        ]
    bad_traj = {
        "status": "failed", "verifier": None,
        "steps": [{"node": "main", "primitive": "read", "ok": True}, {"node": "main", "primitive": "edit", "ok": False}],
        "effects": [{"node": "main", "tool": "edit", "committed": False}],
        "cost_usd": 0.0, "tokens": 0, "latency_ms": 2200,
    }
    out.append(session_case(
        "bad", "bad", "Fix the total_quantity off-by-one in store.py", "Fix the total_quantity off-by-one in store.py",
        bad_trace, bad_traj,
        base + [{"type": "forbidden_step", "steps": ["edit"]}],
        [{"type": "status", "pass": False, "detail": "status failed, wanted completed"},
         {"type": "forbidden_step", "pass": False, "detail": "forbidden step edit was used"}],
        quick=["broke_file"],
        note="Synthetic. The edit tool failed, so the run ended failed. Both graders fail."))

    # 3. guardrail: stopped after too many steps.
    def guard_trace(sid):
        return [
            started(1, sid, 1000),
            tool_part(2, sid, 1400, "grep", "completed", "prt_1"),
            tool_part(3, sid, 1800, "grep", "completed", "prt_2"),
            tool_part(4, sid, 2200, "grep", "completed", "prt_3"),
            entry(5, sid, "guardrail.tripped", 2300, {
                "kind": "max_steps", "reason": "step limit reached", "limit": 3, "observed": 3, "tool": "grep", "version": "1"}),
            completed(6, sid, 2500, 3200, 90, 0.006),
        ]
    guard_traj = {
        "status": "stopped", "verifier": None, "stopped_at": "max_steps",
        "steps": [{"node": "main", "primitive": "grep", "ok": True}] * 3,
        "effects": [], "cost_usd": 0.006, "tokens": 3290, "latency_ms": 1500,
    }
    out.append(session_case(
        "guardrail", "bad", "Find every TODO in the repo", "Find every TODO in the repo",
        guard_trace, guard_traj,
        base + [{"type": "stopped_at", "step": "max_steps"}],
        [{"type": "status", "pass": False, "detail": "status stopped, wanted completed"},
         {"type": "stopped_at", "pass": True, "detail": "stopped at max_steps"}],
        quick=["too_slow"],
        note="Synthetic. A guardrail stopped the run at the step limit."))
    return out


# ── write ───────────────────────────────────────────────────────────────────
def dumps(obj):
    return (json.dumps(obj, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def generate():
    """Return {relative path under SUITE_DIR: bytes}."""
    fixture, tasks = load("fixture"), load("tasks")
    files, refs = {}, []
    for i, task in enumerate(tasks.TASKS):
        case, archive = task_case(fixture, task, i)
        base = "cases/%s/" % case["id"]
        files[base + "case.json"] = dumps(case)
        files[base + "workspace.tar.gz"] = archive
        refs.append({"id": case["id"], "source": {"kind": "workspace_task", "path": base + "case.json"}})
    for case, trace in session_cases():
        base = "cases/%s/" % case["id"]
        files[base + "case.json"] = dumps(case)
        files[base + "trace.json"] = dumps(trace)
        refs.append({"id": case["id"], "source": {"kind": "session", "path": base + "case.json"}})
    files["suite.json"] = dumps({
        "schema": "allternit.eval_suite.v1", "id": SUITE_ID, "title": "Shared Gizzi suite",
        "runner": "gizzi", "cases": refs})
    return files


def tree(root):
    root = Path(root)
    return {str(p.relative_to(root)): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()}


def write(files, root):
    for rel, data in files.items():
        p = Path(root) / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)


def main(argv):
    files = generate()
    if "--check" in argv:
        with tempfile.TemporaryDirectory() as tmp:
            write(files, tmp)
            fresh, committed = tree(tmp), tree(SUITE_DIR) if SUITE_DIR.exists() else {}
        bad = sorted(k for k in set(fresh) | set(committed) if fresh.get(k) != committed.get(k))
        for k in bad:
            print("differs: " + k)
        print("shared.v1 is %s" % ("out of date" if bad else "up to date"))
        return 1 if bad else 0
    write(files, SUITE_DIR)
    print("wrote %d files to %s" % (len(files), SUITE_DIR))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
