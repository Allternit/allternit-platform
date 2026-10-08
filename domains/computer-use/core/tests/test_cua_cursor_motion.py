"""Cua Driver 0.34 agent cursor motion: start_session carries the style."""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path
from unittest.mock import AsyncMock

COMPUTER_USE_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(COMPUTER_USE_ROOT))

from providers.cua_driver_canonical import CuaDriverCanonicalProvider  # noqa: E402
from providers.cua_driver_transport import (  # noqa: E402
    CURSOR_MOTION_ENV,
    CuaDriverTransport,
    cursor_motion_style,
    start_session_arguments,
)


def test_style_defaults_to_signature_arc_and_follows_the_setting(monkeypatch) -> None:
    monkeypatch.delenv(CURSOR_MOTION_ENV, raising=False)
    assert cursor_motion_style() == "signature_arc"
    monkeypatch.setenv(CURSOR_MOTION_ENV, "Comet_Swoop")
    assert cursor_motion_style() == "comet_swoop"
    assert cursor_motion_style("magnetic") == "magnetic", "an explicit choice wins"
    monkeypatch.setenv(CURSOR_MOTION_ENV, "teleport")
    assert cursor_motion_style() == "signature_arc", "unknown styles fall back"


def test_start_session_arguments_keep_reduced_motion_with_the_os(monkeypatch) -> None:
    monkeypatch.delenv(CURSOR_MOTION_ENV, raising=False)
    monkeypatch.delenv("ALLTERNIT_CUA_REDUCED_MOTION", raising=False)
    assert start_session_arguments("run-1") == {
        "session": "run-1",
        "cursor_motion": {"style": "signature_arc"},
        "cursor_theme": {"reduced_motion": "auto"},
    }


def test_provider_starts_each_session_once_and_tolerates_old_drivers(tmp_path: Path) -> None:
    transport = AsyncMock(spec=CuaDriverTransport)
    transport.start_session = AsyncMock(side_effect=RuntimeError("unknown tool start_session"))
    provider = CuaDriverCanonicalProvider(transport, tmp_path)

    async def go() -> None:
        await provider._ensure_session("run-1")
        await provider._ensure_session("run-1")
        await provider._ensure_session("run-2")

    asyncio.run(go())
    assert [c.args[0] for c in transport.start_session.await_args_list] == ["run-1", "run-2"]
