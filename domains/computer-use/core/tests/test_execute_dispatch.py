"""Unit tests for the /v1/execute dispatch of the computer_20250124/20251124
action set (cu8-exec).

Covers the ten actions that previously fell through to handle_stub:
mouse_move, left_click, left_click_drag, middle_click, left_mouse_down,
left_mouse_up, cursor_position, hold_key, wait, zoom.

The browser Playwright layer is mocked via a fake session_manager/page and the
desktop layer via a fake accessibility adapter, so no real browser or desktop
session is needed.

Import shim note: this repo has an outer wrapper package (this directory) and
an inner execution package (core/core/). tests/test_replay.py may purge and
re-cache `core` as the inner package; merging both package paths into whichever
`core` is cached here keeps core.canonical_runtime (inner) and core.tests
(outer) resolving through the same module.
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import pytest

CORE_ROOT = Path(__file__).resolve().parents[1]
GATEWAY_DIR = CORE_ROOT / "gateway"
for _p in (str(GATEWAY_DIR), str(CORE_ROOT)):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import core as _core_mod  # noqa: E402

_paths = list(getattr(_core_mod, "__path__", []))
for _p in (str(CORE_ROOT), str(CORE_ROOT / "core")):
    if _p not in _paths:
        _paths.append(_p)
_core_mod.__path__[:] = _paths

# NOTE: `import main` is deliberately deferred to a fixture. tests/test_replay.py
# purges and re-imports the inner `core` package at module import time; importing
# the gateway here at module level would bind its routers to the purged module
# objects and break test_replay's monkeypatching.


class FakeMouse:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    async def move(self, x, y, steps: int | None = None) -> None:
        self.calls.append(("move", x, y, steps))

    async def click(self, x=None, y=None, button: str = "left") -> None:
        self.calls.append(("click", x, y, button))

    async def down(self, **kwargs) -> None:
        self.calls.append(("down", kwargs.get("button", "left")))

    async def up(self, **kwargs) -> None:
        self.calls.append(("up", kwargs.get("button", "left")))


class FakeKeyboard:
    def __init__(self) -> None:
        self.calls: list[tuple] = []

    async def down(self, key: str) -> None:
        self.calls.append(("down", key))

    async def up(self, key: str) -> None:
        self.calls.append(("up", key))


class FakePage:
    def __init__(self) -> None:
        self.mouse = FakeMouse()
        self.keyboard = FakeKeyboard()


class FakeSessionManager:
    """Returns a FakePage for the browser path, or None to force desktop fallback."""

    def __init__(self, page: FakePage | None) -> None:
        self._page = page

    async def get_page(self, session_id: str):
        return self._page


class FakeDesktop:
    """Stands in for the accessibility adapter behind gateway._desktop_input."""

    def __init__(self, position=(10, 20)) -> None:
        self.calls: list[tuple] = []
        self._position = position

    async def execute(self, action: str, params: dict) -> dict:
        if self._position is None:
            return {"success": False, "error": "no native desktop"}
        self.calls.append((action, dict(params)))
        return {"success": True}

    def cursor_position(self):
        return self._position


@pytest.fixture(scope="module")
def gw():
    import main
    return main


def run_action(gw, action: str, parameters: dict | None = None):
    req = gw.ExecuteRequest(
        action=action,
        session_id="test-session",
        run_id="test-run",
        parameters=parameters or {},
    )
    return asyncio.run(gw.execute(req))


@pytest.fixture
def browser_page(gw, monkeypatch):
    page = FakePage()
    monkeypatch.setattr(gw, "session_manager", FakeSessionManager(page))
    return page


@pytest.fixture
def desktop_only(gw, monkeypatch):
    """No browser session; returns the fake desktop adapter the gateway uses."""
    fake = FakeDesktop()
    monkeypatch.setattr(gw, "session_manager", FakeSessionManager(None))
    monkeypatch.setattr(gw, "_desktop_input_adapter", fake)
    return fake


@pytest.fixture
def no_backend(gw, monkeypatch):
    monkeypatch.setattr(gw, "session_manager", FakeSessionManager(None))
    monkeypatch.setattr(gw, "_desktop_input_adapter", FakeDesktop(position=None))


def test_mouse_move_browser(gw, browser_page):
    resp = run_action(gw, "mouse_move", {"coordinate": [100, 200]})
    assert resp.status == "completed"
    assert ("move", 100.0, 200.0, None) in browser_page.mouse.calls
    assert resp.receipts[0].action == "mouse_move"
    assert isinstance(resp.artifacts, list)


def test_mouse_move_requires_coordinate(gw, browser_page):
    resp = run_action(gw, "mouse_move")
    assert resp.status == "failed"
    assert resp.error.code == "MOUSE_MOVE_ERROR"


def test_mouse_move_desktop_fallback(gw, desktop_only):
    resp = run_action(gw, "mouse_move", {"x": 50, "y": 60})
    assert resp.status == "completed"
    assert ("hover", {"x": 50.0, "y": 60.0}) in desktop_only.calls


def test_left_click_browser(gw, browser_page):
    resp = run_action(gw, "left_click", {"coordinate": [10, 20]})
    assert resp.status == "completed"
    assert ("click", 10.0, 20.0, "left") in browser_page.mouse.calls


def test_left_click_current_position_desktop(gw, desktop_only):
    resp = run_action(gw, "left_click")
    assert resp.status == "completed"
    assert ("click", {"x": 10.0, "y": 20.0}) in desktop_only.calls


def test_middle_click_browser(gw, browser_page):
    resp = run_action(gw, "middle_click", {"coordinate": [5, 6]})
    assert resp.status == "completed"
    assert ("click", 5.0, 6.0, "middle") in browser_page.mouse.calls


def test_left_click_drag_browser(gw, browser_page):
    resp = run_action(gw, "left_click_drag", {"coordinate": [0, 0], "to": [300, 400]})
    assert resp.status == "completed"
    moves = [c for c in browser_page.mouse.calls if c[0] == "move"]
    assert (moves[0][1], moves[0][2]) == (0.0, 0.0)
    assert (moves[-1][1], moves[-1][2]) == (300.0, 400.0)
    assert ("down", "left") in browser_page.mouse.calls
    assert ("up", "left") in browser_page.mouse.calls


def test_left_click_drag_desktop_fallback(gw, desktop_only):
    resp = run_action(gw, "left_click_drag", {"coordinate": [1, 2], "to_x": 3, "to_y": 4})
    assert resp.status == "completed"
    assert ("drag", {"from_x": 1.0, "from_y": 2.0, "to_x": 3.0, "to_y": 4.0, "duration": 0.5}) in desktop_only.calls


def test_left_click_drag_requires_target(gw, browser_page):
    resp = run_action(gw, "left_click_drag", {"coordinate": [1, 2]})
    assert resp.status == "failed"
    assert resp.error.code == "LEFT_CLICK_DRAG_ERROR"


def test_left_mouse_down_up_browser(gw, browser_page):
    down = run_action(gw, "left_mouse_down", {"coordinate": [7, 8]})
    up = run_action(gw, "left_mouse_up")
    assert down.status == "completed"
    assert up.status == "completed"
    assert ("move", 7.0, 8.0, None) in browser_page.mouse.calls
    assert ("down", "left") in browser_page.mouse.calls
    assert ("up", "left") in browser_page.mouse.calls


def test_left_mouse_down_desktop_fallback(gw, desktop_only):
    resp = run_action(gw, "left_mouse_down", {"coordinate": [9, 9]})
    assert resp.status == "completed"
    assert ("mouse_down", {"x": 9.0, "y": 9.0}) in desktop_only.calls


def test_cursor_position_desktop(gw, desktop_only):
    resp = run_action(gw, "cursor_position")
    assert resp.status == "completed"
    assert resp.extracted_content == {"x": 10.0, "y": 20.0}
    assert resp.adapter_id == "desktop.accessibility"


def test_cursor_position_explicit_error_without_backend(gw, no_backend):
    resp = run_action(gw, "cursor_position")
    assert resp.status == "failed"
    assert resp.error.code == "CURSOR_POSITION_ERROR"
    assert "not supported on this platform" in resp.error.message


def test_hold_key_browser(gw, browser_page):
    resp = run_action(gw, "hold_key", {"key": "shift", "duration": 0.001})
    assert resp.status == "completed"
    assert ("down", "shift") in browser_page.keyboard.calls
    assert ("up", "shift") in browser_page.keyboard.calls


def test_hold_key_requires_key(gw, browser_page):
    resp = run_action(gw, "hold_key", {"duration": 0.001})
    assert resp.status == "failed"
    assert resp.error.code == "HOLD_KEY_ERROR"


def test_hold_key_desktop_fallback(gw, desktop_only):
    resp = run_action(gw, "hold_key", {"key": "a", "duration": 0.001})
    assert resp.status == "completed"
    assert ("key_down", {"key": "a"}) in desktop_only.calls
    assert ("key_up", {"key": "a"}) in desktop_only.calls


def test_wait(gw):
    resp = run_action(gw, "wait", {"duration": 0.001})
    assert resp.status == "completed"
    assert resp.receipts[0].action == "wait"
    assert resp.extracted_content["duration"] == 0.001


def test_zoom_explicit_unsupported(gw, no_backend):
    resp = run_action(gw, "zoom", {"region": [0, 0, 100, 100]})
    assert resp.status == "failed"
    assert resp.error.code == "UNSUPPORTED_PLATFORM"
    assert "zoom" in resp.error.message


def test_unsupported_action_rejected_by_schema(gw, no_backend):
    import pydantic_core

    with pytest.raises(pydantic_core.ValidationError):
        gw.ExecuteRequest(
            action="not_a_real_action",
            session_id="test-session",
            run_id="test-run",
            parameters={},
        )
