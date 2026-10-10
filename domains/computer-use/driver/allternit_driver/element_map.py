"""The Allternit element map: our own stable element ids and map versions.

Every engine read becomes a list of ``RawNode`` (role, name, value, bounds,
parent). ``ElementMap`` turns that into elements whose ids stay the same
across reads and across engines:

* id = hash(role, name, tree path). The tree path is the chain of
  ``role:ordinal`` segments from the window, where the ordinal counts only
  siblings with the same role *and* name, so inserting a row with another
  name does not shift anyone's id.
* Two nodes that still collide (identical twins under one parent) get the
  window-relative bbox, quantized to 8 pt, mixed into the hash.
* An optional crop hash (a hash of the element's pixels) is attached when the
  read included a screenshot; the vision fallback and replay use it.
* Every element records its ``source``: ``ax`` (the accessibility tree) or
  ``vision`` (a set-of-marks region or a grounded point, see vision/). Vision
  ids start with ``v`` instead of ``e`` so ``act`` can route them without a
  lookup, and they never collide with tree ids.

Each window keeps a short history of versions. A version changes only when
something a model can see changed (ids, names, values, bounds, state), so a
re-read of an idle window returns the same version. Actions carry the version
they were planned on; ``check`` refuses an old one so the caller can answer
with the fresh map (arc's freshness guard, on our ids).
"""

from __future__ import annotations

import hashlib
import threading
from collections import OrderedDict
from dataclasses import dataclass, field
from typing import Any, Iterable

HISTORY = 8  # Versions kept per window for ``since`` diffs.
QUANTUM = 8.0  # Points; bbox quantization for twin disambiguation.


@dataclass(slots=True)
class RawNode:
    """One element as an engine read it. ``native`` is the engine's own handle
    (arc element id, Cua element token); it never leaves the sidecar."""

    key: str  # Engine-local key, unique within one read.
    role: str
    name: str = ""
    value: Any = None
    bounds: tuple[float, float, float, float] | None = None  # Screen points x, y, w, h.
    parent: str | None = None  # ``key`` of the parent node.
    enabled: bool = True
    focused: bool = False
    actions: tuple[str, ...] = ()
    native: Any = None
    source: str = "ax"  # ax | vision


@dataclass(slots=True)
class Element:
    id: str
    role: str
    name: str
    value: Any
    bounds: tuple[float, float, float, float] | None
    path: str
    parent: str | None
    enabled: bool
    focused: bool
    actions: tuple[str, ...]
    native: Any = field(default=None, repr=False, compare=False)
    crop: str | None = None
    source: str = "ax"
    mark: int | None = None  # Set-of-marks number (vision elements).

    def public(self) -> dict[str, Any]:
        """What a model sees: compact, no engine handles, defaults omitted."""
        out: dict[str, Any] = {"id": self.id, "role": self.role}
        if self.name:
            out["name"] = self.name
        if self.value not in (None, ""):
            out["value"] = self.value
        if self.bounds is not None:
            out["bbox"] = [round(v) for v in self.bounds]
        if not self.enabled:
            out["enabled"] = False
        if self.focused:
            out["focused"] = True
        if self.actions:
            out["actions"] = list(self.actions)
        if self.parent:
            out["parent"] = self.parent
        if self.crop:
            out["crop"] = self.crop
        if self.source != "ax":
            out["source"] = self.source
        if self.mark is not None:
            out["mark"] = self.mark
        return out

    def signature(self) -> tuple:
        b = tuple(round(v) for v in self.bounds) if self.bounds else None
        return (self.id, self.role, self.name, _text(self.value), b, self.enabled, self.focused)


def _text(value: Any) -> str:
    return "" if value is None else str(value)


def _short_role(role: str) -> str:
    return role[2:] if role.startswith("AX") else role


def _hash(*parts: Any) -> str:
    h = hashlib.blake2b(digest_size=6)
    for part in parts:
        h.update(str(part).encode("utf-8", "surrogatepass"))
        h.update(b"\x1f")
    return h.hexdigest()


def build(nodes: Iterable[RawNode], origin: tuple[float, float] = (0.0, 0.0)) -> list[Element]:
    """Stable ids for one read, in the read's order. ``origin`` is the window's
    top-left, so moving the window does not change twin ids."""
    nodes = list(nodes)
    by_key = {n.key: n for n in nodes}
    paths: dict[str, str] = {}
    ordinals: dict[tuple[str | None, str, str], int] = {}

    def path_of(node: RawNode) -> str:
        cached = paths.get(node.key)
        if cached is not None:
            return cached
        parent = by_key.get(node.parent) if node.parent is not None else None
        prefix = path_of(parent) if parent is not None and parent.key != node.key else ""
        slot = (node.parent, node.role, node.name)
        n = ordinals.get(slot, 0)
        ordinals[slot] = n + 1
        paths[node.key] = result = f"{prefix}/{_short_role(node.role)}:{n}"
        return result

    # Parents first, so ordinals follow document order.
    for node in nodes:
        path_of(node)

    elements: list[Element] = []
    seen: dict[str, int] = {}
    ids: dict[str, str] = {}
    for node in nodes:
        path = paths[node.key]
        prefix = "v" if node.source == "vision" else "e"
        eid = prefix + _hash(node.role, node.name, path)
        if eid in seen:
            box = ""
            if node.bounds is not None:
                x, y, w, h = node.bounds
                box = tuple(round(v / QUANTUM) for v in (x - origin[0], y - origin[1], w, h))
            eid = prefix + _hash(node.role, node.name, path, box, seen[eid])
        seen[eid] = seen.get(eid, 0) + 1
        ids[node.key] = eid
        elements.append(
            Element(
                id=eid,
                role=_short_role(node.role),
                name=node.name or "",
                value=node.value,
                bounds=node.bounds,
                path=path,
                parent=None,
                enabled=node.enabled,
                focused=node.focused,
                actions=tuple(node.actions),
                native=node.native,
                source=node.source,
            )
        )
    for node, element in zip(nodes, elements):
        if node.parent is not None:
            element.parent = ids.get(node.parent)
    return elements


@dataclass(slots=True)
class Version:
    number: int
    elements: "OrderedDict[str, Element]"
    signature: str
    engine: str
    meta: dict[str, Any]


class StaleVersion(Exception):
    def __init__(self, current: int) -> None:
        super().__init__(f"the element map changed (now version {current})")
        self.current = current


class WindowMap:
    """Version history of one window's element map."""

    def __init__(self, key: str) -> None:
        self.key = key
        self._versions: "OrderedDict[int, Version]" = OrderedDict()
        self._next = 1

    @property
    def current(self) -> Version | None:
        return next(reversed(self._versions.values()), None)

    def update(self, elements: list[Element], engine: str, meta: dict[str, Any] | None = None) -> tuple[Version, bool]:
        """Record a read. Returns the version and whether it is new."""
        sig = _hash(*(e.signature() for e in elements))
        cur = self.current
        mapped = OrderedDict((e.id, e) for e in elements)
        if cur is not None and cur.signature == sig and cur.engine == engine:
            # Same content: keep the number, refresh engine handles and meta.
            cur.elements = mapped
            cur.meta = meta or cur.meta
            return cur, False
        version = Version(self._next, mapped, sig, engine, meta or {})
        self._next += 1
        self._versions[version.number] = version
        while len(self._versions) > HISTORY:
            self._versions.popitem(last=False)
        return version, True

    def get(self, number: int) -> Version | None:
        return self._versions.get(number)

    def check(self, number: int | None) -> Version:
        """The current version, or ``StaleVersion`` when ``number`` is older.
        ``None`` means the caller did not pin a version (pixel-planned steps)."""
        cur = self.current
        if cur is None:
            raise StaleVersion(0)
        if number is not None and number != cur.number:
            raise StaleVersion(cur.number)
        return cur

    def diff(self, since: int) -> dict[str, Any] | None:
        """Changes from version ``since`` to the current one; None when that
        version is no longer kept (the caller sends the full map)."""
        old = self._versions.get(since)
        cur = self.current
        if old is None or cur is None:
            return None
        added, changed = [], []
        for eid, element in cur.elements.items():
            before = old.elements.get(eid)
            if before is None:
                added.append(element.public())
            elif before.signature() != element.signature():
                changed.append(element.public())
        removed = [eid for eid in old.elements if eid not in cur.elements]
        return {"since": since, "added": added, "changed": changed, "removed": removed}


class ElementMaps:
    """All windows' maps, plus an index from element id to its window."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._windows: dict[str, WindowMap] = {}
        self._owner: dict[str, str] = {}

    def window(self, key: str) -> WindowMap:
        with self._lock:
            wm = self._windows.get(key)
            if wm is None:
                wm = self._windows[key] = WindowMap(key)
            return wm

    def record(self, key: str, elements: list[Element], engine: str, meta: dict[str, Any] | None = None) -> tuple[Version, bool]:
        wm = self.window(key)
        version, new = wm.update(elements, engine, meta)
        with self._lock:
            for eid in version.elements:
                self._owner[eid] = key
        return version, new

    def window_of(self, element_id: str) -> str | None:
        with self._lock:
            return self._owner.get(element_id)

    def forget(self, key: str) -> None:
        with self._lock:
            self._windows.pop(key, None)
            self._owner = {e: w for e, w in self._owner.items() if w != key}


def select(elements: Iterable[Element], query: str | None, limit: int | None) -> tuple[list[Element], int]:
    """Elements matching ``query`` (case-insensitive substring of role, name or
    value) plus their ancestors, capped at ``limit``. Returns (kept, total)."""
    elements = list(elements)
    total = len(elements)
    if query:
        q = query.lower()
        by_id = {e.id: e for e in elements}
        keep: set[str] = set()
        for e in elements:
            if q in e.name.lower() or q in e.role.lower() or q in _text(e.value).lower():
                cur: Element | None = e
                while cur is not None and cur.id not in keep:
                    keep.add(cur.id)
                    cur = by_id.get(cur.parent) if cur.parent else None
        elements = [e for e in elements if e.id in keep]
    if limit is not None and limit > 0:
        elements = elements[:limit]
    return elements, total
