"""The local grounder: an instruction ("the Export button") -> a point.

Models (both Apache-2.0, checked on their Hugging Face cards 2026-10-09):

* ``ui-venus-1.5-8b`` (primary): inclusionAI/UI-Venus-1.5-8B, a Qwen3-VL
  fine-tune; on Mac the MLX 4-bit build mlx-community/UI-Venus-1.5-8B-4bit.
* ``holo2-8b`` (fallback): Hcompany/Holo2-8B (Qwen3-VL-8B-Thinking based). No
  MLX 4-bit build is published, so the worker converts it on first use
  (``mlx_vlm.convert``, 4-bit) into its state directory.

Holo2-30B-A3B is excluded (research-only licence).

Both answer in a 0-1000 frame. Each model's prompt and answer format follow
its authors' published grounding code (UI-Venus 1.5 branch
``models/grounding/ui_venus1_5_gd.py``; Holo2 cookbook localisation notebook).

Backends: ``mlx`` (Apple silicon, in the vision worker) and ``openai`` (any
OpenAI-compatible server such as vLLM serving the same repos; set
``ALLTERNIT_GROUNDER_URL``). The cloud vLLM deployment is configuration only.

``ground`` is the policy: pass 1 on the whole window (downscaled to
``max_pixels`` for speed), then a zoom pass on a crop around the answer when
it is low confidence: the window was downscaled, the point hit no known
element, or the caller asked. The crop is cut from the same screenshot at full
resolution, so the second pass sees the target at up to 2x the detail.
"""

from __future__ import annotations

import json
import re
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Sequence

from . import geometry as g

PRESETS: dict[str, dict[str, Any]] = {
    "ui-venus-1.5-8b": {
        "family": "venus15",
        "hf": "inclusionAI/UI-Venus-1.5-8B",
        "mlx": "mlx-community/UI-Venus-1.5-8B-4bit",
        "license": "apache-2.0",
    },
    "holo2-8b": {
        "family": "holo2",
        "hf": "Hcompany/Holo2-8B",
        "mlx": None,  # Converted to 4-bit on first use.
        "license": "apache-2.0",
    },
}
DEFAULT_CHAIN = ("ui-venus-1.5-8b", "holo2-8b")
MAX_PIXELS = 1_400_000  # Pass-1 budget: ~1.4k visual tokens; the zoom pass recovers detail.

_VENUS = (
    "Output the center point of the position corresponding to the following instruction: \n{}. \n\n"
    "The output should just be the coordinates of a point, in the format [x,y]. Additionally, if the task is "
    "infeasible (e.g., the task is not related to the image), the output should be [-1,-1]."
)
_HOLO_SCHEMA = json.dumps({
    "properties": {
        "x": {"description": "The x coordinate, normalized between 0 and 1000.", "maximum": 1000, "minimum": 0, "title": "X", "type": "integer"},
        "y": {"description": "The y coordinate, normalized between 0 and 1000.", "maximum": 1000, "minimum": 0, "title": "Y", "type": "integer"},
    },
    "required": ["x", "y"], "title": "ClickCoordinates", "type": "object",
})
_HOLO = (
    "Localize an element on the GUI image according to the provided target and output a click position.\n"
    f"     * You must output a valid JSON following the format: {_HOLO_SCHEMA}\n"
    "     Your target is:\n{}"
)


def prompt(family: str, instruction: str) -> str:
    instruction = instruction.strip().rstrip(".")
    if family == "holo2":
        return _HOLO.replace("{}", instruction)
    return _VENUS.format(instruction)


_NUM = r"-?\d+(?:\.\d+)?"


def parse(family: str, text: str) -> tuple[str, tuple[float, float] | None]:
    """(status, 0-1000 point). status: positive | infeasible | wrong_format."""
    text = re.sub(r"<think>.*?</think>", "", text or "", flags=re.S).strip()
    if family == "holo2":
        for m in reversed(list(re.finditer(r"\{[^{}]*\}", text))):
            try:
                obj = json.loads(m.group(0))
                return "positive", (float(obj["x"]), float(obj["y"]))
            except (ValueError, KeyError, TypeError):
                continue
    m = re.search(rf"\[\s*({_NUM})\s*,\s*({_NUM})\s*,\s*({_NUM})\s*,\s*({_NUM})\s*\]", text)
    if m:
        x0, y0, x1, y1 = (float(v) for v in m.groups())
        return "positive", ((x0 + x1) / 2, (y0 + y1) / 2)
    m = re.search(rf"\[\s*({_NUM})\s*,\s*({_NUM})\s*\]", text) or re.search(rf"\(\s*({_NUM})\s*,\s*({_NUM})\s*\)", text)
    if not m:
        return "wrong_format", None
    x, y = float(m.group(1)), float(m.group(2))
    if x < 0 or y < 0:
        return "infeasible", None
    return "positive", (min(x, g.NORM), min(y, g.NORM))


# ---- the policy --------------------------------------------------------------------


@dataclass
class Pass:
    model: str
    box: g.Box  # Image px the model saw (the whole image or the crop).
    scale: float  # Downscale applied before the model.
    status: str
    point: g.Point | None  # Image px.
    raw: str
    ms: float


@dataclass
class Grounding:
    status: str  # positive | infeasible | wrong_format | unavailable
    point: g.Point | None = None  # Image px.
    confidence: str = "none"  # high | medium | low | none
    zoomed: bool = False
    model: str | None = None
    snapped: int | None = None  # Index into the caller's boxes.
    passes: list[Pass] = field(default_factory=list)
    error: str | None = None

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {"status": self.status, "confidence": self.confidence, "zoomed": self.zoomed, "model": self.model}
        if self.point is not None:
            out["point"] = [round(self.point[0], 1), round(self.point[1], 1)]
        if self.snapped is not None:
            out["snapped"] = self.snapped
        if self.error:
            out["error"] = self.error
        out["passes"] = [
            {"model": p.model, "box": [round(v) for v in p.box], "scale": round(p.scale, 3), "status": p.status,
             "point": None if p.point is None else [round(p.point[0], 1), round(p.point[1], 1)], "raw": p.raw[:120], "ms": round(p.ms)}
            for p in self.passes
        ]
        return out


# A model call: (model name, PIL image, instruction) -> raw text.
Runner = Callable[[str, Any, str], str]


def _one_pass(run: Runner, model: str, img: Any, box: g.Box, instruction: str, max_pixels: int) -> Pass:
    family = PRESETS.get(model, {}).get("family", "venus15")
    crop = img if box == (0, 0, img.size[0], img.size[1]) else img.crop(tuple(int(round(v)) for v in box))
    w, h = crop.size
    s = g.fit_scale(w, h, max_pixels)
    if s < 1.0:
        crop = crop.resize((max(1, int(w * s)), max(1, int(h * s))))
    t = time.perf_counter()
    raw = run(model, crop, prompt(family, instruction))
    status, norm = parse(family, raw)
    point = g.norm_to_px(norm[0], norm[1], box) if norm is not None else None
    return Pass(model, box, s, status, point, raw, (time.perf_counter() - t) * 1000)


def ground(
    run: Runner,
    img: Any,
    instruction: str,
    boxes: Sequence[g.Box] = (),
    chain: Sequence[str] = DEFAULT_CHAIN,
    zoom: str = "auto",
    max_pixels: int = MAX_PIXELS,
    available: Callable[[str], bool] = lambda _m: True,
) -> Grounding:
    """Ground ``instruction`` in ``img``. ``boxes`` are known element boxes
    (image px) to snap to; ``chain`` is tried in order until a model answers
    a point; ``zoom`` is auto | always | never."""
    w, h = img.size
    full: g.Box = (0, 0, w, h)
    out = Grounding("unavailable")
    first: Pass | None = None
    for model in chain:
        if not available(model):
            continue
        try:
            p = _one_pass(run, model, img, full, instruction, max_pixels)
        except Exception as e:  # A model that fails to load or run hands over to the next.
            out.error = f"{model}: {type(e).__name__}: {e}"[:300]
            continue
        out.passes.append(p)
        out.status, out.model = p.status, model
        if p.status == "positive":
            first = p
            break
    if first is None or first.point is None:
        return out
    out.point, out.snapped = first.point, g.snap(first.point, boxes)
    low = first.scale < 1.0 or (bool(boxes) and out.snapped is None)
    if zoom == "never" or (zoom == "auto" and not low):
        out.confidence = "high" if out.snapped is not None or not boxes else "medium"
        return out
    crop = g.zoom_box(w, h, first.point)
    if crop == full and first.scale >= 1.0:
        out.confidence = "medium"
        return out
    try:
        second = _one_pass(run, first.model, img, crop, instruction, max_pixels)
    except Exception as e:
        out.error = f"zoom: {type(e).__name__}: {e}"[:300]
        out.confidence = "low"
        return out
    out.passes.append(second)
    out.zoomed = True
    if second.status != "positive" or second.point is None:
        out.confidence = "low"  # The crop lost the target: keep pass 1.
        return out
    out.point, out.snapped = second.point, g.snap(second.point, boxes)
    out.confidence = "high" if g.agree(first.point, second.point, w, h) or out.snapped is not None else "medium"
    return out


# ---- backends ----------------------------------------------------------------------


class OpenAIBackend:
    """An OpenAI-compatible chat endpoint (vLLM serving UI-Venus / Holo2)."""

    def __init__(self, url: str, model: str | None = None, key: str | None = None, timeout: float = 60.0) -> None:
        self.url = url.rstrip("/")
        self.model = model
        self.key = key
        self.timeout = timeout

    def __call__(self, model: str, img: Any, text: str) -> str:
        import base64
        import io
        import urllib.request

        buf = io.BytesIO()
        img.save(buf, format="PNG")
        body = {
            "model": self.model or PRESETS.get(model, {}).get("hf", model),
            "temperature": 0,
            "max_tokens": 64,
            "messages": [{"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()}},
                {"type": "text", "text": text},
            ]}],
            "chat_template_kwargs": {"enable_thinking": False},
        }
        req = urllib.request.Request(self.url + "/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"})
        if self.key:
            req.add_header("Authorization", f"Bearer {self.key}")
        with urllib.request.urlopen(req, timeout=self.timeout) as resp:
            data = json.loads(resp.read())
        return str(data["choices"][0]["message"].get("content") or "")


class MlxBackend:
    """mlx-vlm on Apple silicon. Keeps one model in memory (the fallback
    loads only when the primary can't answer, and replaces it)."""

    def __init__(self, models_dir: str | None = None) -> None:
        self.models_dir = models_dir
        self.loaded: tuple[str, Any, Any] | None = None
        self.load_s: float | None = None

    def path(self, model: str) -> str:
        preset = PRESETS.get(model)
        if preset is None:
            return model  # A local path or another MLX repo id.
        if preset["mlx"]:
            return preset["mlx"]  # Downloaded to the Hugging Face cache on first load.
        return self._converted(preset["hf"])

    def _converted(self, repo: str) -> str:
        import os

        from mlx_vlm import convert

        base = self.models_dir or os.path.join(os.path.expanduser("~"), ".cache", "allternit-driver", "models")
        dest = os.path.join(base, repo.replace("/", "--") + "-mlx-4bit")
        if not os.path.exists(os.path.join(dest, ".complete")):
            os.makedirs(base, exist_ok=True)
            convert(repo, mlx_path=dest, quantize=True, q_bits=4)
            open(os.path.join(dest, ".complete"), "w").close()
        return dest

    def _load(self, model: str) -> tuple[Any, Any]:
        if self.loaded is not None and self.loaded[0] == model:
            return self.loaded[1], self.loaded[2]
        from mlx_vlm import load

        self.loaded = None
        t = time.perf_counter()
        m, proc = load(self.path(model))
        self.load_s = round(time.perf_counter() - t, 2)
        self.loaded = (model, m, proc)
        return m, proc

    def __call__(self, model: str, img: Any, text: str) -> str:
        from mlx_vlm import generate
        from mlx_vlm.prompt_utils import apply_chat_template

        m, proc = self._load(model)
        kwargs = {"enable_thinking": False} if PRESETS.get(model, {}).get("family") == "holo2" else {}
        p = apply_chat_template(proc, m.config, text, num_images=1, **kwargs)
        r = generate(m, proc, p, image=[img], max_tokens=48, temperature=0.0, verbose=False)
        return str(getattr(r, "text", r))

    def unload(self) -> None:
        self.loaded = None
        try:
            import mlx.core as mx

            mx.clear_cache()
        except Exception:
            pass
