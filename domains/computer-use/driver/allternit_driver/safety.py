"""What the safety layer in allternit-api needs from the screen.

* ``context``: where an action lands — the app (name, bundle id, pid), the
  window title, the page URL when the window is a browser, the element under
  a screen point (AX hit test), and the role/name of map elements by id. The
  executor uses it for irreversible-action classification, watch mode,
  per-app/per-domain allowlists and credential binding.
* ``ocr``: text lines with per-word boxes for one PNG (Apple Vision), so the
  executor can black out personal data in a screenshot before it reaches a
  model. The PNG is the exact image the executor holds, so boxes line up.
  macOS only; other platforms answer ``unsupported`` and the executor keeps
  its rules-based redaction.

``context`` runs on macOS (AX hit test), Linux (AT-SPI) and Windows (UIA);
where it can't gather anything it answers ``unsupported`` and the executor
falls back to its own rules (documented in surfaces/docs/tools/computer-safety).
"""

from __future__ import annotations

import base64
import os
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
    if sys.platform == "darwin":
        return _context_macos(p, pid_hint, elements)
    if sys.platform.startswith("linux"):
        return _context_atspi(p, pid_hint, elements)
    if sys.platform == "win32":
        return _context_uia(p, pid_hint, elements)
    raise Unsupported("context is available on macOS, Linux and Windows only")


def _context_macos(p: dict[str, Any], pid_hint: int | None, elements: dict[str, Any] | None) -> dict[str, Any]:
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


def _context_atspi(p: dict[str, Any], pid_hint: int | None, elements: dict[str, Any] | None) -> dict[str, Any]:
    """Linux safety context: app name, window title and the element under a
    point, from the AT-SPI tree (the same connection the engine uses). No page
    URL: browsers don't publish one over AT-SPI."""
    try:
        import pyatspi  # type: ignore
    except Exception as e:
        raise Unsupported(f"AT-SPI isn't available: {e}") from e
    start = time.perf_counter()
    try:
        desktop = pyatspi.Registry.getDesktop(0)
    except Exception as e:
        raise Unsupported(f"the accessibility bus isn't answering: {e}") from e
    out: dict[str, Any] = {"scale": 1.0}
    pid = pid_hint
    point = p.get("point")
    try:
        apps = [desktop.getChildAtIndex(i) for i in range(desktop.getChildCount())]
        apps = [a for a in apps if a is not None]
    except Exception as e:
        raise Unsupported(f"the accessibility desktop is empty: {e}") from e
    if isinstance(point, list) and len(point) == 2:
        x, y = float(point[0]), float(point[1])
        for app in apps:
            if pid is not None:
                break
            for i in range(app.getChildCount()):
                try:
                    win = app.getChildAtIndex(i)
                    if win is None or win.getRoleName().lower() not in ("frame", "dialog", "window"):
                        continue
                    comp = win.queryComponent()
                    ext = comp.getExtents(0)
                    if ext.x <= x <= ext.x + ext.width and ext.y <= y <= ext.y + ext.height:
                        hit = comp.getAccessibleAtPoint(x, y, 0)
                        if hit is not None:
                            out["at_point"] = {"role": _text(hit.getRoleName()), "name": _text(hit.getName())}
                        pid = _atspi_pid(app)
                        out["title"] = _text(win.getName())
                        break
                except Exception:
                    continue
    if pid is None:
        for app in apps:
            got = _atspi_pid(app)
            if got is not None:
                try:
                    name = app.getName()
                except Exception:
                    name = ""
                if name:
                    out["app"] = _text(name)
                    break
    else:
        for app in apps:
            if _atspi_pid(app) == pid:
                try:
                    out["app"] = _text(app.getName())
                except Exception:
                    pass
                try:
                    for i in range(app.getChildCount()):
                        win = app.getChildAtIndex(i)
                        if win is not None and win.getRoleName().lower() in ("frame", "dialog", "window"):
                            title = _text(win.getName())
                            if title:
                                out["title"] = title
                            break
                except Exception:
                    pass
                break
    if elements:
        out["elements"] = elements
    out["ms"] = _ms(start)
    return out


def _atspi_pid(app: Any) -> int | None:
    get_pid = getattr(app, "getProcessId", None)
    if get_pid is not None:
        try:
            return int(get_pid())
        except Exception:
            pass
    return None


def _context_uia(p: dict[str, Any], pid_hint: int | None, elements: dict[str, Any] | None) -> dict[str, Any]:
    """Windows safety context: app, window title and the element under a
    point, from UI Automation. No page URL (the address bar is just an edit
    control; URLs stay the browser toolset's business)."""
    try:
        import comtypes  # type: ignore
        import comtypes.client  # type: ignore

        comtypes.CoInitialize()
        comtypes.client.GetModule("UIAutomationCore.dll")
        from comtypes.gen import UIAutomationClient as U  # type: ignore

        automation = comtypes.client.CreateObject(U.CUIAutomation, interface=U.IUIAutomation)
    except Exception as e:
        raise Unsupported(f"UI Automation isn't available: {e}") from e
    import ctypes

    start = time.perf_counter()
    out: dict[str, Any] = {"scale": 1.0}
    pid = pid_hint
    point = p.get("point")
    if isinstance(point, list) and len(point) == 2:
        try:
            pt = ctypes.wintypes.POINT(x=int(float(point[0])), y=int(float(point[1])))
            el = automation.ElementFromPoint(pt)
            if el is not None:
                out["at_point"] = {"role": _uia_role(int(el.GetCurrentPropertyValue(30003) or 0)),
                                   "name": _text(el.GetCurrentPropertyValue(30005))}
                try:
                    pid = int(el.GetCurrentPropertyValue(30002))
                except Exception:
                    pass
        except Exception:
            pass
    if pid is None:
        out["ms"] = _ms(start)
        if elements:
            out["elements"] = elements
        return out
    user32 = ctypes.windll.user32
    hwnd_holder: list[int] = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def enum(hwnd: int, _: Any) -> bool:
        win_pid = ctypes.c_ulong(0)
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(win_pid))
        if int(win_pid.value) == pid and user32.IsWindowVisible(hwnd):
            hwnd_holder.append(int(hwnd))
            return False
        return True

    user32.EnumWindows(enum, None)
    if hwnd_holder:
        title_buf = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(hwnd_holder[0], title_buf, 512)
        if title_buf.value:
            out["title"] = _text(title_buf.value)
    try:
        kernel32 = ctypes.windll.kernel32
        psapi = ctypes.windll.psapi
        handle = kernel32.OpenProcess(0x1000, False, pid)
        if handle:
            try:
                buf = ctypes.create_unicode_buffer(512)
                if psapi.GetModuleFileNameExW(handle, None, buf, 512):
                    out["app"] = _text(os.path.basename(buf.value))
            finally:
                kernel32.CloseHandle(handle)
    except Exception:
        pass
    out["pid"] = pid
    if elements:
        out["elements"] = elements
    out["ms"] = _ms(start)
    return out


_UIA_ROLES = {
    50000: "button", 50002: "checkbox", 50003: "combobox", 50004: "edit", 50005: "link",
    50006: "image", 50008: "list", 50009: "menu", 50011: "menuitem", 50013: "radiobutton",
    50015: "slider", 50018: "tab", 50020: "text", 50024: "treeitem", 50032: "window", 50033: "pane",
}


def _uia_role(control_type: int) -> str:
    return _UIA_ROLES.get(control_type, f"control:{control_type}")


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
