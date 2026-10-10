"""One-pass option scoring on MLX (Apple silicon).

Ported from open-jev's `OptionScorer` (MIT, daseinlabs; see ../NOTICE) and
extended for hybrid models such as Qwen3.5, whose linear-attention layers keep
recurrent state (`ArraysCache`) next to the usual `KVCache`:

1. The prompt is prefilled once. The state at each cut point (see prompt.py)
   is kept in a small LRU, so the next decision that shares a head only
   prefills what changed. Recurrent state
   cannot be rewound, which is why boundaries are snapshotted, not trimmed.
2. With one-token labels the decision is read straight from the next-token
   distribution of that prefill: no option pass at all.
3. Otherwise the state is expanded across the batch dimension and every label
   (plus the end-of-turn token) is scored in one right-padded forward pass.
   Right padding is safe for both cache kinds: each position only sees what
   came before it.

Scores are summed label log-probabilities; probabilities are a softmax over the
option set (relative to the set, not calibrated). Nothing is generated.
"""

from __future__ import annotations

import math
import threading
import time
from collections import OrderedDict
from typing import Any

import mlx.core as mx
from mlx_lm import load
from mlx_lm.models.cache import ArraysCache, KVCache, make_prompt_cache

from .prompt import LETTERS, chat_messages, common_prefix_len, labels_for, render

SNAPSHOTS = 12


def _copy(cache: list[Any]) -> list[Any]:
    """A cache list whose next update cannot write into `cache`."""
    out: list[Any] = []
    for c in cache:
        if isinstance(c, KVCache):
            e = KVCache()
            e.offset = c.offset
            if c.keys is not None:
                # Exact-size slices: the next update must grow the buffer, so it
                # allocates instead of writing into ours.
                e.keys = c.keys[..., : c.offset, :]
                e.values = c.values[..., : c.offset, :]
            out.append(e)
        elif isinstance(c, ArraysCache):
            e = ArraysCache(len(c.cache))
            e.cache = list(c.cache)  # layers assign new arrays, never write in place
            out.append(e)
        else:
            raise TypeError(f"unsupported cache type {type(c).__name__}")
    return out


def _expand(cache: list[Any], n: int) -> list[Any]:
    out: list[Any] = []
    for c in cache:
        if isinstance(c, KVCache):
            e = KVCache()
            e.offset = c.offset
            e.keys = mx.repeat(c.keys[..., : c.offset, :], n, axis=0)
            e.values = mx.repeat(c.values[..., : c.offset, :], n, axis=0)
        else:
            e = ArraysCache(len(c.cache))
            e.cache = [None if a is None else mx.repeat(a, n, axis=0) for a in c.cache]
        out.append(e)
    return out


def _arrays(cache: list[Any]) -> list[mx.array]:
    out: list[mx.array] = []
    for c in cache:
        if isinstance(c, KVCache):
            if c.keys is not None:
                out += [c.keys, c.values]
        else:
            out += [a for a in c.cache if a is not None]
    return out


def softmax(xs: list[float]) -> list[float]:
    m = max(xs)
    exps = [math.exp(x - m) for x in xs]
    z = sum(exps)
    return [e / z for e in exps]


class MlxScorer:
    engine = "mlx"
    vision = False

    def __init__(self, model_path: str, max_batch: int = 64) -> None:
        self.model, self.tok = load(model_path)
        self.max_batch = max(1, max_batch)
        self.lock = threading.Lock()
        eos = self.tok.eos_token_id
        self.end_id = eos if isinstance(eos, int) else None
        self.pad_id = self.tok.pad_token_id if self.tok.pad_token_id is not None else (self.end_id or 0)
        letter_ids = [self.tok.encode(x, add_special_tokens=False) for x in LETTERS]
        self.letters_ok = all(len(i) == 1 for i in letter_ids) and len({i[0] for i in letter_ids}) == len(LETTERS)
        self._letter = {x: i[0] for x, i in zip(LETTERS, letter_ids)} if self.letters_ok else {}
        # token-id prefix -> (cache, last logits); most recent last.
        self._snaps: OrderedDict[tuple[int, ...], tuple[list[Any], mx.array]] = OrderedDict()

    def _chat_ids(self, user: str) -> list[int]:
        text = self.tok.apply_chat_template(
            chat_messages(user), tokenize=False, add_generation_prompt=True, enable_thinking=False
        )
        return self.tok.encode(text, add_special_tokens=False)

    def _prefill(self, ids: list[int], cache: list[Any]) -> mx.array:
        logits = self.model(mx.array(ids)[None], cache=cache)
        last = logits[0, -1].astype(mx.float32)
        mx.eval(last, *_arrays(cache))
        return last

    def _state(self, full: list[int], bounds: list[int]) -> tuple[list[Any], mx.array, int]:
        """Prefilled state for all of `full`, reusing and refreshing boundary snapshots."""
        start, cache, last = 0, None, None
        for key in sorted(self._snaps, key=len, reverse=True):
            if len(key) < len(full) and tuple(full[: len(key)]) == key:
                start = len(key)
                cache, last = self._snaps[key]
                self._snaps.move_to_end(key)
                break
        reused = start
        cache = make_prompt_cache(self.model) if cache is None else _copy(cache)
        for b in [b for b in bounds if start < b < len(full)] + [len(full)]:
            last = self._prefill(full[start:b], cache)
            start = b
            if b < len(full):
                self._snaps[tuple(full[:b])] = (cache, last)
                while len(self._snaps) > SNAPSHOTS:
                    self._snaps.popitem(last=False)
                cache = _copy(cache)
        return cache, last, reused

    def score(self, context: str, options: list[str], kind: str = "choice", question: str | None = None) -> dict[str, Any]:
        with self.lock:
            t0 = time.perf_counter()
            labels = labels_for(len(options), self.letters_ok)
            r = render(context, options, labels, kind, question)
            full = self._chat_ids(r.text)
            bounds = sorted({common_prefix_len(self._chat_ids(r.text[:c]), full) for c in r.cuts})
            cache, last, reused = self._state(full, bounds)
            t1 = time.perf_counter()

            if labels[0] in self._letter:
                lp = last - mx.logsumexp(last)
                sums = mx.take(lp, mx.array([self._letter[x] for x in labels])).tolist()
                option_tokens = 0
            else:
                sums, option_tokens = self._score_labels(cache, last, labels)
            t2 = time.perf_counter()

        return {
            "probs": softmax(sums),
            "timing": {
                "prefill_ms": round((t1 - t0) * 1000, 2),
                "options_ms": round((t2 - t1) * 1000, 2),
                "total_ms": round((t2 - t0) * 1000, 2),
                "prompt_tokens": len(full),
                "reused_tokens": reused,
                "option_tokens": option_tokens,
            },
        }

    def _score_labels(self, cache: list[Any], last: mx.array, labels: list[str]) -> tuple[list[float], int]:
        end = [self.end_id] if self.end_id is not None else self.tok.encode("\n", add_special_tokens=False)
        opts = [self.tok.encode(x, add_special_tokens=False) + end for x in labels]
        sums: list[float] = []
        for start in range(0, len(opts), self.max_batch):
            chunk = opts[start : start + self.max_batch]
            n, width = len(chunk), max(len(x) for x in chunk)
            arr = mx.array([x + [self.pad_id] * (width - len(x)) for x in chunk])
            mask = mx.array([[1.0] * len(x) + [0.0] * (width - len(x)) for x in chunk])
            logits = self.model(arr, cache=_expand(cache, n))
            first = mx.broadcast_to(last[None, None, :], (n, 1, logits.shape[-1]))
            pred = mx.concatenate([first, logits[:, :-1].astype(mx.float32)], axis=1)
            tgt = mx.take_along_axis(pred, arr[..., None], axis=-1)[..., 0]
            s = ((tgt - mx.logsumexp(pred, axis=-1)) * mask).sum(-1)
            mx.eval(s)
            sums.extend(s.tolist())
        return sums, sum(len(o) for o in opts)
