#!/usr/bin/env python3
"""Convert LongMemEval (longmemeval_s / longmemeval_oracle json) to the
allternit-api memory eval fixture format (see
cmd/allternit-api/src/memory_retrieve/eval.rs).

LongMemEval is NOT vendored in this repo: check its license upstream
(github.com/xiaowu0162/LongMemEval; the repo is MIT-licensed, the data is on
Hugging Face xiaowu0162/longmemeval-cleaned, re-check its card) before
redistributing. Download it yourself, then:

    python3 tools/memory-eval/longmemeval_to_fixture.py longmemeval_s.json /tmp/lme-fixture.json [--limit N]

Each question becomes its own conversation (its haystack is private to it).
Turn ids are S<session>:<turn> (1-based). Gold evidence is the turns with
`has_answer: true` when present, else every turn of the answer sessions.
`category` carries question_type; question ids ending `_abs` are abstention
questions (the fixture keeps the gold answer, `abstention: true` marks them).
Eval.rs ignores the extra qa fields.
"""
import json
import sys


def convert(items, limit=None):
    convs = []
    for it in items[:limit] if limit else items:
        turns, answer_turns, by_session = [], [], {}
        sids = it.get("haystack_session_ids") or [str(i + 1) for i in range(len(it["haystack_sessions"]))]
        dates = it.get("haystack_dates") or [None] * len(sids)
        for si, (sid, sess) in enumerate(zip(sids, it["haystack_sessions"]), start=1):
            for ti, t in enumerate(sess, start=1):
                tid = f"S{si}:{ti}"
                text = t["content"]
                if dates[si - 1]:
                    text = f"({dates[si - 1]}) {text}"
                turns.append({"id": tid, "speaker": t.get("role"), "text": text, "session": str(si)})
                by_session.setdefault(sid, []).append(tid)
                if t.get("has_answer"):
                    answer_turns.append(tid)
        ev = answer_turns or [x for s in it.get("answer_session_ids", []) for x in by_session.get(s, [])]
        qid = str(it["question_id"])
        ab = qid.endswith("_abs")
        qa = {"question": it["question"], "evidence": ev, "answer": it.get("answer"),
              "category": it.get("question_type"), "abstention": ab}
        convs.append({"id": qid, "turns": turns, "qa": [qa]})
    return {"name": "longmemeval", "conversations": convs}


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    limit = None
    if "--limit" in sys.argv:
        limit = int(sys.argv[sys.argv.index("--limit") + 1])
        args = [a for a in args if a != str(limit)]
    if len(args) != 2:
        sys.exit("usage: longmemeval_to_fixture.py <longmemeval.json> <out-fixture.json> [--limit N]")
    with open(args[0]) as f:
        data = json.load(f)
    out = convert(data, limit)
    with open(args[1], "w") as f:
        json.dump(out, f)
    print(f"{len(out['conversations'])} conversations -> {args[1]}")
