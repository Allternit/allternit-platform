"""Grounder self-test on a static screenshot: no screen, no input.

    python -m allternit_driver.vision.selftest [--state-dir DIR] [--no-model]

``fixtures/canvas.png`` is a drawn app (a "Sketchpad" with tool buttons, a
canvas and Export/Cancel) whose controls exist only as pixels. The test runs
the production path: the vision worker (installed into the state dir on
first use), set-of-marks -> element ids, then the local grounder for each
case -> the snapped element id -> the desktop click point. The window is
pretended to sit at (100, 50) pt on a 2x display, so the click-point math is
checked too. Exit status 0 when every case lands inside its target.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from dataclasses import dataclass

from . import Shot, Vision
from . import geometry as g

FIXTURE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures", "canvas.png")
ORIGIN, SCALE = (100.0, 50.0), 2.0
# instruction -> the target's box in fixture px.
CASES = [
    ("the Export button", (1080, 730, 1240, 780)),
    ("the Eraser tool", (20, 140, 140, 190)),
    ("the red circle on the canvas", (500, 250, 620, 370)),
]


@dataclass
class _Target:
    key: str = "selftest:1"
    app: str = "selftest"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--state-dir")
    ap.add_argument("--no-model", action="store_true", help="marks only (no grounder)")
    args = ap.parse_args()
    with open(FIXTURE, "rb") as f:
        png = f.read()
    shot = Shot(png, ORIGIN, SCALE)
    vision = Vision(args.state_dir, lambda _t: Shot(png, ORIGIN, SCALE))
    t = _Target()
    report: dict = {"fixture": os.path.basename(FIXTURE), "size": list(shot.size), "cases": []}
    ok = True
    try:
        t0 = time.perf_counter()
        vision.worker.ensure(wait_s=1800)  # First use installs packages; a read only waits 20 s.
        report["prepare_s"] = round(time.perf_counter() - t0, 1)
        t0 = time.perf_counter()
        version, _ = vision.read(t)
        els = list(version.elements.values())
        report["marks"] = {"count": len(els), "ms": round((time.perf_counter() - t0) * 1000),
                           "sample": [e.public() for e in els[:8]]}
        names = {e.name for e in els}
        report["marks"]["has_export_control"] = any("Export" in n for n in names)
        ok &= len(els) > 0 and report["marks"]["has_export_control"]
        if not args.no_model:
            for instruction, target in CASES:
                res = vision.ground(t, instruction, els)
                row = {"instruction": instruction, **res}
                cur = vision.maps.window(t.key).current
                e = cur.elements.get(res.get("id", "")) if cur else None
                if e is not None:
                    box = shot.to_px(e.bounds)
                    row["mark"] = e.mark
                    row["element_box_px"] = [round(v) for v in box]
                    row["window_click_px"] = [round(v, 1) for v in g.center(box)]
                    # The click lands where act would click: the element's centre.
                    row["inside"] = g.contains(target, g.center(box))
                else:
                    row["inside"] = False
                ok &= row["inside"]
                report["cases"].append(row)
    finally:
        vision.close()
    report["ok"] = bool(ok)
    print(json.dumps(report, indent=1, default=str))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
