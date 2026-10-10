"""Option scoring on llama.cpp (Linux, Windows, Intel Macs, cloud guests).

Same interface and prompt as `engine_mlx.MlxScorer`. The prompt is prefilled
once; full llama.cpp states (KV and recurrent) are kept at each cut point in a
small LRU, so a decision that shares a head with an earlier one only prefills
the rest. With one-token labels the decision is read from the next-token
logits of that single prefill. Lists longer than the label alphabet fall back
to scoring each numeric label from a restored state (one short eval each).
"""

from __future__ import annotations

import math
import os
import threading
import time
from collections import OrderedDict
from typing import Any

import numpy as np
import llama_cpp
from llama_cpp import Llama

from .prompt import LETTERS, chat_messages, common_prefix_len, labels_for, render

SNAPSHOTS = 6


def _log_softmax(x: np.ndarray) -> np.ndarray:
    m = float(x.max())
    return x - (m + math.log(float(np.exp(x - m).sum())))


class LlamaCppScorer:
    engine = "llamacpp"
    vision = False

    def __init__(self, model_path: str, n_ctx: int = 8192) -> None:
        threads = max(1, (os.cpu_count() or 2) - 1)
        self.llm = Llama(
            model_path=model_path,
            n_ctx=n_ctx,
            n_batch=512,
            n_threads=threads,
            n_gpu_layers=-1,  # all layers on the GPU when the build has one
            logits_all=False,
            verbose=False,
        )
        self.n_vocab = self.llm.n_vocab()
        self.lock = threading.Lock()
        self.end_id = self.llm.token_eos()
        letter_ids = [self._tok(x) for x in LETTERS]
        self.letters_ok = all(len(i) == 1 for i in letter_ids) and len({i[0] for i in letter_ids}) == len(LETTERS)
        self._letter = {x: i[0] for x, i in zip(LETTERS, letter_ids)} if self.letters_ok else {}
        self._snaps: OrderedDict[tuple[int, ...], Any] = OrderedDict()
        template = self.llm.metadata.get("tokenizer.chat_template")
        if not template:
            raise RuntimeError("the GGUF has no chat template")
        from jinja2.sandbox import ImmutableSandboxedEnvironment

        self._template = ImmutableSandboxedEnvironment(trim_blocks=True, lstrip_blocks=True).from_string(template)

    def _tok(self, text: str) -> list[int]:
        return self.llm.tokenize(text.encode(), add_bos=False, special=True)

    def _chat_ids(self, user: str) -> list[int]:
        text = self._template.render(
            messages=chat_messages(user), add_generation_prompt=True, enable_thinking=False,
            bos_token="", eos_token="",
        )
        return self._tok(text)

    def _last_logits(self) -> np.ndarray:
        ptr = llama_cpp.llama_get_logits_ith(self.llm._ctx.ctx, -1)
        return np.ctypeslib.as_array(ptr, shape=(self.n_vocab,)).astype(np.float32).copy()

    def _eval_to(self, full: list[int], bounds: list[int]) -> int:
        """Leave the context holding all of `full`; return how many tokens were reused."""
        start = 0
        for key in sorted(self._snaps, key=len, reverse=True):
            if len(key) < len(full) and tuple(full[: len(key)]) == key:
                self.llm.load_state(self._snaps[key])
                self._snaps.move_to_end(key)
                start = len(key)
                break
        if start == 0:
            self.llm.reset()
            ctx = self.llm._ctx
            if hasattr(ctx, "memory_clear"):  # newer bindings: memory_clear(data)
                ctx.memory_clear(True)
            else:
                ctx.kv_cache_clear()
        reused = start
        for b in [b for b in bounds if start < b < len(full)] + [len(full)]:
            self.llm.eval(full[start:b])
            start = b
            if b < len(full):
                self._snaps[tuple(full[:b])] = self.llm.save_state()
                while len(self._snaps) > SNAPSHOTS:
                    self._snaps.popitem(last=False)
        return reused

    def score(self, context: str, options: list[str], kind: str = "choice", question: str | None = None) -> dict[str, Any]:
        with self.lock:
            t0 = time.perf_counter()
            labels = labels_for(len(options), self.letters_ok)
            r = render(context, options, labels, kind, question)
            full = self._chat_ids(r.text)
            bounds = sorted({common_prefix_len(self._chat_ids(r.text[:c]), full) for c in r.cuts})
            reused = self._eval_to(full, bounds)
            lp = _log_softmax(self._last_logits())
            t1 = time.perf_counter()
            option_tokens = 0
            if labels[0] in self._letter:
                sums = [float(lp[self._letter[x]]) for x in labels]
            else:
                state = self.llm.save_state()
                sums = []
                for label in labels:
                    ids = self._tok(label) + [self.end_id]
                    option_tokens += len(ids)
                    self.llm.load_state(state)
                    total, step = float(lp[ids[0]]), lp
                    for prev, nxt in zip(ids, ids[1:]):
                        self.llm.eval([prev])
                        step = _log_softmax(self._last_logits())
                        total += float(step[nxt])
                    sums.append(total)
            t2 = time.perf_counter()

        m = max(sums)
        exps = [math.exp(s - m) for s in sums]
        z = sum(exps)
        return {
            "probs": [e / z for e in exps],
            "timing": {
                "prefill_ms": round((t1 - t0) * 1000, 2),
                "options_ms": round((t2 - t1) * 1000, 2),
                "total_ms": round((t2 - t0) * 1000, 2),
                "prompt_tokens": len(full),
                "reused_tokens": reused,
                "option_tokens": option_tokens,
            },
        }
