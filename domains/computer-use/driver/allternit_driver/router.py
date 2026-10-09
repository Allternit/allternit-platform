"""Per-op router: which engine serves an op, for an app, on this OS.

It starts from the defaults below and learns from real latency and failures,
per user (persisted as JSON), and every decision lands in the audit log.

* An engine only gets an op it can do (``capable``); otherwise the other one
  serves it and the decision says ``capability``.
* An engine that keeps failing an op for an app (>= 3 calls, >= half failed)
  hands it to the other engine (``failures``).
* When both have >= 5 timed calls and the other is at most half as slow, it
  takes over (``latency``).
* Read-only, stateless ops (verify, screenshot, zoom) try the other engine on
  one call in ``EXPLORE_EVERY`` until it has enough samples, so latency can be
  compared at all. Input never explores.
* An app whose tree comes back empty or degraded is marked ``degraded``: reads
  still answer, flagged, and the vision fallback (a later phase) takes over.
"""

from __future__ import annotations

import json
import os
import threading
import time
from dataclasses import dataclass, field
from typing import Any

ARC, CUA = "arc", "cua"
ENGINES = (ARC, CUA)

# Op classes the router decides. ``act`` is not routed: an element action goes
# to the engine that produced the element map it was planned on.
OPS = ("read", "wait", "settle", "menu", "batch", "verify", "screenshot", "zoom", "parse_visual_regions", "record", "pixel")

DEFAULTS: dict[str, dict[str, str]] = {
    "darwin": {
        "read": ARC, "wait": ARC, "settle": ARC, "menu": ARC,
        "batch": CUA, "verify": CUA, "screenshot": CUA, "zoom": CUA,
        "parse_visual_regions": CUA, "record": CUA, "pixel": CUA,
    },
    # Linux and Windows: Cua only.
    "linux": {op: CUA for op in OPS},
    "win32": {op: CUA for op in OPS},
}

EXPLORABLE = {"verify", "screenshot", "zoom"}
EXPLORE_EVERY = 20
MIN_FAIL_CALLS = 3
MIN_LATENCY_SAMPLES = 5
EWMA = 0.3


@dataclass
class Stats:
    calls: int = 0
    failures: int = 0
    ewma_ms: float | None = None
    recent: list[bool] = field(default_factory=list)  # Last 10 outcomes.

    def add(self, ms: float, ok: bool) -> None:
        self.calls += 1
        if not ok:
            self.failures += 1
        self.recent = (self.recent + [ok])[-10:]
        if ok:
            self.ewma_ms = ms if self.ewma_ms is None else (1 - EWMA) * self.ewma_ms + EWMA * ms

    def failing(self) -> bool:
        return len(self.recent) >= MIN_FAIL_CALLS and self.recent.count(False) * 2 >= len(self.recent)

    def to_json(self) -> dict[str, Any]:
        return {"calls": self.calls, "failures": self.failures, "ewma_ms": self.ewma_ms, "recent": self.recent}

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> "Stats":
        return cls(int(data.get("calls", 0)), int(data.get("failures", 0)), data.get("ewma_ms"), list(data.get("recent", []))[-10:])


@dataclass(frozen=True)
class Decision:
    engine: str
    reason: str  # default | capability | failures | latency | explore | only


def _other(engine: str) -> str:
    return CUA if engine == ARC else ARC


class Router:
    def __init__(self, os_name: str, state_dir: str | None = None, audit: Any = None) -> None:
        self.os = os_name if os_name in DEFAULTS else "linux"
        self._path = os.path.join(state_dir, "router.json") if state_dir else None
        self._audit = audit
        self._lock = threading.Lock()
        self._stats: dict[tuple[str, str, str], Stats] = {}  # (op, app, engine)
        self._counts: dict[tuple[str, str], int] = {}
        self.degraded: dict[str, str] = {}  # app -> reason
        self._dirty = 0
        self._saved_at = 0.0
        self._load()

    # ---- deciding ----------------------------------------------------------

    def choose(self, op: str, app: str, capable: set[str]) -> Decision:
        default = DEFAULTS[self.os].get(op, CUA)
        if not capable:
            return Decision(default, "unavailable")
        if len(capable) == 1:
            only = next(iter(capable))
            return Decision(only, "default" if only == default else ("only" if self.os != "darwin" else "capability"))
        with self._lock:
            n = self._counts.get((op, app), 0) + 1
            self._counts[(op, app)] = n
            return self._pick(op, app, n)

    def _pick(self, op: str, app: str, n: int | None) -> Decision:
        """The pick for an op with both engines capable; lock held. ``n`` is the
        call count for exploration, None to never explore."""
        cur = DEFAULTS[self.os].get(op, CUA)
        alt = _other(cur)
        cs, as_ = self._stats.get((op, app, cur)), self._stats.get((op, app, alt))
        reason = "default"
        if cs is not None and cs.failing() and not (as_ is not None and as_.failing()):
            cur, alt, cs, as_, reason = alt, cur, as_, cs, "failures"
        elif (
            cs is not None and as_ is not None
            and cs.ewma_ms is not None and as_.ewma_ms is not None
            and cs.calls >= MIN_LATENCY_SAMPLES and as_.calls >= MIN_LATENCY_SAMPLES
            and as_.ewma_ms * 2 <= cs.ewma_ms
        ):
            cur, alt, cs, as_, reason = alt, cur, as_, cs, "latency"
        if (
            n is not None and op in EXPLORABLE and n % EXPLORE_EVERY == 0
            and (as_ is None or as_.calls < MIN_LATENCY_SAMPLES)
            and not (as_ is not None and as_.failing())
        ):
            return Decision(alt, "explore")
        return Decision(cur, reason)

    # ---- learning ----------------------------------------------------------

    def record(self, op: str, app: str, decision: Decision, ms: float, ok: bool, error: str | None = None) -> None:
        with self._lock:
            stats = self._stats.setdefault((op, app, decision.engine), Stats())
            stats.add(ms, ok)
            self._dirty += 1
        if self._audit is not None:
            self._audit.write(
                {"op": op, "app": app, "engine": decision.engine, "reason": decision.reason,
                 "ms": round(ms, 1), "ok": ok, **({"error": error[:200]} if error else {})}
            )
        self._maybe_save()

    def mark_degraded(self, app: str, reason: str | None) -> None:
        with self._lock:
            if reason:
                if self.degraded.get(app) != reason:
                    self.degraded[app] = reason
                    self._dirty += 1
            elif app in self.degraded:
                del self.degraded[app]
                self._dirty += 1
        if reason and self._audit is not None:
            self._audit.write({"op": "read", "app": app, "degraded": reason})

    # ---- reporting / persistence --------------------------------------------

    def table(self) -> dict[str, Any]:
        """The learned table: per app and op, each engine's stats and today's pick."""
        with self._lock:
            out: dict[str, Any] = {}
            for (op, app, engine), stats in self._stats.items():
                out.setdefault(app, {}).setdefault(op, {})[engine] = stats.to_json()
            degraded = dict(self.degraded)
            for app, ops in out.items():
                for op, engines in ops.items():
                    if op not in OPS:  # act: bound to the engine that produced the map.
                        engines["pick"] = {"engine": "map", "reason": "element map's engine"}
                        continue
                    d = self._pick(op, app, None) if self.os == "darwin" else Decision(CUA, "only")
                    engines["pick"] = {"engine": d.engine, "reason": d.reason}
        return {"os": self.os, "defaults": DEFAULTS[self.os], "apps": out, "degraded": degraded}

    def _load(self) -> None:
        if not self._path or not os.path.exists(self._path):
            return
        try:
            with open(self._path, encoding="utf-8") as f:
                data = json.load(f)
            if data.get("os") != self.os:
                return
            for row in data.get("stats", []):
                self._stats[(row["op"], row["app"], row["engine"])] = Stats.from_json(row)
            self.degraded = dict(data.get("degraded", {}))
        except Exception:  # A corrupt table only costs the learning, never startup.
            self._stats.clear()

    def _maybe_save(self, force: bool = False) -> None:
        if not self._path:
            return
        now = time.monotonic()
        with self._lock:
            if not self._dirty or (not force and self._dirty < 20 and now - self._saved_at < 5):
                return
            rows = [{"op": op, "app": app, "engine": eng, **s.to_json()} for (op, app, eng), s in self._stats.items()]
            data = {"os": self.os, "stats": rows, "degraded": self.degraded}
            self._dirty = 0
            self._saved_at = now
        tmp = f"{self._path}.tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(data, f)
        os.replace(tmp, self._path)

    def save(self) -> None:
        self._maybe_save(force=True)


class Audit:
    """Append-only JSONL log of routed ops (rotates at 5 MB, keeps one old file)."""

    LIMIT = 5 * 1024 * 1024

    def __init__(self, state_dir: str | None) -> None:
        self._path = os.path.join(state_dir, "audit.jsonl") if state_dir else None
        self._lock = threading.Lock()

    def write(self, row: dict[str, Any]) -> None:
        if not self._path:
            return
        line = json.dumps({"ts": round(time.time(), 3), **row}, separators=(",", ":")) + "\n"
        with self._lock:
            try:
                if os.path.exists(self._path) and os.path.getsize(self._path) > self.LIMIT:
                    os.replace(self._path, self._path + ".1")
                with open(self._path, "a", encoding="utf-8") as f:
                    f.write(line)
            except OSError:
                pass
