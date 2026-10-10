#!/usr/bin/env python3
"""Side-by-side decision backends: the same decisions through each backend of
allternit-api's `POST /v1/decisions`, one table of latency and accuracy
(decision-runtime spec phase E6).

    python3 compare_backends.py --api-bin <allternit-api> --cases cases.jsonl \
        --backends local --local-url http://127.0.0.1:PORT --limit 60

Each case runs once per backend with `backends: [name]`, so it goes through
the same adapters, validation and store as production. `--api URL` uses a
running allternit-api; `--api-bin` boots a private one on a loopback port with
a scratch data dir (single-user dev auth, like the computer-use suite).

Cases are JSONL, either native
    {"context": ..., "options": [{"id", "text"}], "kind": ..., "question": ..., "label": <id>}
or chat rows in the decision-model training format (system / user JSON with
evidence, criterion and lettered options / assistant letter), which are
converted on the fly.

`--passes 2 --feedback` replays the cases with each outcome PATCHed back, so
the second pass shows what the flywheel's lookup head would do; its shadow
metrics are printed from `GET /v1/decisions/heads`.

Vendor backends (`vendor`, `typesafe`) are metered: they are refused unless
`--allow-metered` is passed, and they also need the key and the project
enabled in the API's environment. Get Eoj's OK on the spend first.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

METERED = {"vendor", "typesafe", "oracle"}


def http(method: str, url: str, body: dict | None = None, token: str = "", timeout: float = 120) -> tuple[int, dict]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={"content-type": "application/json"})
    if token:
        req.add_header("authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read() or b"{}")
        except ValueError:
            return e.code, {}


def load_cases(path: Path, limit: int, seed: int) -> list[dict]:
    rows = []
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        r = json.loads(line)
        if "messages" in r:
            user = next(m["content"] for m in r["messages"] if m["role"] == "user")
            answer = next(m["content"] for m in r["messages"] if m["role"] == "assistant").strip()
            u = json.loads(user)
            ev = u["evidence"]
            ctx = ev if isinstance(ev, str) else json.dumps(ev)
            opts = [{"id": o["letter"], "text": o["description"]} for o in u["options"]]
            if not (2 <= len(opts) <= 255) or answer not in {o["id"] for o in opts}:
                continue
            rows.append({"context": ctx[:60000], "options": opts, "kind": r.get("kind", "choice"), "question": u.get("criterion"), "label": answer})
        else:
            rows.append(r)
    random.Random(seed).shuffle(rows)
    return rows[:limit] if limit else rows


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def boot(bin_path: str, env_extra: dict) -> tuple[str, subprocess.Popen, Path]:
    work = Path(tempfile.mkdtemp(prefix="decisions-compare-"))
    port = free_port()
    env = dict(os.environ, ALLTERNIT_API_HOST="127.0.0.1", ALLTERNIT_API_PORT=str(port),
               ALLTERNIT_DATA_DIR=str(work / "api-data"), ALLTERNIT_LOCAL_DEV_BYPASS="1", **env_extra)
    log = open(work / "api.log", "w")
    proc = subprocess.Popen([bin_path], env=env, stdout=log, stderr=subprocess.STDOUT)
    api = f"http://127.0.0.1:{port}"
    for _ in range(300):
        try:
            if http("GET", f"{api}/v1/decisions/heads", timeout=2)[0] < 500:
                return api, proc, work
        except OSError:
            pass
        if proc.poll() is not None:
            break
        time.sleep(0.3)
    proc.terminate()
    raise SystemExit(f"allternit-api did not start; see {work / 'api.log'}")


def pct(xs: list[float], p: float) -> float | None:
    if not xs:
        return None
    xs = sorted(xs)
    return xs[min(len(xs) - 1, round((len(xs) - 1) * p))]


def fmt(x: float | None, digits: int = 1) -> str:
    return "-" if x is None else f"{x:.{digits}f}"


def run(api: str, token: str, cases: list[dict], backend: str, args) -> dict:
    st = {"n": 0, "answered": 0, "correct": 0, "abstained": 0, "errors": 0, "lat": [], "rtt": []}
    for c in cases:
        body = {"context": c["context"], "options": c["options"], "kind": c.get("kind", "choice"), "question": c.get("question"),
                "allow_abstain": True, "backends": [backend], "latency_budget_ms": args.budget_ms, "task": "compare_backends"}
        if args.project:
            body["project"] = args.project
        t = time.perf_counter()
        status, r = http("POST", f"{api}/v1/decisions", body, token)
        st["rtt"].append((time.perf_counter() - t) * 1000)
        st["n"] += 1
        if status >= 300:
            st["errors"] += 1
            continue
        att = next((a for a in r.get("attempts", []) if a.get("backend") == r.get("backend")), None)
        if att and "latency_ms" in att:
            st["lat"].append(att["latency_ms"])
        if any("error" in a for a in r.get("attempts", [])) and r.get("backend") in (None, "none"):
            st["errors"] += 1
        if r.get("abstained"):
            st["abstained"] += 1
        if r.get("choice") is not None:
            st["answered"] += 1
            st["correct"] += r["choice"] == c["label"]
        if args.feedback and r.get("id"):
            ok = r.get("choice") == c["label"]
            http("PATCH", f"{api}/v1/decisions/{r['id']}", {"status": "success" if ok else "failure", "label": c["label"]}, token)
    return st


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--api", help="a running allternit-api base URL")
    ap.add_argument("--api-bin", help="boot this allternit-api binary privately instead")
    ap.add_argument("--token", default=os.environ.get("ALLTERNIT_API_TOKEN", ""))
    ap.add_argument("--cases", required=True, type=Path)
    ap.add_argument("--limit", type=int, default=60)
    ap.add_argument("--seed", type=int, default=20261009)
    ap.add_argument("--backends", default="local")
    ap.add_argument("--local-url", help="ALLTERNIT_DECISIONS_LOCAL_URL for a booted API")
    ap.add_argument("--budget-ms", type=int, default=5000)
    ap.add_argument("--project", help="project id (vendor backends are enabled per project)")
    ap.add_argument("--passes", type=int, default=1)
    ap.add_argument("--feedback", action="store_true", help="PATCH each outcome back (feeds the flywheel)")
    ap.add_argument("--allow-metered", action="store_true", help="allow vendor / oracle backends (paid; needs Eoj's OK)")
    ap.add_argument("--json", type=Path, help="also write the raw results here")
    args = ap.parse_args()

    backends = [b.strip() for b in args.backends.split(",") if b.strip()]
    if METERED & set(backends) and not args.allow_metered:
        raise SystemExit(f"{sorted(METERED & set(backends))} are metered: pass --allow-metered once the spend is approved")
    cases = load_cases(args.cases, args.limit, args.seed)
    if not cases:
        raise SystemExit("no cases")
    proc = None
    if args.api_bin:
        extra = {"ALLTERNIT_DECISIONS_ORACLE": "0"}
        if args.local_url:
            extra["ALLTERNIT_DECISIONS_LOCAL_URL"] = args.local_url
        api, proc, work = boot(args.api_bin, extra)
        print(f"booted allternit-api at {api} (data {work})", file=sys.stderr)
    elif args.api:
        api = args.api.rstrip("/")
    else:
        raise SystemExit("pass --api or --api-bin")
    try:
        results = []
        for p in range(1, args.passes + 1):
            for b in backends:
                st = run(api, args.token, cases, b, args)
                results.append({"pass": p, "backend": b, **st})
        print(f"\n{len(cases)} decisions from {args.cases.name}, budget {args.budget_ms} ms\n")
        print("| pass | backend | n | answered | accuracy (all) | accuracy (answered) | abstained | errors | backend p50 ms | p95 ms | round trip p50 ms |")
        print("|---|---|---|---|---|---|---|---|---|---|---|")
        for r in results:
            n = r["n"] or 1
            acc_ans = r["correct"] / r["answered"] if r["answered"] else None
            print(f"| {r['pass']} | {r['backend']} | {r['n']} | {r['answered']} | {fmt(100 * r['correct'] / n)}% | "
                  f"{fmt(None if acc_ans is None else 100 * acc_ans)}% | {r['abstained']} | {r['errors']} | "
                  f"{fmt(pct(r['lat'], .5))} | {fmt(pct(r['lat'], .95))} | {fmt(statistics.median(r['rtt']) if r['rtt'] else None)} |")
        status, heads = http("GET", f"{api}/v1/decisions/heads", token=args.token)
        if status < 300 and heads.get("data"):
            print("\nFlywheel heads (shadow metrics since the last transition):\n")
            print("| head | kind | state | decisions | coverage | agreement | accuracy | labelled | head p50 ms | p95 ms |")
            print("|---|---|---|---|---|---|---|---|---|---|")
            for h in heads["data"]:
                m = h["metrics"]
                pc = lambda x: "-" if x is None else f"{100 * x:.1f}%"  # noqa: E731
                print(f"| {h['head']} | {h['kind']} | {h['state']} | {m['decisions']} | {pc(m['coverage'])} | {pc(m['agreement'])} | "
                      f"{pc(m['accuracy'])} | {m['labelled']} | {fmt(m['p50_latency_ms'], 3)} | {fmt(m['p95_latency_ms'], 3)} |")
        if args.json:
            args.json.write_text(json.dumps({"results": [{k: v for k, v in r.items()} for r in results], "heads": heads}, indent=2))
    finally:
        if proc:
            proc.terminate()
            proc.wait(timeout=10)


if __name__ == "__main__":
    main()
