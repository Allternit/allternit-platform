"""Pure geometry for the vision fallback: no image libraries, so it is unit
tested anywhere and shared by the driver and its vision worker.

Coordinates:
* image px: pixels of the window screenshot, top-left origin.
* norm: the grounders' 0-1000 frame (UI-Venus and Holo2 both answer in it).
* screen points: the element map's frame (what accessibility bounds use).
"""

from __future__ import annotations

import math
from typing import Iterable, Sequence

Box = tuple[float, float, float, float]  # x0, y0, x1, y1 in image px
Point = tuple[float, float]

NORM = 1000.0
ZOOM_FACTOR = 2.0  # The crop is 1/ZOOM_FACTOR of each image side...
ZOOM_MIN_SIDE = 448.0  # ...but never smaller than this many px a side.
AGREE_PX = 12.0  # Two passes agree within max(AGREE_PX, AGREE_FRAC * diagonal).
AGREE_FRAC = 0.015


def fit_scale(w: int, h: int, max_pixels: int) -> float:
    """The downscale (<= 1) that brings a w x h image under max_pixels."""
    if w <= 0 or h <= 0 or max_pixels <= 0 or w * h <= max_pixels:
        return 1.0
    return math.sqrt(max_pixels / float(w * h))


def norm_to_px(nx: float, ny: float, box: Box) -> Point:
    """A 0-1000 point in the image (or crop) ``box`` -> image px."""
    x0, y0, x1, y1 = box
    return (x0 + nx / NORM * (x1 - x0), y0 + ny / NORM * (y1 - y0))


def zoom_box(w: float, h: float, p: Point, factor: float = ZOOM_FACTOR, min_side: float = ZOOM_MIN_SIDE) -> Box:
    """The crop to reground in: centred on ``p``, 1/factor of each side (at
    least ``min_side``, at most the image), shifted to stay inside the image."""
    cw = min(w, max(min_side, w / factor))
    ch = min(h, max(min_side, h / factor))
    x0 = min(max(p[0] - cw / 2, 0.0), w - cw)
    y0 = min(max(p[1] - ch / 2, 0.0), h - ch)
    return (x0, y0, x0 + cw, y0 + ch)


def agree(a: Point, b: Point, w: float, h: float) -> bool:
    """Whether two grounding passes point at the same spot."""
    return math.dist(a, b) <= max(AGREE_PX, AGREE_FRAC * math.hypot(w, h))


def contains(box: Box, p: Point) -> bool:
    return box[0] <= p[0] <= box[2] and box[1] <= p[1] <= box[3]


def area(box: Box) -> float:
    return max(0.0, box[2] - box[0]) * max(0.0, box[3] - box[1])


def iou(a: Box, b: Box) -> float:
    inter = area((max(a[0], b[0]), max(a[1], b[1]), min(a[2], b[2]), min(a[3], b[3])))
    union = area(a) + area(b) - inter
    return inter / union if union > 0 else 0.0


def snap(p: Point, boxes: Sequence[Box]) -> int | None:
    """Index of the smallest box containing ``p`` (the most specific target)."""
    best, best_area = None, math.inf
    for i, b in enumerate(boxes):
        if contains(b, p) and area(b) < best_area:
            best, best_area = i, area(b)
    return best


def center(box: Box) -> Point:
    return ((box[0] + box[2]) / 2, (box[1] + box[3]) / 2)


def reading_order(boxes: Iterable[Box], row: float = 12.0) -> list[int]:
    """Indices in reading order: rows of ``row`` px top to bottom, then left to right."""
    boxes = list(boxes)
    return sorted(range(len(boxes)), key=lambda i: (round(center(boxes[i])[1] / row), boxes[i][0]))


def nms(boxes: Sequence[Box], scores: Sequence[float], threshold: float = 0.6) -> list[int]:
    """Greedy non-maximum suppression: indices kept, best score first."""
    keep: list[int] = []
    for i in sorted(range(len(boxes)), key=lambda i: -scores[i]):
        if all(iou(boxes[i], boxes[j]) < threshold for j in keep):
            keep.append(i)
    return keep


def px_to_screen(box: Box, origin: Point, scale: float) -> tuple[float, float, float, float]:
    """Image px box -> screen points (x, y, w, h), the element map's bounds."""
    s = scale or 1.0
    return (origin[0] + box[0] / s, origin[1] + box[1] / s, (box[2] - box[0]) / s, (box[3] - box[1]) / s)


def hamming(a: str, b: str) -> int:
    """Bit distance of two equal-length hex hashes (mean-hash freshness check)."""
    if not a or not b or len(a) != len(b):
        return 1 << 30
    return bin(int(a, 16) ^ int(b, 16)).count("1")
