"""What the safety layer in allternit-api needs from the screen.

* ``context``: where an action lands — the app (name, bundle id, pid), the
  window title, the page URL when the window is a browser, the element under
  a screen point (AX hit test), and the role/name of map elements by id. The
  executor uses it for irreversible-action classification, watch mode,
  per-app/per-domain allowlists and credential binding.
* ``ocr``: text lines with per-word boxes for one PNG (Apple Vision), so the
  executor can black out personal data in a screenshot before it reaches a
  model. The PNG is the exact image the executor holds, so boxes line up.

macOS only today; other platforms answer ``unsupported`` and the executor
falls back to its own rules (documented in surfaces/docs/tools/computer-safety).
"""

from __future__ import annotations

import base64
import sys
import time
from typing import Any
from urllib.parse import urlparse

WEB_AREA_SEARCH_LIMIT = 400  # AX nodes visited looking for a window's web area.
OCR_MAX_PNG_BYTES = 40 * 1024 * 1024


class Unsupported(Exception):
    pass


def _ms(start: float) -> float:
    return round((time.perf_counter() - start) * 1000, 1)


def _attr(AS: Any, ref: Any, name: str) -> Any:
    try:
        err, value = AS.AXUIElementCopyAttributeValue(ref, name, None)
    except Exception:
        return None
    return value if err == 0 else None


def _text(v: Any) -> str:
    if v is None:
        return ""
    try:
        s = str(v)
    except Exception:
        return ""
    return " ".join(s.split())[:200]


def _describe_ax(AS: Any, ref: Any) -> dict[str, Any]:
    """Role and the name a person would read for one AX element. Static text
    inside a button or link reports the control (a click on a button's label
    clicks the button)."""
    role = _text(_attr(AS, ref, "AXRole"))
    subrole = _text(_attr(AS, ref, "AXSubrole"))
    name = _text(_attr(AS, ref, "AXTitle")) or _text(_attr(AS, ref, "AXDescription"))
    if not name and role in ("AXStaticText", "AXImage"):
        name = _text(_attr(AS, ref, "AXValue"))
    out: dict[str, Any] = {"role": role, "name": name}
    if subrole == "AXSecureTextField" or role == "AXSecureTextField":
        out["secure"] = True
    if role in ("AXStaticText", "AXImage", "AXGroup", ""):
        parent = _attr(AS, ref, "AXParent")
        for _ in range(3):
            if parent is None:
                break
            prole = _text(_attr(AS, parent, "AXRole"))
            if prole in ("AXButton", "AXLink", "AXMenuItem", "AXMenuButton", "AXPopUpButton", "AXCheckBox", "AXRadioButton"):
                pname = _text(_attr(AS, parent, "AXTitle")) or _text(_attr(AS, parent, "AXDescription")) or name
                return {"role": prole, "name": pname}
            parent = _attr(AS, parent, "AXParent")
    return out


def _web_url(AS: Any, window: Any) -> str | None:
    """The URL of the first web area in a window (Safari, Chrome, Edge,
    Arc, Firefox and Electron apps expose ``AXURL`` on it)."""
    queue = [window]
    seen = 0
    while queue and seen < WEB_AREA_SEARCH_LIMIT:
        node = queue.pop(0)
        seen += 1
        if _text(_attr(AS, node, "AXRole")) == "AXWebArea":
            url = _attr(AS, node, "AXURL")
            if url is not None:
                try:
                    return str(url.absoluteString())
                except Exception:
                    return _text(url) or None
        kids = _attr(AS, node, "AXChildren")
        if kids:
            queue.extend(list(kids)[:60])
    return None


def host_of(url: str | None) -> str | None:
    if not url:
        return None
    try:
        host = urlparse(url).hostname
    except ValueError:
        return None
    return host.lower() if host else None


def _screen_scale() -> float:
    try:
        import AppKit  # type: ignore

        screen = AppKit.NSScreen.mainScreen()
        return float(screen.backingScaleFactor()) if screen is not None else 1.0
    except Exception:
        return 1.0


def _app_of(pid: int) -> dict[str, Any]:
    import AppKit  # type: ignore

    app = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
    if app is None:
        return {"pid": pid}
    return {"pid": pid, "app": _text(app.localizedName()), "bundle_id": _text(app.bundleIdentifier())}


def context(p: dict[str, Any], pid_hint: int | None = None, elements: dict[str, Any] | None = None) -> dict[str, Any]:
    """Where an action lands. Params: ``point`` [x, y] in screen pixels (the
    pixel members' space), ``url`` (true to look up the page URL). The caller
    (Driver.context) resolves map ids itself and passes their window's pid."""
    if sys.platform != "darwin":
        raise Unsupported("context is available on macOS only")
    import AppKit  # type: ignore
    import ApplicationServices as AS  # type: ignore

    start = time.perf_counter()
    out: dict[str, Any] = {"scale": _screen_scale()}
    pid = pid_hint
    point = p.get("point")
    if isinstance(point, list) and len(point) == 2:
        scale = out["scale"] or 1.0
        x, y = float(point[0]) / scale, float(point[1]) / scale
        system = AS.AXUIElementCreateSystemWide()
        try:
            err, hit = AS.AXUIElementCopyElementAtPosition(system, x, y, None)
        except Exception:
            err, hit = -1, None
        if err == 0 and hit is not None:
            out["at_point"] = _describe_ax(AS, hit)
            try:
                err, hpid = AS.AXUIElementGetPid(hit, None)
                if err == 0 and hpid:
                    pid = int(hpid)
            except Exception:
                pass
    if pid is None:
        front = AppKit.NSWorkspace.sharedWorkspace().frontmostApplication()
        if front is not None:
            pid = int(front.processIdentifier())
    if pid is not None:
        out.update(_app_of(pid))
        app_ref = AS.AXUIElementCreateApplication(pid)
        window = _attr(AS, app_ref, "AXFocusedWindow") or _attr(AS, app_ref, "AXMainWindow")
        if window is not None:
            out["title"] = _text(_attr(AS, window, "AXTitle"))
            if p.get("url", True):
                url = _web_url(AS, window)
                if url:
                    out["url"] = url
                    out["host"] = host_of(url)
    if elements:
        out["elements"] = elements
    out["ms"] = _ms(start)
    return out


def ocr(p: dict[str, Any]) -> dict[str, Any]:
    """Text lines with per-word boxes for one PNG, in that image's pixels
    (top-left origin). Params: ``png`` (base64), ``level`` fast|accurate."""
    if sys.platform != "darwin":
        raise Unsupported("ocr is available on macOS only")
    import objc  # type: ignore
    import Quartz  # type: ignore  # noqa: F401  (loads CoreGraphics for Vision)
    import Vision  # type: ignore
    from Foundation import NSData  # type: ignore

    start = time.perf_counter()
    raw = base64.b64decode(str(p.get("png") or ""), validate=False)
    if not raw:
        raise ValueError("ocr needs png (base64)")
    if len(raw) > OCR_MAX_PNG_BYTES:
        raise ValueError("png is too large for ocr")
    width, height = _png_size(raw)
    lines: list[dict[str, Any]] = []
    with objc.autorelease_pool():
        data = NSData.dataWithBytes_length_(raw, len(raw))
        request = Vision.VNRecognizeTextRequest.alloc().init()
        accurate = str(p.get("level") or "fast") == "accurate"
        request.setRecognitionLevel_(
            getattr(Vision, "VNRequestTextRecognitionLevelAccurate", 0) if accurate else getattr(Vision, "VNRequestTextRecognitionLevelFast", 1)
        )
        request.setUsesLanguageCorrection_(False)
        handler = Vision.VNImageRequestHandler.alloc().initWithData_options_(data, None)
        ok = handler.performRequests_error_([request], None)
        if isinstance(ok, tuple):
            ok = ok[0]
        if not ok:
            raise RuntimeError("Vision text recognition failed")
        for obs in list(request.results() or []):
            cands = obs.topCandidates_(1)
            if not cands:
                continue
            cand = cands[0]
            text = str(cand.string() or "")
            if not text.strip():
                continue
            line = {"text": text, "box": _px(obs.boundingBox(), width, height), "words": []}
            for start_i, end_i in _word_spans(text):
                box = _range_box(cand, start_i, end_i)
                if box is not None:
                    line["words"].append({"start": start_i, "end": end_i, "box": _px(box, width, height)})
            lines.append(line)
    return {"width": width, "height": height, "lines": lines, "ms": _ms(start)}


def _word_spans(text: str) -> list[tuple[int, int]]:
    spans, i, n = [], 0, len(text)
    while i < n:
        while i < n and text[i].isspace():
            i += 1
        j = i
        while j < n and not text[j].isspace():
            j += 1
        if j > i:
            spans.append((i, j))
        i = j
    return spans


def _range_box(cand: Any, start: int, end: int) -> Any:
    try:
        from Foundation import NSMakeRange  # type: ignore

        res = cand.boundingBoxForRange_error_(NSMakeRange(start, end - start), None)
        obs = res[0] if isinstance(res, tuple) else res
        return obs.boundingBox() if obs is not None else None
    except Exception:
        return None


def _px(rect: Any, width: int, height: int) -> list[int]:
    """Vision's normalized, bottom-left rect as [x0, y0, x1, y1] image px."""
    try:
        x, y, w, h = rect.origin.x, rect.origin.y, rect.size.width, rect.size.height
    except AttributeError:
        (x, y), (w, h) = rect
    x0 = max(0, int(x * width))
    x1 = min(width, int(round((x + w) * width)) + 1)
    y0 = max(0, int((1.0 - y - h) * height))
    y1 = min(height, int(round((1.0 - y) * height)) + 1)
    return [x0, y0, x1, y1]


def _png_size(raw: bytes) -> tuple[int, int]:
    if raw[:8] == b"\x89PNG\r\n\x1a\n" and len(raw) >= 24:
        return int.from_bytes(raw[16:20], "big"), int.from_bytes(raw[20:24], "big")
    raise ValueError("ocr takes a PNG")
