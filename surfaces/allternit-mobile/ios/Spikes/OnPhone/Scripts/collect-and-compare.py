#!/usr/bin/env python3
"""Pull the on-phone report off the device and apply the S0 pass bar.

    Scripts/collect-and-compare.py --udid <udid> --out <dir> [--helper helper-*.json ...]

Writes <out>/s0-result.json and <out>/s0-result.md. Pass bar: 95% of taps land,
median read at most 1.5x the helper (per app when the helper run is labelled
with that app, else the helper's overall median), typing exact.
"""
import argparse, json, pathlib, statistics, subprocess, sys

BUNDLE = "com.allternit.mobile.onphone-spike"


def pull(udid: str, out: pathlib.Path) -> dict:
    for name in ("onphone-report.json", "onphone-report.md"):
        subprocess.run(["xcrun", "devicectl", "device", "copy", "from", "--device", udid,
                        "--domain-type", "appDataContainer", "--domain-identifier", BUNDLE,
                        "--source", f"Documents/{name}", "--destination", str(out / name)],
                       check=True, capture_output=True)
    return json.loads((out / "onphone-report.json").read_text())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--udid")
    ap.add_argument("--report", help="use a report already on disk instead of pulling")
    ap.add_argument("--out", required=True)
    ap.add_argument("--helper", nargs="*", default=[])
    a = ap.parse_args()
    out = pathlib.Path(a.out); out.mkdir(parents=True, exist_ok=True)
    rep = json.loads(pathlib.Path(a.report).read_text()) if a.report else pull(a.udid, out)
    items = {i["id"]: i for i in rep.get("items", [])}

    helpers = [json.loads(pathlib.Path(h).read_text()) for h in a.helper]
    by_app = {h["app"].lower(): h["read"]["median"] for h in helpers if h["read"]["median"]}
    all_reads = [v for h in helpers for v in h["read"]["samples"]]
    overall = statistics.median(all_reads) if all_reads else None

    checks = []
    taps = items.get("tap.landed", {})
    checks.append({"bar": "taps >= 95% land", "status": "blocked" if taps.get("status") == "blocked" else "not_run",
                   "detail": taps.get("note", "")})
    for app in ("settings", "messages", "safari", "notes", "store_app"):
        it = items.get(f"read.{app}", {})
        mine, base = it.get("value"), by_app.get(app.replace("_", " "), overall)
        if mine is None or base is None:
            checks.append({"bar": f"read {app} <= 1.5x helper", "status": "not_run", "phone_ms": mine, "helper_ms": base})
        else:
            checks.append({"bar": f"read {app} <= 1.5x helper", "status": "pass" if mine <= 1.5 * base else "fail",
                           "phone_ms": mine, "helper_ms": base, "ratio": round(mine / base, 2)})
    ty = items.get("type.exact", {})
    checks.append({"bar": "typing exact (200 chars)", "status": {"ok": "pass", "fail": "fail"}.get(ty.get("status"), ty.get("status", "not_run")),
                   "detail": ty.get("note", "")})
    verdict = "fail" if any(c["status"] in ("fail", "blocked") for c in checks) else (
        "pass" if all(c["status"] == "pass" for c in checks) else "incomplete")
    result = {"verdict": verdict, "checks": checks, "device": rep.get("device"), "helpers": helpers,
              "observed": {k: v for k, v in items.items() if k.startswith(("tunnel", "pairing", "screen", "resilience", "battery"))}}
    (out / "s0-result.json").write_text(json.dumps(result, indent=2))
    md = [f"# S0 result: {verdict}", "", "| Bar | Status | Detail |", "|---|---|---|"]
    for c in checks:
        d = c.get("detail") or (f"phone {c.get('phone_ms')} ms vs helper {c.get('helper_ms')} ms" + (f" (x{c['ratio']})" if "ratio" in c else ""))
        md.append(f"| {c['bar']} | {c['status']} | {str(d).replace('|', '/')[:300]} |")
    (out / "s0-result.md").write_text("\n".join(md) + "\n")
    print((out / "s0-result.md").read_text())
    return 0


if __name__ == "__main__":
    sys.exit(main())
