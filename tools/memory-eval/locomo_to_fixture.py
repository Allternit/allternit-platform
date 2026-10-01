#!/usr/bin/env python3
"""Convert the public LoCoMo set (locomo10.json) to the allternit-api memory
eval fixture format (see cmd/allternit-api/src/memory_retrieve/eval.rs).

LoCoMo is NOT vendored in this repo: check its license upstream
(github.com/snap-research/locomo) before redistributing. Download it yourself:

    curl -L -o /tmp/locomo10.json \
      https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json
    python3 tools/memory-eval/locomo_to_fixture.py /tmp/locomo10.json /tmp/locomo-fixture.json

Turn ids are LoCoMo's dia_ids (D<session>:<turn>), so its `evidence` lists
are the gold ids as-is. Category 5 (adversarial, unanswerable) is skipped by
default (`--keep-adversarial` keeps it). Image turns use their caption.
"""
import json
import re
import sys


def convert(samples, keep_adversarial=False):
    convs = []
    for s in samples:
        conv = s["conversation"]
        turns = []
        sessions = sorted(
            (k for k in conv if re.fullmatch(r"session_\d+", k)),
            key=lambda k: int(k.split("_")[1]),
        )
        for sk in sessions:
            n = sk.split("_")[1]
            when = conv.get(f"{sk}_date_time")
            for t in conv[sk]:
                text = t.get("text") or ""
                if t.get("blip_caption"):
                    text = f"{text} [shares an image: {t['blip_caption']}]".strip()
                if when:
                    text = f"({when}) {text}"
                turns.append({"id": t["dia_id"], "speaker": t.get("speaker"), "text": text, "session": n})
        known = {t["id"] for t in turns}
        qa = []
        for q in s.get("qa", []):
            if q.get("category") == 5 and not keep_adversarial:
                continue
            ev = [e.strip() for e in q.get("evidence", []) if e.strip() in known]
            if not ev:
                continue
            qa.append({"question": q["question"], "evidence": ev, "answer": q.get("answer")})
        convs.append({"id": str(s.get("sample_id", len(convs))), "turns": turns, "qa": qa})
    return {"name": "locomo10", "conversations": convs}


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 2:
        sys.exit("usage: locomo_to_fixture.py <locomo10.json> <out-fixture.json> [--keep-adversarial]")
    with open(args[0]) as f:
        data = json.load(f)
    out = convert(data, "--keep-adversarial" in sys.argv)
    with open(args[1], "w") as f:
        json.dump(out, f)
    n = sum(len(c["qa"]) for c in out["conversations"])
    print(f"{len(out['conversations'])} conversations, {n} questions -> {args[1]}")
