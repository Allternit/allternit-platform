"""python -m allternit_driver --listen unix:<path> [--cua <cua-driver>] ...

Started by Allternit Desktop (computer-use-driver-manager.ts) next to Cua
Driver's daemon. Exits on SIGTERM, and when its parent goes away.
"""

from __future__ import annotations

import argparse
import logging
import os
import signal
import sys
import time

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(HERE, "arc"))  # The forked arc-driver package.

from .core import Driver  # noqa: E402
from .engines.cua import CuaEngine  # noqa: E402
from .server import serve  # noqa: E402


def main() -> None:
    ap = argparse.ArgumentParser(prog="allternit_driver")
    ap.add_argument("--listen", required=True, help="unix:<path> or tcp:127.0.0.1:0")
    ap.add_argument("--endpoint-file", help="write the bound endpoint here (tcp port 0)")
    ap.add_argument("--cua", default=os.environ.get("ALLTERNIT_CUA_DRIVER_PATH"), help="cua-driver executable")
    ap.add_argument("--cua-socket", default=os.environ.get("ALLTERNIT_CUA_DRIVER_SOCKET"), help="Cua daemon socket")
    ap.add_argument("--cua-embedded", action="store_true", default=os.environ.get("ALLTERNIT_CUA_DRIVER_EMBEDDED") == "true")
    ap.add_argument("--engine", choices=["auto", "atspi", "uia", "none"], default=os.environ.get("ALLTERNIT_DRIVER_ENGINE", "auto"),
                    help="native engine for non-mac hosts: auto picks atspi on Linux, uia on Windows (guest images)")
    ap.add_argument("--state-dir", help="router table and audit log directory")
    args = ap.parse_args()

    logging.basicConfig(level=logging.INFO, stream=sys.stderr, format="%(name)s %(levelname)s %(message)s")
    token = os.environ.pop("ALLTERNIT_DRIVER_TOKEN", None)
    if args.state_dir:
        os.makedirs(args.state_dir, mode=0o700, exist_ok=True)

    native = None
    engine = args.engine
    if engine == "auto":
        engine = {"linux": "atspi", "win32": "uia"}.get(sys.platform, "none")
    if engine == "atspi":
        from .engines.atspi import ATSPIEngine

        native = ATSPIEngine()
    elif engine == "uia":
        from .engines.uia import UIAEngine

        native = UIAEngine()

    cua_env = {k: v for k, v in os.environ.items() if not k.startswith("ALLTERNIT_DRIVER_")}
    cua_env.update(CUA_DRIVER_RS_TELEMETRY_ENABLED="false", CUA_TELEMETRY_ENABLED="false", NO_COLOR="1")
    driver = Driver(CuaEngine(args.cua, args.cua_socket, args.cua_embedded, cua_env), native=native, state_dir=args.state_dir)
    driver.start()

    def stop(*_: object) -> None:
        try:
            driver.close()
        finally:
            os._exit(0)

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    if sys.platform != "darwin":
        serve(driver, args.listen, args.endpoint_file, token)
        return
    # macOS: NSWorkspace (running apps, the frontmost app) only updates while
    # the main thread runs its run loop; without it, an app launched after the
    # sidecar started is "not running" forever. Serve on a thread, pump here.
    import threading

    import Foundation  # type: ignore

    def serve_then_exit() -> None:
        try:
            serve(driver, args.listen, args.endpoint_file, token)
        finally:
            os._exit(1)

    threading.Thread(target=serve_then_exit, name="server", daemon=True).start()
    loop = Foundation.NSRunLoop.mainRunLoop()
    while True:
        began = time.monotonic()
        loop.runUntilDate_(Foundation.NSDate.dateWithTimeIntervalSinceNow_(1.0))
        if time.monotonic() - began < 0.01:  # No sources yet: don't spin.
            time.sleep(0.25)


if __name__ == "__main__":
    main()
