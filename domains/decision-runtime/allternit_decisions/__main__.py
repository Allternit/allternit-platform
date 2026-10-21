"""python -m allternit_decisions --state-dir <dir> [--port 0] [--endpoint-file <file>] [--prepare]

Started by Allternit Desktop (decision-runtime-manager.ts) on loopback; the
launch token comes in ALLTERNIT_DECISIONS_TOKEN. Exits on SIGTERM. In the
cloud, run it next to allternit-api and point ALLTERNIT_DECISIONS_LOCAL_URL at it.
"""

from __future__ import annotations

import argparse
import logging
import os
import signal
import sys
from pathlib import Path

from .runtime import Runtime
from .server import serve


def main() -> None:
    ap = argparse.ArgumentParser(prog="allternit_decisions")
    ap.add_argument("--state-dir", required=True, help="packages, model weights")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--endpoint-file", help="write http://host:port here once listening")
    ap.add_argument("--engine", choices=["mlx", "llamacpp"], help="default: mlx on Apple silicon, else llamacpp")
    ap.add_argument("--model", help="preset (qwen3.5-4b, gemma-3-4b), HF repo id, or local path")
    ap.add_argument("--prepare", action="store_true", help="install, download and load now instead of on first use")
    args = ap.parse_args()

    logging.basicConfig(level=logging.INFO, stream=sys.stderr, format="%(name)s %(levelname)s %(message)s")
    state = Path(args.state_dir)
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    rt = Runtime(state, args.engine, args.model)
    if args.prepare:
        rt.ensure_started()
    signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
    serve(rt, args.host, args.port, os.environ.pop("ALLTERNIT_DECISIONS_TOKEN", None), args.endpoint_file)


if __name__ == "__main__":
    main()
