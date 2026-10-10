"""Vision fallback for read_ui / act (Driver phase D6).

When a window's accessibility tree is empty or can't see into part of the
window (canvas apps, games, remote desktops, custom-drawn UIs), the router
(``Router.choose_source``) switches that read to vision:

1. One window screenshot (arc on macOS, Cua elsewhere).
2. Set-of-marks (marks.py): numbered text/control/icon regions.
3. Each mark becomes an element in the same element-map shape as the tree's,
   with ``source: "vision"``, its ``mark`` number and a ``v`` id, so ``act``
   and ``run_batch`` take it like any other id.
4. ``read_ui(target=...)`` grounds a description with the local grounder
   (grounder.py), zooms and regrounds when unsure, and snaps the point to the
   smallest element (tree or mark) that contains it; with no such element it
   adds a ``point`` element there.

Acting on a vision id clicks/types at its centre through Cua's desktop pixel
path, after a pixel freshness check: the mark's mean hash must still match
the live window, otherwise the caller gets ``stale_version`` and a fresh map.
"""

from __future__ import annotations

import base64
import struct
import threading
import time
from typing import Any, Callable

from .. import element_map as em
from . import geometry as g
from .client import VisionUnavailable, VisionWorker

VISION_TTL_S = 1.0  # A vision map this fresh answers a re-read without a new screenshot.
HASH_TOLERANCE = 40  # Of 256 bits: anti-aliasing and the caret pass, a changed control doesn't.
POINT_SIZE = 8.0  # Screen points: the box of a grounded point that hit no element.


def png_size(png: bytes) -> tuple[int, int]:
    if png[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    w, h = struct.unpack(">II", png[16:24])
    return w, h


class Shot:
    """One window screenshot and where it sits on screen."""

    def __init__(self, png: bytes, origin: g.Point, scale: float) -> None:
        self.png = png
        self.size = png_size(png)
        self.origin = origin
        self.scale = scale or 1.0
        self.at = time.monotonic()
        self._b64: str | None = None

    @property
    def b64(self) -> str:
        if self._b64 is None:
            self._b64 = base64.b64encode(self.png).decode()
        return self._b64

    def to_px(self, bounds: tuple[float, float, float, float]) -> g.Box:
        """Screen-point bounds (x, y, w, h) -> image px box."""
        x, y, w, h = bounds
        s = self.scale
        return ((x - self.origin[0]) * s, (y - self.origin[1]) * s, (x + w - self.origin[0]) * s, (y + h - self.origin[1]) * s)


def marks_to_nodes(
    marks: list[dict[str, Any]],
    shot: Shot,
    regions: tuple[tuple[float, float, float, float], ...] = (),
    ax_boxes: list[g.Box] = (),  # type: ignore[assignment]
) -> list[em.RawNode]:
    """Worker marks (image px) -> RawNodes in screen points, flagged vision.

    ``regions`` (screen points) limits the marks to what the tree can't see
    (hybrid reads); marks that duplicate a tree element (IoU >= 0.5) drop."""
    px_regions = [shot.to_px(r) for r in regions]
    nodes: list[em.RawNode] = []
    for m in marks:
        box: g.Box = tuple(m["box"])  # type: ignore[assignment]
        c = g.center(box)
        if px_regions and not any(g.contains(r, c) for r in px_regions):
            continue
        if any(g.iou(box, a) >= 0.5 for a in ax_boxes):
            continue
        n = len(nodes) + 1
        nodes.append(em.RawNode(
            key=f"m{n}",
            role=m.get("kind") or "icon",
            name=m.get("name") or "",
            bounds=g.px_to_screen(box, shot.origin, shot.scale),
            actions=("click",),
            native={"box": box, "hash": m.get("hash"), "mark": n},
            source="vision",
        ))
    return nodes


def build(nodes: list[em.RawNode], origin: g.Point) -> list[em.Element]:
    elements = em.build(nodes, origin)
    for node, e in zip(nodes, elements):
        e.mark = (node.native or {}).get("mark")
    return elements


class Vision:
    """Vision maps per window, backed by the worker. ``shoot(target)``
    returns a fresh ``Shot`` of the target's window."""

    def __init__(self, state_dir: str | None, shoot: Callable[[Any], Shot]) -> None:
        self.worker = VisionWorker(state_dir)
        self.maps = em.ElementMaps()
        self.shoot = shoot
        self._shots: dict[str, Shot] = {}
        self._lock = threading.Lock()

    def status(self) -> dict[str, Any]:
        return self.worker.status()

    def close(self) -> None:
        self.worker.close()

    def shot(self, t: Any, fresh: bool = False) -> Shot:
        cur = self._shots.get(t.key)
        if not fresh and cur is not None and time.monotonic() - cur.at < VISION_TTL_S:
            return cur
        s = self.shoot(t)
        self._shots[t.key] = s
        return s

    def read(self, t: Any, regions: tuple = (), ax: list[em.Element] = (), fresh: bool = False, max_marks: int = 120) -> tuple[em.Version, Shot]:  # type: ignore[assignment]
        """Marks for one window -> a vision map version."""
        wm = self.maps.window(t.key)
        cur = wm.current
        if not fresh and cur is not None and time.monotonic() - cur.meta.get("_at", 0) < VISION_TTL_S and cur.meta.get("regions") == regions:
            return cur, self._shots[t.key]
        shot = self.shot(t, fresh=True)
        out = self.worker.call("marks", png=shot.b64, max_marks=max_marks)
        ax_boxes = [shot.to_px(e.bounds) for e in list(ax)[1:] if e.bounds is not None]
        nodes = marks_to_nodes(out["marks"], shot, regions, ax_boxes)
        meta = {"_at": time.monotonic(), "size": list(shot.size), "scale": shot.scale, "origin": list(shot.origin), "regions": regions}
        version, _ = self.maps.record(t.key, build(nodes, shot.origin), "vision", meta)
        return version, shot

    def ground(self, t: Any, target: str, candidates: list[em.Element], zoom: str = "auto") -> dict[str, Any]:
        """Ground ``target`` and snap it to a candidate element (tree or mark).
        A point that hits none becomes a new ``point`` element in the vision map."""
        start = time.perf_counter()
        shot = self.shot(t)
        boxed = [e for e in candidates if e.bounds is not None and e.bounds[2] > 0 and e.bounds[3] > 0]
        # The window element contains everything: never a snap target.
        boxed = [e for e in boxed if not (e.parent is None and e.source == "ax")]
        res = self.worker.call("ground", timeout=240.0, png=shot.b64, instruction=target,
                               boxes=[list(shot.to_px(e.bounds)) for e in boxed], zoom=zoom)
        out: dict[str, Any] = {k: res.get(k) for k in ("status", "confidence", "zoomed", "model") if k in res}
        if res.get("error"):
            out["error"] = res["error"]
        if res.get("status") != "positive" or not res.get("point"):
            out["ms"] = round((time.perf_counter() - start) * 1000, 1)
            return out
        px = tuple(res["point"])
        sx, sy = shot.origin[0] + px[0] / shot.scale, shot.origin[1] + px[1] / shot.scale
        out["point"] = [round(sx, 1), round(sy, 1)]
        if res.get("snapped") is not None:
            e = boxed[int(res["snapped"])]
            out.update(id=e.id, source=e.source)
        else:
            out.update(id=self._add_point(t, target, px, shot), source="vision")
        out["ms"] = round((time.perf_counter() - start) * 1000, 1)
        return out

    def _add_point(self, t: Any, name: str, px: g.Point, shot: Shot) -> str:
        """Add a grounded point as an element of the window's vision map."""
        wm = self.maps.window(t.key)
        cur = wm.current
        half = POINT_SIZE * shot.scale / 2
        box = (px[0] - half, px[1] - half, px[0] + half, px[1] + half)
        node = em.RawNode(key="p", role="point", name=name[:120], bounds=g.px_to_screen(box, shot.origin, shot.scale),
                          actions=("click",), native={"box": box, "hash": None, "mark": None}, source="vision")
        point = build([node], shot.origin)[0]
        elements = [e for e in (cur.elements.values() if cur else []) if e.id != point.id] + [point]
        meta = dict(cur.meta) if cur else {"_at": time.monotonic(), "size": list(shot.size), "scale": shot.scale, "origin": list(shot.origin), "regions": ()}
        self.maps.record(t.key, elements, "vision", meta)
        return point.id

    def locate(self, eid: str) -> tuple[str, em.Element] | None:
        key = self.maps.window_of(eid)
        cur = self.maps.window(key).current if key else None
        e = cur.elements.get(eid) if cur else None
        return (key, e) if key and e is not None else None

    def fresh_point(self, t: Any, e: em.Element) -> tuple[bool, g.Point]:
        """Is the element's region unchanged on screen? Returns (fresh, the
        element's centre in window-local screenshot px). Cua's pid/window
        pixel path anchors these to the window wherever it sits, so a window
        on a secondary or differently scaled display clicks the right spot."""
        shot = self.shot(t, fresh=True)
        # The window-relative image box: still right after the window moved.
        box = tuple((e.native or {}).get("box") or shot.to_px(e.bounds))
        want = (e.native or {}).get("hash")
        if want:
            got = self.worker.call("hash", png=shot.b64, boxes=[list(box)])["hashes"][0]
            if g.hamming(want, got) > HASH_TOLERANCE:
                return False, (0.0, 0.0)
        return True, g.center(box)

    def invalidate(self, key: str) -> None:
        self._shots.pop(key, None)
        wm = self.maps.window(key)
        if wm.current is not None:
            wm.current.meta["_at"] = 0.0


__all__ = ["Vision", "VisionUnavailable", "Shot", "marks_to_nodes", "build", "png_size"]
