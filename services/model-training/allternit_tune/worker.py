# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Worker process: runs one TrainPlan and reports events on stdout.

Started by the engine server as `python -m allternit_tune.worker <plan.json>`.
Running training in its own process means cancel can stop it at once and a
crash in torch cannot take the server down.
"""

from __future__ import annotations

import json
import os
import sys
import traceback

from .events import Emitter


def main(argv: list) -> int:
    if len(argv) != 1:
        print("usage: python -m allternit_tune.worker <plan.json>", file=sys.stderr)
        return 2
    # Keep library noise off stdout-driven progress and never phone home.
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    # An op MPS lacks runs on CPU instead of failing the job.
    os.environ.setdefault("PYTORCH_ENABLE_MPS_FALLBACK", "1")
    emit = Emitter()
    try:
        from .backends import select_backend
        from .hardware import probe
        from .spec import TrainPlan

        with open(argv[0], encoding="utf-8") as fh:
            plan = TrainPlan.from_dict(json.load(fh))
        for note in plan.notes:
            emit.log(note)
        device = probe()["device"]
        backend = select_backend(device)
        emit.log(f"backend {backend.id}")
        backend.run(plan, emit)
        return 0
    except Exception as exc:  # noqa: BLE001 - every failure becomes an error event
        traceback.print_exc(file=sys.stderr)
        emit.error(f"{type(exc).__name__}: {exc}")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
