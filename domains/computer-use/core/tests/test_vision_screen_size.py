"""The planner's coordinate space is the screenshot's real size, not 1280x720."""

from __future__ import annotations

import base64
import struct
import sys
import zlib
from pathlib import Path

COMPUTER_USE_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(COMPUTER_USE_ROOT))

from core import vision_providers as vp  # noqa: E402


def _png(width: int, height: int) -> bytes:
    def chunk(kind: bytes, body: bytes) -> bytes:
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IEND", b"")


def _jpeg(width: int, height: int) -> bytes:
    app0 = b"\xff\xe0" + struct.pack(">H", 16) + b"JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00"
    sof0 = b"\xff\xc0" + struct.pack(">HBHHB", 11, 8, height, width, 1) + b"\x01\x11\x00"
    return b"\xff\xd8" + app0 + sof0 + b"\xff\xd9"


def test_reads_png_and_jpeg_sizes_from_bytes_and_base64() -> None:
    png = _png(1920, 1080)
    assert vp.image_size(png) == (1920, 1080)
    assert vp.image_size(base64.b64encode(png).decode()) == (1920, 1080)
    assert vp.image_size("data:image/png;base64," + base64.b64encode(_png(2560, 1600)).decode()) == (2560, 1600)
    assert vp.image_size(_jpeg(1440, 900)) == (1440, 900)
    assert vp.image_size(b"not an image") is None


def test_resolve_prefers_session_size_then_screenshot_then_cloud_default() -> None:
    shot = base64.b64encode(_png(1920, 1080)).decode()
    assert vp.resolve_screen_size(shot) == (1920, 1080)
    assert vp.resolve_screen_size(shot, (3024, 1964)) == (3024, 1964)
    assert vp.resolve_screen_size("") == vp.FALLBACK_SCREEN_SIZE == (1920, 1080)


def test_subprocess_brain_prompt_uses_the_real_size(monkeypatch) -> None:
    import asyncio

    seen: list = []

    def fake_prompt(task, history_text, screen_size):
        seen.append(screen_size)
        raise RuntimeError("stop after the prompt is built")

    monkeypatch.setattr(vp, "_build_planning_prompt", fake_prompt)
    provider = vp.SubprocessVisionProvider(cmd="true")
    shot = base64.b64encode(_png(1920, 1080)).decode()
    try:
        asyncio.run(provider.ground_and_reason(shot, "task"))
    except Exception:
        pass
    assert seen == [(1920, 1080)]
