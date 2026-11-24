# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Stand-in for the training worker in tests: walks the real event protocol
without torch. Behaviour comes from FAKE_WORKER_MODE: ok, slow, fail."""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from allternit_tune.events import Emitter  # noqa: E402


def main() -> int:
    plan = json.loads(Path(sys.argv[1]).read_text())
    mode = os.environ.get("FAKE_WORKER_MODE", "ok")
    emit = Emitter()
    print("plain library output on stdout", flush=True)
    emit.state("running", "training lora")
    steps = 4
    for step in range(1, steps + 1):
        emit.progress(step / steps, f"step {step}/{steps} loss {2.0 / step:.3f}")
        if mode == "slow":
            time.sleep(0.5)
    if mode == "fail":
        emit.error("RuntimeError: out of memory")
        return 1
    emit.state("exporting")
    adapter = Path(plan.get("output_dir") or plan["job_dir"]) / "adapter"
    adapter.mkdir()
    (adapter / "adapter_model.safetensors").write_bytes(b"x" * 10)
    emit.output("adapter", adapter)
    return 0


if __name__ == "__main__":
    sys.exit(main())
