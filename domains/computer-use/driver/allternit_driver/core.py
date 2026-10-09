"""The Allternit Driver: one API over two engines.

Ops: read_ui, act, run_batch, verify, screenshot, zoom, wait, pixel_*,
status and router. Each op is routed (router.py), each read lands in our own
element map (element_map.py), and every window has one reader and one input
owner at a time (locks.py).
"""

from __future__ import annotations

import base64
import logging
import os
import sys
import threading
import time
from dataclasses import dataclass
from typing import Any, Callable

from . import element_map as em
from . import live as lv
from .engines.arc import ArcEngine
from .engines.cua import CuaEngine, CuaError
from .locks import WindowLocks
from .router import ARC, CUA, Audit, Decision, Router

log = logging.getLogger("allternit_driver")

# Cua desktop-scope tools the pixel_* ops forward (screen coordinates).
PIXEL_TOOLS = {
    "click", "double_click", "right_click", "move_cursor", "drag", "scroll",
    "type_text", "press_key", "hotkey", "get_cursor_position", "get_screen_size",
}
DEFAULT_MAX_ELEMENTS = 200
COALESCE_S = 0.25  # A reader that waited on another reader reuses a map this fresh.
PREWARM_WHILE_USED_S = 600.0  # Prewarm the frontmost app while a read happened this recently.


class DriverError(Exception):
    def __init__(self, code: str, message: str, data: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.data = data or {}


@dataclass
class Target:
    pid: int
    window_id: int
    app: str  # Bundle id (macOS) or app name: the router's app key.

    @property
    def key(self) -> str:
        return f"{self.pid}:{self.window_id}"


def _ms(start: float) -> float:
    return round((time.perf_counter() - start) * 1000, 1)


class Driver:
    def __init__(self, cua: CuaEngine, arc: ArcEngine | None = None, state_dir: str | None = None, os_name: str = sys.platform) -> None:
        self.os = os_name
        self.cua = cua
        self.arc = arc if arc is not None else ArcEngine()
        self.audit = Audit(state_dir)
        self.router = Router(os_name, state_dir, self.audit)
        self.maps = em.ElementMaps()
        self.locks = WindowLocks()
        self._snaps: dict[str, Any] = {}  # window key -> arc snapshot behind the current map
        self._targets: dict[str, Target] = {}  # window key -> target
        self._labels: dict[int, str] = {}  # pid -> app key
        self._pids: dict[str, int] = {}  # app name / bundle id -> pid
        self._last_use = float("-inf")
        self.live = lv.LiveMaps(self._observer(), self._live_refresh, self._live_forget)

    def _observer(self) -> lv.Observer:
        """This OS's accessibility notifications for the live map. Any failure
        leaves the base Observer: every window then reads on a timer."""
        try:
            if self.os == "darwin":
                from .live.macos import AXHub

                if self.arc.hub is None:
                    self.arc.hub = AXHub()
                return self.arc.hub
            if self.os.startswith("linux"):
                from .live.atspi import ATSPIObserver

                return ATSPIObserver()
            if self.os == "win32":
                from .live.uia import UIAObserver

                return UIAObserver()
        except Exception as e:
            log.warning("live map observer unavailable: %s", e)
        return lv.Observer()

    def start(self) -> None:
        if self.os == "darwin":
            self.arc.start()
        if self._arc_ok():
            self._last_use = time.monotonic()  # Warm the frontmost app once at launch, too.
            threading.Thread(target=self._prewarm, name="arc-prewarm", daemon=True).start()
        # Cua's MCP session can take seconds to come up; the API answers
        # meanwhile (macOS reads are arc's), and a Cua call waits for it.
        threading.Thread(target=self.cua.start, name="cua-start", daemon=True).start()

    def _prewarm(self) -> None:
        """While computer use is active (a read in the last 10 minutes), get the
        frontmost app's window ready before anyone asks: its notification
        watch, window lookup and first walk. The first read_ui of an app the
        user just switched to is then a cached re-read. Nothing runs while no
        agent is using the driver."""
        import AppKit  # type: ignore

        ws = AppKit.NSWorkspace.sharedWorkspace()
        warmed: int | None = None
        while True:
            time.sleep(0.5)
            if time.monotonic() - self._last_use > PREWARM_WHILE_USED_S:
                continue
            try:
                front = ws.frontmostApplication()
                pid = int(front.processIdentifier()) if front is not None else None
                if pid is None or pid == warmed or pid == os.getpid():
                    continue
                warmed = pid
                t = Target(pid, self.arc.resolve(pid, None), self._label(pid))
                self._targets[t.key] = t
                self._read(t, asked=False)
            except Exception as e:  # An app without windows, or one quitting.
                log.debug("prewarm skipped: %s", e)

    def close(self) -> None:
        self.live.close()
        self.router.save()
        self.arc.close()
        self.cua.close()

    # ---- engines / routing -------------------------------------------------------

    def _arc_ok(self) -> bool:
        return self.os == "darwin" and self.arc.available

    def capable(self, op: str) -> set[str]:
        arc, cua = self._arc_ok(), self.cua.available
        table = {
            "read": {ARC} if arc else set(),
            "wait": {ARC} if arc else set(),
            "settle": {ARC} if arc else set(),
            "menu": {ARC} if arc else set(),
            # Cua 0.34 has no run_actions: our executor runs batches on the map's engine.
            "batch": {ARC} if arc else set(),
            "verify": {ARC} if arc else set(),
            "screenshot": {ARC} if arc else set(),
        }.get(op, set())
        if cua and (op != "batch" or not arc):
            table = table | {CUA}
        return table

    def _engine(self, name: str) -> Any:
        return self.arc if name == ARC else self.cua

    def _routed(self, op: str, app: str, run: Callable[[Decision], Any], capable: set[str] | None = None) -> Any:
        decision = self.router.choose(op, app, self.capable(op) if capable is None else capable)
        if decision.reason == "unavailable":
            raise DriverError("engine_unavailable", f"no engine can {op} here", {"engines": self.engines()})
        start = time.perf_counter()
        try:
            out = run(decision)
        except Exception as e:
            self.router.record(op, app, decision, _ms(start), False, str(e))
            raise
        self.router.record(op, app, decision, _ms(start), True)
        return out

    def engines(self) -> dict[str, Any]:
        return {
            "arc": {"available": self._arc_ok(), "error": self.arc.error},
            "cua": {"available": self.cua.available, "error": self.cua.error,
                    "run_actions": "run_actions" in self.cua.tools},
        }

    # ---- targets ---------------------------------------------------------------------

    def _label(self, pid: int, fallback: str = "") -> str:
        label = self._labels.get(pid)
        if label is None and self._arc_ok():
            for app in self.arc.apps():
                self._labels[app["pid"]] = app["bundle_id"] or app["name"]
            label = self._labels.get(pid)
        return label or fallback or str(pid)

    def target(self, p: dict[str, Any]) -> Target:
        if p.get("element"):
            key = self.maps.window_of(p["element"])
            if key and key in self._targets:
                return self._targets[key]
        pid = int(p["pid"]) if p.get("pid") else None
        wid = int(p["window_id"]) if p.get("window_id") else None
        app = str(p.get("app") or "").strip()
        if self._arc_ok():
            if pid is None:
                pid = self._mac_pid(app)
            wid = self.arc.resolve(pid, wid)
            t = Target(pid, wid, self._label(pid, app))
        else:
            wins = [w for w in self.cua.windows(pid) if w.get("is_on_screen", True)]
            if app:
                wins = [w for w in wins if app.lower() in str(w.get("app_name", "")).lower()]
            if wid:
                wins = [w for w in wins if int(w.get("window_id", 0)) == wid] or wins
            if not wins:
                raise DriverError("window_not_found", f"no on-screen window for {app or pid or 'the front app'}")
            w = wins[0]
            t = Target(int(w["pid"]), int(w["window_id"]), str(w.get("app_name") or app or w["pid"]))
        self._targets[t.key] = t
        return t

    def _mac_pid(self, app: str) -> int:
        import AppKit  # type: ignore

        ws = AppKit.NSWorkspace.sharedWorkspace()
        if not app:
            front = ws.frontmostApplication()
            if front is None:
                raise DriverError("window_not_found", "no frontmost app")
            return int(front.processIdentifier())
        q = app.lower()
        pid = self._pids.get(q)
        if pid is not None:
            running = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
            if running is not None and not running.isTerminated():
                return pid
        for a in ws.runningApplications():
            name, bundle = str(a.localizedName() or ""), str(a.bundleIdentifier() or "")
            if q in (name.lower(), bundle.lower()):
                self._pids[q] = int(a.processIdentifier())
                return self._pids[q]
        raise DriverError("app_not_running", f"{app} isn't running")

    # ---- read_ui ---------------------------------------------------------------------

    def read_ui(self, p: dict[str, Any]) -> dict[str, Any]:
        """Tree of a window as our element map. Params: app | pid [+ window_id],
        query, max_elements (default 200), since (a version: answer only the
        changes), fresh (skip the no-change shortcut), crops (attach crop hashes)."""
        start = time.perf_counter()
        self._last_use = time.monotonic()
        t = self.target(p)
        try:
            version, info = self._read(t, fresh=bool(p.get("fresh")), crops=bool(p.get("crops")))
        except Exception as e:
            # The app's window, resolved without a notification since, closed
            # quietly (in the background): resolve it again once.
            if p.get("window_id") or type(e).__name__ != "TargetUnavailable" or not self._arc_ok():
                raise
            self.arc.forget_window(t.pid)
            self.live.drop(t.key, "window gone")
            t = self.target(p)
            version, info = self._read(t, fresh=bool(p.get("fresh")), crops=bool(p.get("crops")))
        out = self._answer(t, version, info, p)
        out["ms"] = _ms(start)
        return out

    def _read(self, t: Target, fresh: bool = False, crops: bool = False, asked: bool = True) -> tuple[em.Version, dict[str, Any]]:
        """The window's map. A watched, clean window answers from the live map
        with no accessibility call; a dirty one is patched first; ``fresh``
        (a forced refresh) and first sight walk the whole window."""
        self.live.ensure(t.key, t.pid, t.window_id, asked)

        def run(d: Decision) -> tuple[em.Version, dict[str, Any]]:
            with self.locks.reader(t.key) as waited:
                cur = self.maps.window(t.key).current
                mode = "full"
                if not fresh and not crops and cur is not None and cur.engine == d.engine:
                    mode = self.live.serve(t.key)
                    if mode is None or (waited and time.monotonic() - cur.meta.get("_at", 0) < COALESCE_S):
                        return cur, {"engine": d.engine, "cached": True}
                return self._walk(t, d.engine, full=mode == "full", crops=crops), {"engine": d.engine, "cached": False}

        return self._routed("read", t.app, run)

    def _walk(self, t: Target, engine: str, full: bool, crops: bool = False) -> em.Version:
        """Re-read one window into its map (reader lock held): a patch or a full
        walk on arc; a read on Cua (narrow while the window is degraded)."""
        full = self.live.begin(t.key) or full
        if engine == ARC:
            nodes, meta, snap = self.arc.read(t.pid, t.window_id, full=full)
            self._snaps[t.key] = snap
        else:
            w = self.live.get(t.key)
            narrow = lv.DEGRADED_MAX_ELEMENTS if w is not None and w.degraded else None
            nodes, meta = self.cua.read(t.pid, t.window_id, max_elements=narrow)
        origin = nodes[0].bounds[:2] if nodes and nodes[0].bounds else (0.0, 0.0)
        elements = em.build(nodes, origin)
        if crops and engine == ARC:
            self.arc.crop_hashes(t.pid, t.window_id, elements)
        meta["_at"] = time.monotonic()
        version, _ = self.maps.record(t.key, elements, engine, meta)
        self.live.done(t.key, full)
        self.router.mark_degraded(t.app, meta.get("degraded"))
        return version

    def _live_refresh(self, key: str, full: bool) -> None:
        """The live map's worker: patch a window after its notifications."""
        t = self._targets.get(key)
        cur = self.maps.window(key).current
        if t is None or cur is None:
            return
        with self.locks.reader(key):
            try:
                self._walk(t, cur.engine, full)
            except Exception as e:
                if type(e).__name__ != "TargetUnavailable":
                    raise
                if self.arc.running(t.pid):
                    raise lv.WindowGone(str(e)) from e
                raise lv.AppGone(str(e)) from e

    def _live_forget(self, key: str) -> None:
        """A window left the live map (idle, evicted, or its app quit)."""
        t = self._targets.pop(key, None)
        self._snaps.pop(key, None)
        self.maps.forget(key)
        if t is not None and self._arc_ok() and not any(o.pid == t.pid for o in self._targets.values()):
            self.arc.release(t.pid)
            self._labels.pop(t.pid, None)

    def _answer(self, t: Target, version: em.Version, info: dict[str, Any], p: dict[str, Any]) -> dict[str, Any]:
        meta = version.meta
        out: dict[str, Any] = {
            "window": {"pid": t.pid, "window_id": t.window_id, "app": meta.get("app") or t.app, "title": meta.get("title")},
            "version": version.number,
            "engine": info["engine"],
            "cached": info["cached"],
            "live": self.live.state(t.key),
        }
        degraded = self.router.degraded.get(t.app)
        if degraded:
            out["degraded"] = True
            out["degraded_reason"] = degraded
        since = p.get("since")
        if since is not None:
            diff = self.maps.window(t.key).diff(int(since))
            if diff is not None:
                out["diff"] = diff
                return out
            out["reset"] = True  # That version is gone: here is the whole map.
        limit = p.get("max_elements", DEFAULT_MAX_ELEMENTS)
        kept, total = em.select(version.elements.values(), p.get("query"), int(limit) if limit else None)
        out["elements"] = [e.public() for e in kept]
        out["total"] = total
        if len(kept) < total and not p.get("query"):
            out["truncated"] = True
        return out

    # ---- act -------------------------------------------------------------------------

    def act(self, p: dict[str, Any]) -> dict[str, Any]:
        """Act on an element id: click, double_click, right_click, set_value,
        type, select, press (key), focus; or ``menu`` with ``path`` and a window.
        ``version`` pins the map the action was planned on: an older one is
        refused with the fresh map."""
        start = time.perf_counter()
        op = str(p.get("op") or "click")
        if op == "menu":
            out = self._menu(self.target(p), p.get("path"))
            out["ms"] = _ms(start)
            return out
        eid = str(p.get("id") or "")
        key = self.maps.window_of(eid)
        if not key or key not in self._targets:
            raise DriverError("unknown_element", f"{eid} isn't in any element map; call read_ui first")
        t = self._targets[key]
        wm = self.maps.window(key)
        self._read(t)  # Up to date with every notification so far (no AX call when clean).
        try:
            cur = wm.check(int(p["version"]) if p.get("version") is not None else None)
        except em.StaleVersion:
            fresh = self.read_ui({"pid": t.pid, "window_id": t.window_id})
            return {"status": "stale_version", "map": fresh, "ms": _ms(start)}
        out = self._act_on(t, cur, eid, op, p.get("value"), p.get("key"))
        out["ms"] = _ms(start)
        return out

    def _act_on(self, t: Target, cur: em.Version, eid: str, op: str, value: Any, key: str | None) -> dict[str, Any]:
        element = cur.elements.get(eid)
        if element is None:
            raise DriverError("element_gone", f"{eid} is not in version {cur.number}")
        before = cur.number
        with self.locks.input(t.key):
            start = time.perf_counter()
            d = Decision(cur.engine, "map")
            try:
                if op == "press" and cur.engine == ARC and self.cua.available:
                    # arc's Skylight key posting can take the whole sidecar
                    # down (SIGSEGV in the private SPI path on macOS 14).
                    # Key input runs on the Cua engine instead — the same
                    # engine the pixel key ops route to. The key goes to the
                    # window's focus (arc map natives are arc element ids,
                    # not Cua tokens, so no element token is passed).
                    self.cua.act(t.pid, t.window_id, {}, op, value, key)
                    status, settled = "done", None
                    cur, _ = self._read(t, fresh=True)
                    d = Decision(CUA, "map")
                elif cur.engine == ARC:
                    res = self.arc.act(self._snaps[t.key], element.native, op, value, key)
                    status = res.status
                    if res.snapshot is not None:
                        self._snaps[t.key] = res.snapshot
                        nodes = self.arc.nodes(res.snapshot)
                        meta = {**self.arc.meta(res.snapshot), "_at": time.monotonic()}
                        origin = nodes[0].bounds[:2] if nodes and nodes[0].bounds else (0.0, 0.0)
                        cur, _ = self.maps.record(t.key, em.build(nodes, origin), ARC, meta)
                        self.live.done(t.key, False)
                    settled = getattr(res, "settled", None)
                else:
                    self.cua.act(t.pid, t.window_id, element.native, op, value, key)
                    status, settled = "done", None
                    cur, _ = self._read(t, fresh=True)
            except Exception as e:
                self.router.record("act", t.app, d, _ms(start), False, str(e))
                raise
            self.router.record("act", t.app, d, _ms(start), status == "done")
        out: dict[str, Any] = {"status": status, "version": cur.number, "engine": d.engine}
        if settled is not None:
            out["settled"] = {"reacted": settled.reacted, "timed_out": settled.timed_out, "ms": settled.elapsed_ms}
        if status == "done":
            diff = self.maps.window(t.key).diff(before)
            if diff is not None:
                out["changes"] = diff
        else:  # changed / stale: the app moved under the plan; hand back the fresh map.
            out["map"] = self._answer(t, cur, {"engine": cur.engine, "cached": False}, {})
        return out

    def _menu(self, t: Target, path: Any) -> dict[str, Any]:
        parts = [s.strip() for s in (path.split(">") if isinstance(path, str) else list(path or [])) if str(s).strip()]
        if not parts:
            raise DriverError("bad_input", "menu needs a path such as 'File > Save'")
        prev = self.maps.window(t.key).current

        def run(d: Decision) -> None:
            with self.locks.input(t.key):
                if d.engine == ARC:
                    self.arc.menu(t.pid, t.window_id, parts)
                else:
                    self.cua.menu(t.pid, t.window_id, parts)

        self._routed("menu", t.app, run)
        cur, info = self._read(t, fresh=True)
        out: dict[str, Any] = {"status": "done", "version": cur.number, "engine": info["engine"]}
        if prev is not None and (diff := self.maps.window(t.key).diff(prev.number)) is not None:
            out["changes"] = diff
        return out

    # ---- conditions (wait_for / expect / verify) ------------------------------------------

    @staticmethod
    def check(version: em.Version, cond: dict[str, Any]) -> tuple[bool, str]:
        """Evaluate one bounded check on a map: id | role/name selector, then
        gone / text / value / enabled. All given parts must hold."""
        matches = list(version.elements.values())
        if cond.get("id"):
            e = version.elements.get(cond["id"])
            matches = [e] if e is not None else []
        if cond.get("role"):
            role = str(cond["role"]).removeprefix("AX").lower()
            matches = [e for e in matches if e.role.lower() == role]
        if cond.get("name"):
            name = str(cond["name"]).lower()
            matches = [e for e in matches if name in e.name.lower()]
        selector = any(cond.get(k) for k in ("id", "role", "name"))
        if cond.get("gone"):
            return (not matches, "gone" if not matches else f"{len(matches)} still present")
        if selector and not matches:
            return False, "no element matches"
        if "text" in cond:
            text = str(cond["text"]).lower()
            hit = any(text in e.name.lower() or text in em._text(e.value).lower() for e in matches)
            if not hit:
                return False, f"no match shows {cond['text']!r}"
        if "value" in cond:
            want = str(cond["value"])
            if not any(em._text(e.value) == want for e in matches):
                got = em._text(matches[0].value) if matches else ""
                return False, f"value is {got!r}, not {want!r}"
        if "enabled" in cond and not any(e.enabled == bool(cond["enabled"]) for e in matches):
            return False, f"enabled is not {bool(cond['enabled'])}"
        return True, "ok"

    def _await(self, t: Target, cond: dict[str, Any], timeout_ms: int) -> tuple[bool, str, em.Version]:
        deadline = time.monotonic() + max(0, timeout_ms) / 1000
        version, _ = self._read(t)
        while True:
            ok, why = self.check(version, cond)
            if ok or time.monotonic() >= deadline:
                return ok, why, version
            remaining = deadline - time.monotonic()
            if version.engine == ARC and t.key in self._snaps:
                with self.locks.reader(t.key):
                    snap = self.arc.wait(self._snaps[t.key], timeout_s=min(remaining, 1.0))
                self._snaps[t.key] = snap
                nodes = self.arc.nodes(snap)
                origin = nodes[0].bounds[:2] if nodes and nodes[0].bounds else (0.0, 0.0)
                version, _ = self.maps.record(t.key, em.build(nodes, origin), ARC, {**self.arc.meta(snap), "_at": time.monotonic()})
            else:
                time.sleep(min(remaining, 0.15))
                version, _ = self._read(t, fresh=True)

    # ---- run_batch -----------------------------------------------------------------

    def run_batch(self, p: dict[str, Any]) -> dict[str, Any]:
        """Ordered steps, each one of {act: {id, op, value?, key?}},
        {menu: path}, {pixel: {tool, args}}, {wait: ms}, with optional
        ``wait_for`` (before) and ``expect`` (after) checks and ``timeout_ms``.
        Stops at the first failed step or check; ends with one observe and a
        cross-check by the other engine."""
        start = time.perf_counter()
        steps = list(p.get("steps") or [])
        if not steps:
            raise DriverError("bad_input", "run_batch needs steps")
        first_id = next((s["act"]["id"] for s in steps if isinstance(s.get("act"), dict) and s["act"].get("id")), None)
        t = self.target({**p, "element": first_id})
        wm = self.maps.window(t.key)
        self._read(t)  # Up to date with every notification so far (no AX call when clean).
        if p.get("version") is not None:
            try:
                wm.check(int(p["version"]))
            except em.StaleVersion:
                return {"ok": False, "status": "stale_version", "map": self.read_ui({"pid": t.pid, "window_id": t.window_id}), "ms": _ms(start)}
        initial = wm.current.number
        batch_engine = self.router.choose("batch", t.app, self.capable("batch"))
        results: list[dict[str, Any]] = []
        failed_at: int | None = None
        engines_used: set[str] = set()
        for i, step in enumerate(steps):
            s0 = time.perf_counter()
            timeout = int(step.get("timeout_ms", 3000))
            res: dict[str, Any] = {"i": i}
            try:
                if step.get("wait_for"):
                    ok, why, _ = self._await(t, step["wait_for"], timeout)
                    if not ok:
                        raise DriverError("wait_for_failed", why)
                if isinstance(step.get("act"), dict):
                    a = step["act"]
                    cur = wm.current
                    out = self._act_on(t, cur, str(a.get("id")), str(a.get("op") or "click"), a.get("value"), a.get("key"))
                    engines_used.add(out["engine"])
                    if out["status"] != "done":
                        raise DriverError(out["status"], f"the window changed before step {i}")
                elif step.get("menu"):
                    engines_used.add(self._menu(t, step["menu"])["engine"])
                elif isinstance(step.get("pixel"), dict):
                    self.pixel(str(step["pixel"].get("tool")), dict(step["pixel"].get("args") or {}))
                    engines_used.add(CUA)
                elif step.get("wait") is not None:
                    time.sleep(min(float(step["wait"]), 10_000) / 1000)
                elif not step.get("wait_for"):
                    raise DriverError("bad_input", f"step {i} has nothing to do")
                if step.get("expect"):
                    ok, why, _ = self._await(t, step["expect"], int(step.get("timeout_ms", 1500)))
                    if not ok:
                        raise DriverError("expect_failed", why)
                res["status"] = "ok"
            except DriverError as e:
                res.update(status="failed", code=e.code, error=str(e))
                failed_at = i
            except Exception as e:
                res.update(status="failed", code="error", error=str(e)[:300])
                failed_at = i
            res["ms"] = _ms(s0)
            results.append(res)
            if failed_at is not None:
                break
        self.router.record("batch", t.app, batch_engine, _ms(start), failed_at is None)
        final, info = self._read(t, fresh=True)  # The one observe.
        out: dict[str, Any] = {
            "ok": failed_at is None, "steps": results, "version": final.number, "engine": info["engine"],
            "cross_check": self._cross_check(t, steps, engines_used, final),
        }
        if failed_at is not None:
            out["failed_at"] = failed_at
        diff = wm.diff(initial)
        out["changes"] = diff if diff is not None else self._answer(t, final, info, {})
        out["ms"] = _ms(start)
        return out

    def _cross_check(self, t: Target, steps: list[dict[str, Any]], used: set[str], final: em.Version) -> dict[str, Any]:
        """One cheap confirm by the engine that didn't run the batch. macOS: arc's
        freshness check (the final expect on a fresh, uncached walk). Elsewhere:
        Cua's verify_state on the final expect."""
        last = next((s["expect"] for s in reversed(steps) if s.get("expect")), None)
        if last is None:
            return {"skipped": "no expect to confirm"}
        start = time.perf_counter()
        try:
            if self._arc_ok():
                ok, why = self.check(final, last) if final.engine == ARC else self.check(self._read(t, fresh=True)[0], last)
                return {"engine": ARC, "ok": ok, "detail": why, "ms": _ms(start)}
            expect = _cua_predicate(last)
            if expect is None:
                return {"skipped": "check not expressible as verify_state"}
            r = self.cua.verify(t.pid, t.window_id, [expect], timeout_ms=0)
            return {"engine": CUA, "ok": _cua_verified(r), "detail": r.get("_text", "")[:200], "ms": _ms(start)}
        except Exception as e:
            return {"engine": CUA if not self._arc_ok() else ARC, "ok": None, "detail": str(e)[:200], "ms": _ms(start)}

    # ---- verify / wait ------------------------------------------------------------------

    def verify(self, p: dict[str, Any]) -> dict[str, Any]:
        """Bounded checks (role, name, text, gone, value, enabled) on a fresh read."""
        start = time.perf_counter()
        checks = list(p.get("checks") or [])
        if not checks:
            raise DriverError("bad_input", "verify needs checks")
        t = self.target({**p, "element": next((c.get("id") for c in checks if c.get("id")), None)})

        def run(d: Decision) -> list[dict[str, Any]]:
            preds = [_cua_predicate(c) for c in checks]
            if d.engine == CUA and all(preds):
                r = self.cua.verify(t.pid, t.window_id, preds, timeout_ms=int(p.get("timeout_ms", 0)))
                ok = _cua_verified(r)
                return [{"check": c, "ok": ok, "detail": "verify_state"} for c in checks]
            version = self._read(t, fresh=True)[0]
            out = []
            for c in checks:
                ok, why = self.check(version, c)
                out.append({"check": c, "ok": ok, "detail": why})
            return out

        results = self._routed("verify", t.app, run)
        return {"ok": all(r["ok"] for r in results), "results": results, "ms": _ms(start)}

    def wait(self, p: dict[str, Any]) -> dict[str, Any]:
        """Wait for a condition (``for``) or for the window to change/settle, up to timeout_ms."""
        start = time.perf_counter()
        t = self.target(p)
        timeout = int(p.get("timeout_ms", 2000))
        if p.get("for"):
            ok, why, version = self._await(t, p["for"], timeout)
            return {"ok": ok, "detail": why, "version": version.number, "ms": _ms(start)}
        before = self._read(t)[0].number

        def run(d: Decision) -> em.Version:
            if d.engine == ARC and t.key in self._snaps:
                snap = self.arc.wait(self._snaps[t.key], timeout_s=timeout / 1000)
                self._snaps[t.key] = snap
                nodes = self.arc.nodes(snap)
                origin = nodes[0].bounds[:2] if nodes and nodes[0].bounds else (0.0, 0.0)
                return self.maps.record(t.key, em.build(nodes, origin), ARC, {**self.arc.meta(snap), "_at": time.monotonic()})[0]
            deadline = time.monotonic() + timeout / 1000
            while True:  # The live map patches on each notification; poll it, not the app.
                v = self._read(t)[0]
                if v.number != before or time.monotonic() >= deadline:
                    return v
                time.sleep(0.02)

        version = self._routed("wait", t.app, run)
        out: dict[str, Any] = {"ok": True, "changed": version.number != before, "version": version.number, "ms": _ms(start)}
        if version.number != before and (diff := self.maps.window(t.key).diff(before)) is not None:
            out["changes"] = diff
        return out

    # ---- pixels ------------------------------------------------------------------------

    def screenshot(self, p: dict[str, Any]) -> dict[str, Any]:
        """PNG of a window (app / pid / window_id) or, with none given, the desktop."""
        start = time.perf_counter()
        window = any(p.get(k) for k in ("app", "pid", "window_id"))
        t = self.target(p) if window else None
        capable = self.capable("screenshot") if t else ({CUA} if self.cua.available else set())

        def run(d: Decision) -> dict[str, Any]:
            if d.engine == ARC and t is not None:
                png, scale = self.arc.screenshot(t.pid, t.window_id)
                return {"png": base64.b64encode(png).decode(), "scale": scale}
            png = self.cua.screenshot(t.pid if t else None, t.window_id if t else None)
            return {"png": base64.b64encode(png).decode()}

        out = self._routed("screenshot", t.app if t else "desktop", run, capable)
        out["ms"] = _ms(start)
        return out

    def zoom(self, p: dict[str, Any]) -> dict[str, Any]:
        """Cropped image of a window region [x1, y1, x2, y2] in screenshot pixels."""
        start = time.perf_counter()
        t = self.target(p)
        region = p.get("region")
        if not isinstance(region, list) or len(region) != 4:
            raise DriverError("bad_input", "zoom needs region [x1, y1, x2, y2]")
        png = self._routed("zoom", t.app, lambda d: self.cua.zoom(t.pid, t.window_id, [float(v) for v in region]))
        return {"png": base64.b64encode(png).decode(), "ms": _ms(start)}

    def pixel(self, tool: str, args: dict[str, Any]) -> dict[str, Any]:
        """Screen-wide input through Cua's desktop scope (the existing pixel path)."""
        if tool not in PIXEL_TOOLS:
            raise DriverError("bad_input", f"pixel_{tool} isn't a pixel op")
        args = {"scope": "desktop", **args} if tool not in ("get_cursor_position", "get_screen_size") else args
        reads = tool in ("get_cursor_position", "get_screen_size")

        def run(d: Decision) -> dict[str, Any]:
            if reads:
                return self.cua.pixel(tool, args)
            with self.locks.desktop_input():
                return self.cua.pixel(tool, args)

        out = self._routed("pixel", "desktop", run, {CUA} if self.cua.available else set())
        out.pop("_images", None)
        text = out.pop("_text", "")
        if text:
            out["text"] = text
        return out

    # ---- introspection ----------------------------------------------------------------

    def status(self, _p: dict[str, Any] | None = None) -> dict[str, Any]:
        return {"os": self.os, "engines": self.engines(), "live": self.live.status()}

    def router_table(self, _p: dict[str, Any] | None = None) -> dict[str, Any]:
        return self.router.table()


def _cua_predicate(cond: dict[str, Any]) -> dict[str, Any] | None:
    """Our check as a Cua verify_state predicate, when it can be one (Cua
    can't prove absence, and has no id or substring-of-value checks)."""
    if cond.get("gone") or cond.get("id") or "text" in cond:
        return None
    selector: dict[str, Any] = {}
    if cond.get("role"):
        role = str(cond["role"])
        selector["role"] = role if role.startswith("AX") or sys.platform != "darwin" else f"AX{role}"
    if cond.get("name"):
        selector["label_contains"] = str(cond["name"])
    if not selector:
        return None
    element: dict[str, Any] = {"selector": selector, "exists": True}
    if "value" in cond:
        element["value_equals"] = str(cond["value"])
    if "enabled" in cond:
        element["enabled"] = bool(cond["enabled"])
    return {"element": element}


def _cua_verified(r: dict[str, Any]) -> bool:
    """Cua 0.34's verify_state answers ``status: "satisfied"`` (with one status
    per predicate); other builds expose a boolean ``satisfied`` or ``ok``."""
    status = r.get("status")
    if isinstance(status, str):
        return status == "satisfied"
    satisfied = r.get("satisfied", r.get("ok"))
    return bool(satisfied) if satisfied is not None else False


def handles_cua_error(e: Exception) -> DriverError:
    if isinstance(e, DriverError):
        return e
    if isinstance(e, CuaError):
        return DriverError(e.code, str(e))
    return DriverError("error", str(e)[:500])
