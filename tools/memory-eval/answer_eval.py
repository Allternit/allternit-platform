#!/usr/bin/env python3
"""E1 answer-eval harness for Allternit Sleep: compare memory arms by the
ANSWERS a model gives, not only retrieval recall.

Arms:
  none   question only
  raw    the full raw conversation history (turns, plus facts if present)
  topk   the incumbent recall top-k (ids from the cargo eval report, resolved
         back to fixture turn / fact text)
  pages  prototype topic page: one brain call per conversation turns the
         turns (+ facts) into short dated atomic claims with source turn ids

Scoring is per question category (LoCoMo `category`, LongMemEval
`question_type`, carried as an optional `category` on each `qa`) with
abstention: questions with no gold answer count correct only when the model
abstains.

The brain is a subscription CLI run as a subprocess (prompt on stdin), never
a metered API: --brain-cmd or env ALLTERNIT_EVAL_BRAIN_CMD, e.g. `claude -p`.
--brain fake is a deterministic offline brain: it answers with the gold
answer when that answer (normalized) appears in the arm's context, else
"I don't know". A fake run is therefore a real context-coverage check.

Standard library only. See docs: architecture/context-memory/memory.
"""
import argparse
import collections
import hashlib
import json
import os
import random
import re
import shlex
import subprocess
import sys

ABSTAIN_PHRASES = (
    "i don't know", "i do not know", "not mentioned", "no information",
    "cannot be determined", "can't be determined", "unanswerable", "not enough information",
    "not specified", "unknown",
)


def norm(s):
    s = re.sub(r"[^a-z0-9 ]+", " ", str(s).lower())
    return " ".join(s.split())


def is_abstain(text):
    t = str(text).lower().replace("’", "'")
    return any(p in t for p in ABSTAIN_PHRASES)


def gold_of(qa):
    a = qa.get("answer")
    if a is None or (isinstance(a, str) and not a.strip()):
        return None
    return str(a)


def is_abstention_q(qa):
    return gold_of(qa) is None or bool(qa.get("abstention"))


def ctok(s):
    return len(s) // 4


# ---- brain -----------------------------------------------------------------
class Brain:
    def __init__(self, kind, cmd):
        self.kind, self.cmd, self.calls = kind, cmd, 0

    def run(self, prompt):
        self.calls += 1
        if not self.cmd:
            sys.exit("no brain command: pass --brain-cmd or set ALLTERNIT_EVAL_BRAIN_CMD (e.g. 'claude -p')")
        p = subprocess.run(shlex.split(self.cmd), input=prompt, capture_output=True, text=True, timeout=600)
        if p.returncode != 0:
            sys.exit(f"brain command failed ({p.returncode}): {p.stderr[:300]}")
        return p.stdout.strip()


# ---- contexts --------------------------------------------------------------
def turn_line(t):
    who = f"{t['speaker']}: " if t.get("speaker") else ""
    return f"[{t['id']}] {who}{t['text']}"


def raw_context(conv):
    lines = [turn_line(t) for t in conv["turns"]]
    lines += [f"[{f['id']}] (fact) {f['text']}" for f in conv.get("facts", [])]
    return "\n".join(lines)


def topk_context(conv, ids):
    by = {t["id"]: turn_line(t) for t in conv["turns"]}
    by.update({f["id"]: f"[{f['id']}] (fact) {f['text']}" for f in conv.get("facts", [])})
    return "\n".join(by[i] for i in ids if i in by)


PAGE_PROMPT = (
    "Turn this conversation into a memory page of short dated atomic claims. One claim per "
    "line, in the form `- <date if known> | <claim> | src:<turn id>`. Keep every concrete "
    "detail (names, places, dates, numbers). If a later claim replaces an earlier one, keep "
    "both and mark the older `(superseded)`. No commentary.\n\n{history}\n"
)


def build_page(conv, brain, cache_dir, dry=False):
    if brain.kind == "fake":
        facts = conv.get("facts", [])
        if facts:
            return "\n".join(f"- | {f['text']} | src:{f['id']}" for f in facts)
        return "\n".join(f"- | {t['text']} | src:{t['id']}" for t in conv["turns"])
    prompt = PAGE_PROMPT.format(history=raw_context(conv))
    path = None
    if cache_dir:
        key = hashlib.sha1(prompt.encode()).hexdigest()[:16]
        path = os.path.join(cache_dir, f"page-{conv['id']}-{key}.md")
        if os.path.exists(path):
            return open(path).read()
    page = brain.run(prompt)
    if path:
        os.makedirs(cache_dir, exist_ok=True)
        open(path, "w").write(page)
    return page


ANSWER_PROMPT = (
    "Answer the question using only the memory below. Reply with a short answer. If the memory "
    "does not contain the answer, reply exactly: I don't know.\n\n"
    "Memory:\n{ctx}\n\nQuestion: {q}\nAnswer:"
)
NONE_CTX = "(none)"
JUDGE_PROMPT = (
    "Question: {q}\nGold answer: {gold}\nModel answer: {ans}\n"
    "Does the model answer match the gold answer in meaning? Reply with only CORRECT or WRONG."
)
JUDGE_ABS_PROMPT = (
    "Question: {q}\nThis question cannot be answered from the conversation.\nModel answer: {ans}\n"
    "Did the model correctly say it does not know? Reply with only CORRECT or WRONG."
)


def fake_answer(ctx, qa):
    g = gold_of(qa)
    if is_abstention_q(qa):
        return "I don't know"
    if g is not None and norm(g) and norm(g) in norm(ctx):
        return g
    return "I don't know"


def judge(qa, ans, kind, brain):
    g = gold_of(qa)
    if is_abstention_q(qa):
        if kind == "brain":
            return brain.run(JUDGE_ABS_PROMPT.format(q=qa["question"], ans=ans)).upper().startswith("CORRECT")
        return is_abstain(ans)
    if kind == "brain":
        return brain.run(JUDGE_PROMPT.format(q=qa["question"], gold=g, ans=ans)).upper().startswith("CORRECT")
    return norm(g) in norm(ans) and not is_abstain(ans)


# ---- run -------------------------------------------------------------------
def load_questions(fx, sample, seed):
    items = []
    for c in fx["conversations"]:
        for i, qa in enumerate(c["qa"]):
            items.append((c, i, qa))
    if sample and sample < len(items):
        items = random.Random(seed).sample(items, sample)
    return items


def topk_ids(report, conv, qa):
    for r in report["per_question"]:
        if r["conversation"] == conv["id"] and r["question"] == qa["question"]:
            return r["incumbent"]
    return []


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--fixture", required=True)
    ap.add_argument("--arms", default="none,raw,topk,pages")
    ap.add_argument("--topk-report")
    ap.add_argument("--brain", choices=["fake", "cmd"], default="fake")
    ap.add_argument("--brain-cmd", default=os.environ.get("ALLTERNIT_EVAL_BRAIN_CMD"))
    ap.add_argument("--judge", choices=["exact", "brain"], default="exact")
    ap.add_argument("--sample", type=int)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out")
    ap.add_argument("--dry", action="store_true")
    a = ap.parse_args()

    arms = [x for x in a.arms.split(",") if x]
    for x in arms:
        if x not in ("none", "raw", "topk", "pages"):
            sys.exit(f"unknown arm: {x}")
    fx = json.load(open(a.fixture))
    report = None
    if "topk" in arms:
        if not a.topk_report:
            sys.exit("arm topk needs --topk-report (the cargo eval report; run.sh makes it)")
        report = json.load(open(a.topk_report))
    items = load_questions(fx, a.sample, a.seed)
    brain = Brain(a.brain, a.brain_cmd)
    cache_dir = os.path.join(os.path.dirname(os.path.abspath(a.out)), "pages-cache") if a.out else None
    convs_used = list({c["id"]: c for c, _, _ in items}.values())

    pages = {}

    def ctx_for(arm, c, qa):
        if arm == "none":
            return NONE_CTX
        if arm == "raw":
            return raw_context(c)
        if arm == "topk":
            return topk_context(c, topk_ids(report, c, qa))
        return pages[c["id"]]

    if a.dry:
        print(f"fixture: {fx.get('name')}  questions: {len(items)}  conversations: {len(convs_used)}")
        print(f"brain: {a.brain}  judge: {a.judge}  arms: {','.join(arms)}")
        for arm in arms:
            builds = len(convs_used) if arm == "pages" else 0
            answers = len(items)
            judges = len(items) if a.judge == "brain" else 0
            print(f"  arm {arm}: real run = {answers} answer + {builds} page-build + {judges} judge = {answers + builds + judges} brain calls")
        c, _, qa = items[0]
        for arm in arms:
            if arm == "pages":
                pages[c["id"]] = build_page(c, Brain("fake", None), None) if a.brain == "fake" else "<page built by one brain call from the raw history>"
            print(f"\n--- first prompt, arm {arm} ---")
            print(ANSWER_PROMPT.format(ctx=ctx_for(arm, c, qa), q=qa["question"]))
        return

    results = {}
    for arm in arms:
        rows, ctoks = [], []
        before = brain.calls
        if arm == "pages":
            for c in convs_used:
                pages[c["id"]] = build_page(c, brain, cache_dir)
        for c, i, qa in items:
            ctx = ctx_for(arm, c, qa)
            ctoks.append(ctok(ctx))
            ans = fake_answer(ctx, qa) if a.brain == "fake" else brain.run(ANSWER_PROMPT.format(ctx=ctx, q=qa["question"]))
            ok = judge(qa, ans, a.judge, brain)
            rows.append({"conversation": c["id"], "question": qa["question"], "category": str(qa.get("category", "all")),
                         "abstention": is_abstention_q(qa), "answer": ans, "correct": bool(ok)})
        cat = collections.defaultdict(list)
        for r in rows:
            cat[r["category"]].append(r["correct"])
        ab = [r["correct"] for r in rows if r["abstention"]]
        results[arm] = {
            "accuracy": sum(r["correct"] for r in rows) / len(rows),
            "per_category": {k: sum(v) / len(v) for k, v in sorted(cat.items())},
            "abstention_accuracy": (sum(ab) / len(ab)) if ab else None,
            "abstention_n": len(ab),
            "mean_context_tokens": sum(ctoks) / len(ctoks),
            "brain_calls": brain.calls - before,
            "questions": len(rows),
            "rows": rows,
        }

    cats = sorted({k for r in results.values() for k in r["per_category"]})
    head = ["arm", "acc"] + [f"cat:{c}" for c in cats] + ["abstain", "ctx_tok", "calls"]
    table = [head]
    for arm in arms:
        r = results[arm]
        table.append([arm, f"{r['accuracy']:.2f}"] + [f"{r['per_category'].get(c, float('nan')):.2f}" for c in cats]
                     + ["n/a" if r["abstention_accuracy"] is None else f"{r['abstention_accuracy']:.2f}",
                        f"{r['mean_context_tokens']:.0f}", str(r["brain_calls"])])
    w = [max(len(row[i]) for row in table) for i in range(len(head))]
    print(f"fixture: {fx.get('name')}  questions: {len(items)}  brain: {a.brain}  judge: {a.judge}")
    for row in table:
        print("  ".join(c.ljust(w[i]) for i, c in enumerate(row)))
    if a.out:
        os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
        json.dump({"fixture": fx.get("name"), "brain": a.brain, "judge": a.judge, "sample": a.sample, "seed": a.seed,
                   "arms": results}, open(a.out, "w"), indent=1)


if __name__ == "__main__":
    main()
