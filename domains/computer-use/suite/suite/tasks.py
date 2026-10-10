"""Task files: one JSON file per task under tasks/<os>/."""
from __future__ import annotations

import fnmatch
import json
from pathlib import Path

TASKS = Path(__file__).resolve().parent.parent / "tasks"
REQUIRED = ("id", "os", "apps", "setup", "goal", "inputs", "checker", "max_minutes", "tags")
OSES = ("mac", "linux", "windows")
STATUSES = ("ready", "needs_test_machine")


def load_all() -> list:
    out = []
    for f in sorted(TASKS.glob("*/*.json")):
        t = json.loads(f.read_text())
        t["_file"] = str(f.relative_to(TASKS.parent))
        out.append(t)
    return out


def validate(tasks: list) -> list:
    from .checkers import CHECKERS

    problems, seen = [], set()
    for t in tasks:
        tid = t.get("id", t["_file"])
        for k in REQUIRED:
            if k not in t:
                problems.append(f"{tid}: missing {k}")
        if tid in seen:
            problems.append(f"{tid}: duplicate id")
        seen.add(tid)
        if t.get("os") not in OSES:
            problems.append(f"{tid}: os must be one of {OSES}")
        if Path(t["_file"]).parent.name != t.get("os"):
            problems.append(f"{tid}: file is not under tasks/{t.get('os')}/")
        status = t.get("status", "ready")
        if status not in STATUSES:
            problems.append(f"{tid}: status must be one of {STATUSES}")
        if t.get("os") != "mac" and status == "ready":
            problems.append(f"{tid}: non-mac tasks stay needs_test_machine until a non-production test machine exists")
        if not t.get("checker"):
            problems.append(f"{tid}: needs at least one checker")
        for c in t.get("checker", []):
            if c.get("kind") not in CHECKERS:
                problems.append(f"{tid}: unknown checker kind {c.get('kind')}")
        if not isinstance(t.get("inputs"), dict):
            problems.append(f"{tid}: inputs must be an object of literal values")
    return problems


def select(tasks: list, pattern: str = "", os_name: str = "mac", tags: str = "") -> list:
    pats = [p.strip() for p in pattern.split(",") if p.strip()]
    want_tags = {x.strip() for x in tags.split(",") if x.strip()}
    out = []
    for t in tasks:
        if os_name and t["os"] != os_name:
            continue
        if pats and not any(fnmatch.fnmatch(t["id"], p) for p in pats):
            continue
        if want_tags and not want_tags & set(t["tags"]):
            continue
        out.append(t)
    return out
