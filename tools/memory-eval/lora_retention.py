#!/usr/bin/env python3
"""E3 LoRA-retention pipeline for Allternit Sleep (plan D6/D7, step 1.5, gate G3).

Question: can a LoRA adapter trained on a month of one user's dated facts
retain them? This tool is the DRY-RUN pipeline. Real training needs Eoj's OK
and a user-owned machine (no rented compute, no real training on Allternit
machines). It refuses real engines unless --allow-real is passed.

Steps: build the dataset from fixtures/month_of_facts.json, submit a LoRA job
in the Studio job format (services/studio-spec), probe four sets (learned,
paraphrase, reversal, general skill), print a markdown report.

Usage (standard library only, Python 3.9+):

  # print the job spec and stop (default)
  python3 tools/memory-eval/lora_retention.py

  # offline, engine API v1 command run directly (fake engine example):
  #   cargo build -p allternit-studio-spec --example fake_engine
  python3 tools/memory-eval/lora_retention.py \\
      --engine-cmd "target/debug/examples/fake_engine" --probe fake --out E3.md
  # {port} and {token} in --engine-cmd are replaced; when absent, "<port> <token>"
  # is appended (the fake_engine example's argument order).

  # through the real Studio routes of a running allternit-api
  # (host override: ALLTERNIT_STUDIO_ENGINE_CMD_ALLTERNIT_TUNE, engines.rs):
  python3 tools/memory-eval/lora_retention.py --api http://127.0.0.1:PORT \\
      --api-token "$TOKEN" --probe-cmd "claude -p" --out E3.md

Probes: --probe fake (deterministic, answers from the recorded training set,
only meaningful for the report shape) or --probe-cmd "<cli brain>" (prompt on
stdin, answer on stdout). Superseded facts: the probe expects the NEWEST value
and records whether an old value leaks into the answer.
"""
import argparse, json, os, re, secrets, shlex, socket, subprocess, sys, tempfile, time
import urllib.request, urllib.error
from pathlib import Path

HERE = Path(__file__).resolve().parent
FAKE_ENGINE_ID = "fake-engine"  # services/studio-spec/src/fake_engine.rs engine_info
TERMINAL = {"succeeded", "failed", "cancelled", "forward-only"}

GENERAL = [  # (question, accepted substrings, fake canned answer)
    ("What is 12 plus 15?", ["27"], "27"),
    ("What is the capital of France?", ["paris"], "Paris"),
    ("Spell the word 'cat' backwards.", ["tac"], "tac"),
    ("How many days are in a week?", ["7", "seven"], "7"),
    ("What colour do you get by mixing blue and yellow?", ["green"], "Green"),
    ("What is the opposite of 'hot'?", ["cold"], "Cold"),
    ("Name the largest planet in the solar system.", ["jupiter"], "Jupiter"),
    ("What is 9 times 8?", ["72"], "72"),
]


# ---------- fixture + dataset ----------
def norm(s):
    return re.sub(r"[^a-z0-9 ]+", "", s.lower().replace("&", "and")).strip()


def load_fixture(path):
    fx = json.load(open(path))
    facts = fx["facts"]
    superseded = {f["supersedes"] for f in facts if "supersedes" in f}
    latest = [f for f in facts if f["id"] not in superseded]
    by_id = {f["id"]: f for f in facts}
    for f in latest:  # old values for the same slot
        f["old_values"] = [g["value"] for g in facts
                           if g["slot"] == f["slot"] and g["id"] != f["id"]]
    return fx, latest, by_id


def chat(q, a):
    return {"messages": [{"role": "user", "content": q}, {"role": "assistant", "content": a}]}


def build_dataset(fx, latest, known_filter):
    """Rows + counts. Deterministic, template-based, no model call by default."""
    rows, counts = [], {"context_distillation": 0, "paraphrase": 0, "reversal": 0, "raw_text": 0}
    skipped = 0
    for f in latest:
        if known_filter and known_filter(f):
            skipped += 1
            continue
        rows.append(("learned", f, chat(f["question"], f"{f['value']}.")))
        counts["context_distillation"] += 1
        for p in f["paraphrases"][:2]:  # third paraphrase is held out for the probe
            rows.append(("paraphrase", f, chat(p, f"{f['value']}.")))
            counts["paraphrase"] += 1
        rows.append(("reversal", f, chat(f"What is {f['value']} to Mara?", f"It is Mara's {f['label']}.")))
        counts["reversal"] += 1
    for n in fx["day_notes"]:  # raw text so the mix is never only Q/A
        # allternit_tune takes one row format per dataset (data.py detect_format), so the
        # raw note is a chat row too: the note text is the assistant's verbatim reply.
        rows.append(("raw", None, chat(f"What did Mara note on {n['date']}?", n["text"])))
        counts["raw_text"] += 1
    return rows, counts, skipped


def write_dataset(rows, path):
    with open(path, "w") as fh:
        for _, _, r in rows:
            fh.write(json.dumps(r) + "\n")


def job_spec(dataset_path, base):
    return {
        "spec_version": 1, "engine": "allternit-tune", "engine_version": "0.1.0",
        "base": {"path": base},
        "dataset": {"path": str(dataset_path), "format": "chat"},
        "method": "lora",
        "params": {"rank": 16, "alpha": 32, "lr": 0.0002, "epochs": 3, "max_seq_len": 1024},
        "resources": {"place": "local", "backend": "auto", "max_hours": 1},
        "outputs": {"adapter": True, "merged": False, "gguf": [], "mlx": False},
        "eval": {"sets": [], "baseline": "base"},
    }


# ---------- engine protocol (engine-api.yaml / fake_engine.rs) ----------
def http(method, url, token, body=None, timeout=15):
    req = urllib.request.Request(url, method=method, data=None if body is None else json.dumps(body).encode())
    req.add_header("Authorization", f"Bearer {token}")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read())
        except Exception:
            return e.code, {"error": "http", "message": str(e)}


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def refuse_real(engine_id, allow_real):
    if allow_real:
        print("WARNING: --allow-real. Real training needs Eoj's OK and a user-owned machine; "
              "never on Allternit machines or rented compute.", file=sys.stderr)
        return
    print(f"REFUSED: engine '{engine_id}' does not report itself as fake. Real runs need Eoj's OK and a "
          "user-owned machine; pass --allow-real only there.", file=sys.stderr)
    sys.exit(3)


def watch(base_url, token, job_id, prefix="", timeout=120):
    """Poll events until a terminal state; returns the final job."""
    after, end = 0, time.time() + timeout
    while time.time() < end:
        _, ev = http("GET", f"{base_url}{prefix}/jobs/{job_id}/events?after={after}", token)
        for e in ev.get("events", []):
            after = max(after, e["seq"])
        if ev.get("state") in TERMINAL:
            break
        time.sleep(0.3)
    _, job = http("GET", f"{base_url}{prefix}/jobs/{job_id}", token)
    return job


def run_engine_cmd(cmd, spec, workdir, allow_real):
    port, token = free_port(), secrets.token_hex(8)
    if "{port}" in cmd or "{token}" in cmd:
        argv = shlex.split(cmd.replace("{port}", str(port)).replace("{token}", token))
    else:
        argv = shlex.split(cmd) + [str(port), token]
    env = dict(os.environ, ALLTERNIT_STUDIO_OUTPUT_DIR=str(workdir / "engine-out"))
    proc = subprocess.Popen(argv, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    base = f"http://127.0.0.1:{port}"
    try:
        card = None
        for _ in range(100):
            if proc.poll() is not None:
                sys.exit(f"engine command exited early: {proc.stderr.read().decode()[:400]}")
            try:
                st, card = http("GET", f"{base}/v1/engine", token, timeout=2)
                if st == 200:
                    break
            except OSError:
                time.sleep(0.2)
        else:
            sys.exit("engine did not answer GET /v1/engine in 20s")
        engine_id = card.get("id", "?")
        if engine_id != FAKE_ENGINE_ID:
            refuse_real(engine_id, allow_real)
        st, job = http("POST", f"{base}/v1/jobs", token, spec)
        if st != 201:
            sys.exit(f"engine refused the job ({st}): {job}")
        return engine_id, watch(base, token, job["id"], prefix="/v1")
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()


def run_api(api, token, spec, allow_real):
    api = api.rstrip("/") + "/api/v1" if "/api/v1" not in api else api.rstrip("/")
    st, eng = http("GET", f"{api}/studio/engines", token)
    names = json.dumps(eng).lower()
    engine_id = FAKE_ENGINE_ID if "fake engine" in names or FAKE_ENGINE_ID in names else spec["engine"]
    if engine_id != FAKE_ENGINE_ID:  # unverified: see 'not verified' in the PR
        refuse_real(engine_id, allow_real)
    st, job = http("POST", f"{api}/studio/jobs", token, spec)
    if st != 201:
        sys.exit(f"host refused the job ({st}): {job}")
    return engine_id, watch(api, token, job["id"], prefix="/studio")


# ---------- probes ----------
def probe_sets(latest):
    learned, para, rev = [], [], []
    for f in latest:
        exp = [f["value"]]
        learned.append((f["question"], exp, f))
        para.append((f["paraphrases"][2], exp, f))
        rev.append((f"{f['value']} is which part of Mara's life?", [f["label"]], f))
    gen = [(q, acc, None) for q, acc, _ in GENERAL]
    return {"learned": learned, "paraphrase": para, "reversal": rev, "general skill": gen}


def tokens(s):
    return set(norm(s).split())


def fake_brain(rows):
    qa = [(r["messages"][0]["content"], r["messages"][1]["content"]) for _, _, r in rows if "messages" in r]
    canned = {q: a for q, _, a in GENERAL}

    def ask(q):
        if q in canned:  # shape only: the fake cannot damage general skill
            return canned[q]
        best, score = None, 0.0
        for tq, ta in qa:
            a, b = tokens(q), tokens(tq)
            j = len(a & b) / max(1, len(a | b))
            if j > score:
                best, score = ta, j
        return best if score >= 0.5 else "I don't know."
    return ask


def cmd_brain(cmd):
    def ask(q):
        p = subprocess.run(shlex.split(cmd), input=f"Answer in one short sentence.\n{q}\n",
                           capture_output=True, text=True, timeout=180)
        return p.stdout.strip()
    return ask


def run_probes(sets, ask):
    res = {}
    for name, items in sets.items():
        ok = leak = leakable = 0
        for q, exp, f in items:
            a = norm(ask(q))
            ok += any(norm(e) in a for e in exp)
            if f and f.get("old_values"):
                leakable += 1
                leak += any(norm(o) in a and norm(o) not in norm(f["value"]) for o in f["old_values"])
        res[name] = {"n": len(items), "correct": ok, "leakable": leakable, "leaks": leak}
    return res


# ---------- report ----------
def pct(a, b):
    return "n/a" if not b else f"{100 * a / b:.1f}%"


def report(engine_id, job, counts, skipped, filter_note, res, probe_name, spec_path):
    L = ["# E3 LoRA retention report (Allternit Sleep)", "", f"engine: {'fake' if engine_id == FAKE_ENGINE_ID else engine_id}",
         f"job: {job.get('id')} state={job.get('state')}", f"probe: {probe_name}", "",
         "## Dataset", ""]
    L += [f"- context-distillation pairs: {counts['context_distillation']}",
          f"- paraphrase pairs: {counts['paraphrase']}", f"- reversal pairs: {counts['reversal']}",
          f"- raw day-note text rows: {counts['raw_text']}",
          f"- known-vs-unknown filter: {filter_note} (facts skipped: {skipped})", ""]
    for name, key in [("learned", "learned"), ("paraphrase", "paraphrase"),
                      ("reversal", "reversal"), ("general skill", "general skill")]:
        r = res[key]
        L += [f"## {name.capitalize()}", "", f"- accuracy: {r['correct']}/{r['n']} = {pct(r['correct'], r['n'])}"]
        if key in ("learned", "paraphrase", "reversal"):
            L.append(f"- superseded-leak rate: {r['leaks']}/{r['leakable']} = {pct(r['leaks'], r['leakable'])} "
                     "(old value appears in the answer)")
        L.append("")
    L += ["## G3 reading", ""]
    if engine_id == FAKE_ENGINE_ID:
        L.append("Experimental: no real run. The fake engine trains nothing; these numbers only prove the "
                 "pipeline and report shape. Go needs a real engine on a user-owned machine that showed "
                 "retention (learned, paraphrase and reversal up, general skill not damaged, low leak).")
    else:
        L.append("Go only if this run was on a user-owned machine and learned/paraphrase/reversal retention is "
                 "high with general skill undamaged and low superseded-leak; otherwise Experimental.")
    L.append("")
    return "\n".join(L)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--fixture", default=str(HERE / "fixtures" / "month_of_facts.json"))
    ap.add_argument("--workdir", help="where dataset.jsonl and engine output go (default: temp dir)")
    ap.add_argument("--base", default="/opt/allternit/models/qwen2.5-0.5b-instruct")
    ap.add_argument("--api", help="running allternit-api URL (real Studio routes)")
    ap.add_argument("--api-token", default=os.environ.get("ALLTERNIT_API_TOKEN", ""))
    ap.add_argument("--engine-cmd", help="engine API v1 command run directly (offline)")
    ap.add_argument("--allow-real", action="store_true", help="permit a non-fake engine (user-owned machine, Eoj's OK)")
    ap.add_argument("--probe", choices=["fake"], help="deterministic fake brain")
    ap.add_argument("--probe-cmd", help="CLI brain, prompt on stdin, e.g. 'claude -p'")
    ap.add_argument("--filter-known", action="store_true", help="skip facts the base brain already answers (needs --probe-cmd)")
    ap.add_argument("--out", help="also write the report here")
    a = ap.parse_args()

    fx, latest, _ = load_fixture(a.fixture)
    known, filter_note = None, "no-op (needs a model; recorded only)"
    if a.filter_known and a.probe_cmd:
        brain = cmd_brain(a.probe_cmd)
        known = lambda f: norm(f["value"]) in norm(brain(f["question"]))
        filter_note = "applied with --probe-cmd"
    rows, counts, skipped = build_dataset(fx, latest, known)
    workdir = Path(a.workdir or tempfile.mkdtemp(prefix="e3-lora-"))
    workdir.mkdir(parents=True, exist_ok=True)
    ds = workdir / "dataset.jsonl"
    write_dataset(rows, ds)
    spec = job_spec(ds.resolve(), a.base)

    if not (a.api or a.engine_cmd):
        print(json.dumps(spec, indent=2))
        print(f"\n# dataset: {ds} ({len(rows)} rows: {counts}); pass --engine-cmd or --api to submit.", file=sys.stderr)
        return
    if a.api:
        engine_id, job = run_api(a.api, a.api_token, spec, a.allow_real)
    else:
        engine_id, job = run_engine_cmd(a.engine_cmd, spec, workdir, a.allow_real)
    if job.get("state") != "succeeded":
        sys.exit(f"job did not succeed: state={job.get('state')} error={job.get('error')}")

    if a.probe == "fake":
        ask, pname = fake_brain(rows), "fake (answers from the recorded training set; general skill canned)"
    elif a.probe_cmd:
        ask, pname = cmd_brain(a.probe_cmd), f"cmd: {a.probe_cmd}"
        print("note: --probe-cmd asks the base brain, not the adapter, unless the command serves the "
              "adapter (load it first); wire the adapter into the brain before reading G3.", file=sys.stderr)
    else:
        sys.exit("job done; add --probe fake or --probe-cmd to run the probe sets")
    res = run_probes(probe_sets(latest), ask)
    md = report(engine_id, job, counts, skipped, filter_note, res, pname, ds)
    print(md)
    if a.out:
        Path(a.out).write_text(md)


if __name__ == "__main__":
    main()
