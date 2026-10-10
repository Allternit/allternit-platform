"""Set-of-marks: numbered regions of a window screenshot, licence-clean.

Runs in the vision worker (it needs Pillow + NumPy). Region sources:

* ``ocr``: text lines from the OS text recogniser (Apple Vision on macOS,
  through the bundled PyObjC). Nothing to download.
* ``boxes``: controls and icons from a model-free proposer: an edge-density
  grid over the screenshot, grouped into connected blobs. It finds drawn
  buttons, tool icons and swatches in canvas apps that have no tree.
Cua Driver 0.34's ``parse_visual_regions`` is not used: it needs Cua's
optional ``cua-perception`` extension, which is AGPL (driver spec section 5,
"avoid"). These two sources replace it and ship nothing extra.

``merge`` folds them into marks: a control box that holds one line of text
becomes one ``control`` mark named by that text; text outside any control is a
``text`` mark; a control without text is an ``icon`` mark. Duplicates are
suppressed (IoU), marks are numbered in reading order, capped at ``max_marks``.
"""

from __future__ import annotations

import io
import sys
from dataclasses import dataclass
from typing import Any

from . import geometry as g

CELL = 8  # Proposer grid cell, px.
EDGE = 24  # Gradient magnitude that counts as an edge (0-255 luminance).
DENSITY = 0.08  # Fraction of edge pixels that makes a cell active.
MIN_CELLS = 3
MAX_AREA_FRAC = 0.2  # A blob larger than this share of the image is a panel, not a control.
OCR_MIN_CONFIDENCE = 0.4
CONTROL_TEXT_RATIO = 8.0  # A box at most this many times its text's area is that text's control.


@dataclass
class Region:
    box: g.Box
    kind: str  # text | icon | control
    name: str = ""
    score: float = 0.5
    origin: str = "boxes"  # ocr | boxes


def load(png: bytes) -> Any:
    from PIL import Image

    return Image.open(io.BytesIO(png)).convert("RGB")


# ---- OCR ---------------------------------------------------------------------


def ocr(png: bytes, width: int, height: int) -> list[Region]:
    """Text lines with pixel boxes. macOS only (Apple Vision); [] elsewhere."""
    if sys.platform != "darwin":
        return []
    try:
        import Foundation  # type: ignore
        import Quartz  # type: ignore
        import Vision  # type: ignore
    except Exception:  # No PyObjC (not the bundled runtime): no OCR, boxes still work.
        return []
    provider = Quartz.CGDataProviderCreateWithCFData(Foundation.NSData.dataWithBytes_length_(png, len(png)))
    image = Quartz.CGImageCreateWithPNGDataProvider(provider, None, False, Quartz.kCGRenderingIntentDefault)
    if image is None:
        return []
    req = Vision.VNRecognizeTextRequest.alloc().init()
    req.setRecognitionLevel_(Vision.VNRequestTextRecognitionLevelAccurate)
    req.setUsesLanguageCorrection_(False)
    handler = Vision.VNImageRequestHandler.alloc().initWithCGImage_options_(image, {})
    ok, _err = handler.performRequests_error_([req], None)
    if not ok:
        return []
    out: list[Region] = []
    for obs in req.results() or []:
        cand = obs.topCandidates_(1)
        if not cand:
            continue
        text, conf = str(cand[0].string()), float(cand[0].confidence())
        if conf < OCR_MIN_CONFIDENCE or not text.strip():
            continue
        bb = obs.boundingBox()  # Normalised, bottom-left origin.
        x0 = bb.origin.x * width
        y0 = (1 - bb.origin.y - bb.size.height) * height
        out.append(Region((x0, y0, x0 + bb.size.width * width, y0 + bb.size.height * height), "text", text.strip(), conf, "ocr"))
    return out


# ---- model-free box proposer ---------------------------------------------------


def boxes(img: Any) -> list[Region]:
    """Control/icon candidates: cells dense in edges, grouped 8-connected."""
    import numpy as np

    gray = np.asarray(img.convert("L"), dtype=np.int16)
    h, w = gray.shape
    gx = np.zeros_like(gray)
    gy = np.zeros_like(gray)
    gx[:, 1:] = np.abs(gray[:, 1:] - gray[:, :-1])
    gy[1:, :] = np.abs(gray[1:, :] - gray[:-1, :])
    edges = (np.maximum(gx, gy) >= EDGE).astype(np.float32)
    rows, cols = h // CELL, w // CELL
    if rows == 0 or cols == 0:
        return []
    dens = edges[: rows * CELL, : cols * CELL].reshape(rows, CELL, cols, CELL).mean(axis=(1, 3))
    active = dens >= DENSITY
    seen = np.zeros_like(active)
    out: list[Region] = []
    max_area = MAX_AREA_FRAC * w * h
    for r in range(rows):
        for c in range(cols):
            if not active[r, c] or seen[r, c]:
                continue
            stack, cells = [(r, c)], []
            seen[r, c] = True
            while stack:
                y, x = stack.pop()
                cells.append((y, x))
                for dy in (-1, 0, 1):
                    for dx in (-1, 0, 1):
                        ny, nx = y + dy, x + dx
                        if 0 <= ny < rows and 0 <= nx < cols and active[ny, nx] and not seen[ny, nx]:
                            seen[ny, nx] = True
                            stack.append((ny, nx))
            if len(cells) < MIN_CELLS:
                continue
            ys = [y for y, _ in cells]
            xs = [x for _, x in cells]
            box = (min(xs) * CELL, min(ys) * CELL, (max(xs) + 1) * CELL, (max(ys) + 1) * CELL)
            if g.area(box) > max_area:
                continue
            fill = len(cells) / max(1, (max(ys) - min(ys) + 1) * (max(xs) - min(xs) + 1))
            out.append(Region(box, "icon", "", round(0.3 + 0.4 * fill, 3), "boxes"))
    return out


# ---- merge ---------------------------------------------------------------------


def merge(texts: list[Region], shapes: list[Region], max_marks: int = 120) -> list[Region]:
    """Fold text lines and shape boxes into one deduplicated, numbered list."""
    used_text: set[int] = set()
    marks: list[Region] = []
    for s in shapes:
        inside = [i for i, t in enumerate(texts) if g.contains(s.box, g.center(t.box))]
        if inside:
            text_area = sum(g.area(texts[i].box) for i in inside)
            if len(inside) <= 2 and g.area(s.box) <= CONTROL_TEXT_RATIO * max(text_area, 1.0):
                name = " ".join(texts[i].name for i in sorted(inside, key=lambda i: texts[i].box[0]))
                marks.append(Region(s.box, "control", name, max(s.score, *(texts[i].score for i in inside)), s.origin))
                used_text.update(inside)
            continue  # A box around more text is a panel or a text block: the lines speak for it.
        marks.append(s)
    marks.extend(t for i, t in enumerate(texts) if i not in used_text)
    keep = g.nms([m.box for m in marks], [m.score + (0.5 if m.name else 0.0) for m in marks])
    marks = [marks[i] for i in keep]
    marks = [marks[i] for i in g.reading_order(m.box for m in marks)]
    return marks[:max_marks]


def propose(png: bytes, sources: tuple[str, ...] = ("ocr", "boxes"), max_marks: int = 120) -> tuple[list[Region], tuple[int, int]]:
    img = load(png)
    w, h = img.size
    texts = ocr(png, w, h) if "ocr" in sources else []
    shapes = boxes(img) if "boxes" in sources else []
    return merge(texts, shapes, max_marks), (w, h)


def mean_hash(img: Any, box: g.Box | None = None, size: int = 16) -> str:
    """A 256-bit mean hash of a region: tolerant to anti-aliasing and the
    cursor blink, changes when the region's content changes."""
    import numpy as np

    crop = img.crop(tuple(int(round(v)) for v in box)) if box else img
    a = np.asarray(crop.convert("L").resize((size, size)), dtype=np.float32)
    bits = (a > a.mean()).flatten()
    return "%0*x" % (size * size // 4, int("".join("1" if b else "0" for b in bits), 2))
