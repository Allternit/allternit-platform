"""COM set-up for the UI Automation threads (Windows only).

``import comtypes`` calls CoInitializeEx on the importing thread with
``sys.coinit_flags`` (apartment-threaded when unset). A thread that imports
comtypes and then asks for the multithreaded apartment gets
RPC_E_CHANGED_MODE ("Cannot change thread mode after it is set"), which is
what turned UI Automation off on Windows Desktop. Set the flag before the
first import, and treat "already initialised in the other mode" as usable:
UIA client calls work from either apartment on the thread that made them.
"""

from __future__ import annotations

import sys

RPC_E_CHANGED_MODE = -2147417850  # 0x80010106
COINIT_MULTITHREADED = 0x0


def init_com(multithreaded: bool = True) -> None:
    """Initialise COM on the current thread; never fails on a mode clash."""
    if "comtypes" not in sys.modules and not hasattr(sys, "coinit_flags"):
        sys.coinit_flags = COINIT_MULTITHREADED  # type: ignore[attr-defined]
    import comtypes  # type: ignore

    try:
        if multithreaded:
            comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
        else:
            comtypes.CoInitialize()
    except OSError as e:
        if getattr(e, "winerror", None) != RPC_E_CHANGED_MODE and (e.args[:1] != (RPC_E_CHANGED_MODE,)):
            raise
