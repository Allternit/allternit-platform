#!/usr/bin/env bash
# WP12 conformance report: runs the kernel layer (always) and the API layer
# (only with --api <base-url>), then prints one row per coverage-ledger row.
# The output is what flips the ledger's "conformance verified" column; the
# ledger itself is edited by a human/orchestrator, never by this script.
#
#   scripts/conformance-report.sh                       # kernel layer only
#   scripts/conformance-report.sh --api http://127.0.0.1:8080   # + API layer
#   AGENCY_TOKEN=... AGENCY_MODEL_CLASS_A=mc.x AGENCY_MODEL_CLASS_B=mc.y ...
#
# Exit code: 0 only if every row that was run passes AND (for the alpha gate)
# both layers ran. Kernel-only runs exit 0 with the gate marked "API layer not run".
set -uo pipefail
cd "$(dirname "$0")/.."
API=""
[ "${1:-}" = "--api" ] && API="${2:?--api needs a base URL}"
OUT="$(mktemp -d)"

cargo test -p allternit-factory --test conformance 2>&1 | tee "$OUT/kernel.txt" | grep -E "^test result|error(\[|:)" >&2
if [ -n "$API" ]; then
  # Runs take longer than bun's 5 s default per-test timeout.
  AGENCY_BASE_URL="$API" bun test --reporter=junit --reporter-outfile="$OUT/api.xml" --timeout $(( (${AGENCY_RUN_TIMEOUT_S:-120} * 2 + 90) * 1000 )) ./tests/agency-conformance/agency-api.conformance.ts 2>&1 | tee "$OUT/api.txt" | tail -5 >&2
fi

OUT="$OUT" API="$API" python3 - <<'PY'
import os, re, sys
out = os.environ["OUT"]; api_ran = bool(os.environ["API"])
def grab(path, rx):
    r = {}
    try: text = open(path).read()
    except OSError: return r
    for m in re.finditer(rx, text, re.M):
        r[m.group(2) if m.lastindex >= 2 else m.group(1)] = m.group(1)
    return r
k = {n: s for n, s in re.findall(r"^test (\S+) \.\.\. (\w+)", open(f"{out}/kernel.txt").read(), re.M)}
a = {}
if api_ran:
    # bun prints only failures when stdout is not a TTY, so read the JUnit
    # report (every test case) and fall back to the text lines.
    rows = []
    if os.path.exists(f"{out}/api.xml"):
        import xml.etree.ElementTree as ET
        for tc in ET.parse(f"{out}/api.xml").iter("testcase"):
            st = "fail" if tc.find("failure") is not None or tc.find("error") is not None else "skip" if tc.find("skipped") is not None else "pass"
            rows.append((st, tc.get("name", "").split(" > ")[-1]))
    if not rows:
        rows = re.findall(r"^\((pass|fail|skip)\) (?:.*> )?(\S+)", open(f"{out}/api.txt").read(), re.M)
    for s, n in rows:
        a[n] = s
G = lambda d, *p: [n for n in d if n.startswith(p)]
# ledger row -> (description, kernel groups, api groups)
ROWS = [
  ("CL-282", "Agency API alpha gate: restart, resume, verify, replay, receipt-backed completion", ("restart_","resume_","receipt_completion_","replay_"), ("restart_","resume_","receipt_completion_","replay_")),
  ("CL-001", "Model swap does not change loop/state/tool authority/completion/graph", ("model_swap_",), ("model_swap_",)),
  ("CL-154", "First-runtime milestone: swap backend, restart reconstruct", ("restart_","model_swap_"), ("restart_","model_swap_")),
  ("CL-170", "Evidence-backed completion law", ("receipt_completion_",), ("receipt_completion_",)),
  ("CL-226", "Verifier-owned completion (builders propose)", ("receipt_completion_","duplicate_close"), ("receipt_completion_",)),
  ("CL-173", "Conformance gates: receipt completeness, deterministic replay", ("replay_","fault_torn","restart_receipt"), ("replay_",)),
  ("CL-180", "Failure-injection matrix (killed executor, torn write, budget)", ("fault_","resume_killed"), ("fault_",)),
  ("CL-141", "Tool receipts / idempotency keys", ("duplicate_effect","fault_crash_mid"), ("duplicate_",)),
  ("CL-123", "Concurrency: fenced leases, one driver per DAG", ("concurrency_",), ("concurrency_",)),
  ("CL-133", "Rollback / checkpoints", ("rollback_",), ("rollback_",)),
  ("CL-227", "Network egress guard (private/metadata addresses)", ("security_egress",), ()),
  ("CL-011", "Policy outranks probability (deny beats bypass)", ("security_deny",), ("security_",)),
  ("CL-010", "Model ABI never model names", ("security_registry","security_no_vendor","model_swap_"), ("security_default_views","model_swap_")),
  ("CL-014", "Hardware/vendor independence: no vendor names in contracts", ("security_no_vendor","security_registry"), ("security_default_views",)),
]
def verdict(d, groups, ran):
    if not groups: return "n/a"
    if not ran: return "not run"
    names = G(d, *groups)
    if not names: return "no tests"
    st = [d[n] for n in names]
    if any(s in ("FAILED","fail") for s in st): return "FAIL"
    if all(s in ("skip","ignored") for s in st): return "skipped"
    return f"PASS ({sum(s in ('ok','pass') for s in st)}/{len(st)})"
w = max(len(r[1]) for r in ROWS)
print(f"\n{'ledger row':<8}  {'kernel':<12} {'api':<12} {'conformance verified':<22} verifies")
bad = False; all_ok = True
for row, desc, kg, ag in ROWS:
    kv, av = verdict(k, kg, True), verdict(a, ag, api_ran)
    fail = "FAIL" in (kv, av)
    both = kv.startswith("PASS") and (av.startswith("PASS") or av == "n/a")
    kernel_only = kv.startswith("PASS") and av in ("not run", "skipped")
    flip = "FAIL" if fail else "yes" if both else "kernel only (API pending)" if kernel_only else "no"
    bad |= fail
    print(f"{row:<8}  {kv:<12} {av:<12} {flip:<22} {desc}")
tot = len(k); ok = sum(v == "ok" for v in k.values())
print(f"\nkernel layer: {ok}/{tot} passed" + (f"; api layer: {sum(v=='pass' for v in a.values())}/{len(a)} passed, {sum(v=='skip' for v in a.values())} skipped" if api_ran else "; api layer not run (pass --api <base-url> once WP11 is merged)"))
gate = "OPEN" if (not bad and api_ran and a and all(v == "pass" for v in a.values()) and ok == tot) else "CLOSED (needs both layers green)"
print(f"alpha gate CL-282: {gate}")
sys.exit(1 if bad else 0)
PY
