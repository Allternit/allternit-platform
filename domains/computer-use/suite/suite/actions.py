"""Deterministic setup and teardown.

Setup builds a known start state from scratch files, local pages and app
windows the suite opens itself. Teardown closes only what the suite opened
(an app the person already had running keeps running; only the suite's own
documents and windows are closed, without saving), removes scratch files and
deletes the suite's Keychain items. Nothing touches real user data.
"""
from __future__ import annotations

import secrets
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any, Optional

KEYCHAIN_SERVICE = "allternit.computer"  # what computer_v2's Keychain backend reads


def osa(script: str, timeout: float = 20) -> str:
    r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip() or f"osascript failed: {script[:80]}")
    return r.stdout.strip()


def app_running(app: str) -> bool:
    try:
        return osa(f'application "{app}" is running') == "true"
    except Exception:
        return False


class Context:
    """Per-attempt state shared by setup, the planner, checkers and teardown."""

    def __init__(self, task: dict, scratch: Path, web, toolset, run_dir: Path):
        self.task = task
        self.scratch = scratch
        self.web = web
        self.toolset = toolset
        self.run_dir = run_dir
        self.running_before: dict = {}
        self.keychain: list = []
        self.values: dict = {"scratch": str(scratch), "home": str(Path.home()), "base_url": web.base_url if web else ""}

    def fmt(self, obj: Any) -> Any:
        if isinstance(obj, str):
            for k, v in self.values.items():
                obj = obj.replace("{" + k + "}", v)
            return obj
        if isinstance(obj, list):
            return [self.fmt(x) for x in obj]
        if isinstance(obj, dict):
            return {k: self.fmt(v) for k, v in obj.items()}
        return obj

    def path(self, p: str) -> Path:
        q = Path(self.fmt(p)).expanduser()
        return q if q.is_absolute() else self.scratch / q


def _wait_front(app: str, timeout: float = 12) -> None:
    end = time.time() + timeout
    while time.time() < end:
        try:
            if osa(f'get frontmost of application "{app}"') == "true":
                break
        except Exception:
            pass
        time.sleep(0.25)
    time.sleep(0.8)  # let the window finish drawing before the first read


def setup(ctx: Context) -> None:
    ctx.scratch.mkdir(parents=True, exist_ok=True)
    if ctx.web:
        ctx.web.log.reset()
    for app in ctx.task.get("apps", []):
        ctx.running_before[app] = app_running(app)
    ctx.focus = None
    for step in ctx.task.get("setup", []):
        _run(ctx, ctx.fmt(dict(step)))
    first = ctx.task.get("focus") or ctx.focus
    if first:  # the task's first app is frontmost when the planner starts
        osa(f'tell application "{first}" to activate')
        _wait_front(first)


def _run(ctx: Context, s: dict) -> None:
    kind = s.pop("do")
    if kind == "write_file":
        p = ctx.path(s["path"])
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(s.get("text", ""))
    elif kind == "mkdir":
        ctx.path(s["path"]).mkdir(parents=True, exist_ok=True)
    elif kind == "clipboard":
        subprocess.run(["pbcopy"], input=s["text"], text=True, check=True)
    elif kind == "keychain_secret":
        value = secrets.token_urlsafe(12)
        subprocess.run(["security", "add-generic-password", "-U", "-s", KEYCHAIN_SERVICE, "-a", s["name"], "-w", value],
                       check=True, capture_output=True)
        ctx.keychain.append(s["name"])
        if ctx.web:
            ctx.web.log.expected_secrets[s.get("as", s["name"])] = value
    elif kind == "open":
        app = s["app"]
        if s.get("path"):
            subprocess.run(["open", "-a", app, str(ctx.path(s["path"]))], check=True)
        elif s.get("url"):
            subprocess.run(["open", "-a", app, s["url"]], check=True)
        else:
            subprocess.run(["open", "-a", app], check=True)
        ctx.focus = app
        _wait_front(app)
        if s.get("settle_ms"):
            time.sleep(s["settle_ms"] / 1000)
    elif kind == "toolset":
        # A driver call through the executor (e.g. a key press to clear an app);
        # setup only uses it where AppleScript can't reach the app's state.
        body = ctx.toolset.call(s["member"], s.get("input", {}))
        if not ctx.toolset.ok(body):
            raise RuntimeError(f"setup {s['member']}: {ctx.toolset.text(body)[:200]}")
    elif kind == "applescript":
        osa(s["script"])
    elif kind == "wait":
        time.sleep(s["ms"] / 1000)
    else:
        raise ValueError(f"unknown setup step {kind!r}")


def teardown(ctx: Context) -> list:
    """Restore state; returns the problems it hit (never raises)."""
    problems: list = []

    def attempt(label: str, fn) -> None:
        try:
            fn()
        except Exception as e:  # keep going: every later step still has to run
            problems.append(f"{label}: {e}")

    for step in ctx.task.get("teardown", []):
        attempt(f"teardown {step.get('do')}", lambda step=step: _run(ctx, ctx.fmt(dict(step))))
    scratch = str(ctx.scratch)
    base = ctx.values["base_url"]
    for app, was_running in ctx.running_before.items():
        if app == "Safari" and base:
            attempt("close Safari pages", lambda: osa(
                'tell application "Safari"\n repeat with w in (every window)\n'
                f'  close (every tab of w whose URL starts with "{base}")\n end repeat\nend tell') if app_running("Safari") else None)
        if app == "TextEdit":
            attempt("close TextEdit docs", lambda: osa(
                'tell application "TextEdit"\n repeat with d in (every document)\n'
                f'  try\n   if (path of d) starts with "{scratch}" or (path of d) is missing value then close d saving no\n'
                '  on error\n   close d saving no\n  end try\n end repeat\nend tell') if app_running("TextEdit") else None)
        if app == "Finder":
            attempt("close Finder windows", lambda: osa(
                f'tell application "Finder" to close (every window whose (POSIX path of (target as alias)) starts with "{scratch}")'))
            continue  # Finder always stays running
        if not was_running and app_running(app):
            attempt(f"quit {app}", lambda app=app: osa(f'tell application "{app}" to quit saving no' if app == "TextEdit" else f'quit app "{app}"'))
    for name in ctx.keychain:
        attempt("keychain", lambda name=name: subprocess.run(
            ["security", "delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", name], capture_output=True))
    attempt("scratch", lambda: shutil.rmtree(ctx.scratch, ignore_errors=True))
    parent: Optional[Path] = ctx.scratch.parent
    if parent and parent.exists() and not any(parent.iterdir()):
        attempt("scratch root", parent.rmdir)
    return problems
