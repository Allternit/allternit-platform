"""The vision worker: ``python -m allternit_driver.vision.worker --site <dir>``.

A child of the driver sidecar, so a model crash or its memory (about 6 GB for
the 8B grounder) never takes the sidecar down; the driver stops it after
``IDLE_S`` without vision work. JSON lines on stdin/stdout:

    {"id": 1, "op": "marks", "png": <b64>, "sources": [...], "max_marks": 120}
    {"id": 2, "op": "ground", "png": <b64>, "instruction": "...", "boxes": [[x0,y0,x1,y1], ...], "zoom": "auto"}
    {"id": 3, "op": "hash", "png": <b64>, "boxes": [[...], ...]}
    {"id": 4, "op": "health"}
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import platform
import sys
import time
import traceback
from typing import Any


def backend(models_dir: str | None) -> Any:
    from .grounder import MlxBackend, OpenAIBackend

    url = os.environ.get("ALLTERNIT_GROUNDER_URL")
    if url:
        return OpenAIBackend(url, os.environ.get("ALLTERNIT_GROUNDER_MODEL"), os.environ.get("ALLTERNIT_GROUNDER_KEY"))
    if sys.platform == "darwin" and platform.machine() == "arm64":
        return MlxBackend(models_dir)
    return None


def chain() -> tuple[str, ...]:
    from .grounder import DEFAULT_CHAIN

    env = os.environ.get("ALLTERNIT_GROUNDER_MODELS")
    return tuple(m.strip() for m in env.split(",") if m.strip()) if env else DEFAULT_CHAIN


def handle(req: dict[str, Any], state: dict[str, Any]) -> dict[str, Any]:
    from . import grounder as gr
    from . import marks as mk

    op = req.get("op")
    if op == "health":
        b = state["backend"]
        return {"backend": type(b).__name__ if b else None, "chain": list(chain()),
                "loaded": getattr(b, "loaded", None) and b.loaded[0], "load_s": getattr(b, "load_s", None)}
    png = base64.b64decode(req["png"])
    if op == "marks":
        regions, (w, h) = mk.propose(png, tuple(req.get("sources") or ("ocr", "boxes")), int(req.get("max_marks", 120)))
        img = mk.load(png)
        return {"size": [w, h], "marks": [
            {"box": [round(v, 1) for v in r.box], "kind": r.kind, "name": r.name, "score": round(r.score, 3),
             "origin": r.origin, "hash": mk.mean_hash(img, r.box)}
            for r in regions
        ]}
    if op == "hash":
        img = mk.load(png)
        return {"hashes": [mk.mean_hash(img, tuple(b)) for b in req.get("boxes", [])]}
    if op == "ground":
        b = state["backend"]
        if b is None:
            return {"status": "unavailable", "error": "no grounder here: needs Apple silicon (MLX) or ALLTERNIT_GROUNDER_URL"}
        img = mk.load(png)
        res = gr.ground(b, img, str(req["instruction"]), [tuple(x) for x in req.get("boxes", [])],
                        tuple(req.get("chain") or chain()), str(req.get("zoom", "auto")), int(req.get("max_pixels", gr.MAX_PIXELS)))
        return {"size": list(img.size), **res.to_json()}
    raise ValueError(f"unknown op {op}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--site", action="append", default=[])
    ap.add_argument("--models-dir")
    args = ap.parse_args()
    for site in reversed(args.site):
        sys.path.insert(0, site)
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    state = {"backend": backend(args.models_dir)}
    out = sys.stdout
    sys.stdout = sys.stderr  # Library prints must never corrupt the protocol.
    out.write(json.dumps({"ready": True, "pid": os.getpid()}) + "\n")
    out.flush()
    for line in sys.stdin:
        if not line.strip():
            continue
        t = time.perf_counter()
        req: Any = None
        try:
            req = json.loads(line)
            resp = {"id": req.get("id"), "result": handle(req, state)}
        except Exception as e:  # Every failure is an answer.
            traceback.print_exc()
            resp = {"id": req.get("id") if isinstance(req, dict) else None, "error": f"{type(e).__name__}: {e}"[:500]}
        resp["ms"] = round((time.perf_counter() - t) * 1000, 1)
        out.write(json.dumps(resp) + "\n")
        out.flush()


if __name__ == "__main__":
    main()
