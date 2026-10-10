"""arc-driver, forked into the Allternit Driver (see ../NOTICE).

Only the policy-free macOS driver is kept. The Allternit Driver imports
``Driver`` from here and owns ids, routing and locking itself.
"""

from .driver import ActResult, Driver, SettleReport, WindowTarget
from .models import ActionKind, Bounds, DesktopElement, DesktopSnapshot

__all__ = [
    "ActResult",
    "ActionKind",
    "Bounds",
    "DesktopElement",
    "DesktopSnapshot",
    "Driver",
    "SettleReport",
    "WindowTarget",
]
