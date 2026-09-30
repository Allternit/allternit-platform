#!/usr/bin/env python3
"""Write sha256 of every normative + generated file into MANIFEST.json `files` (reproducibility gate)."""
import glob, hashlib, json, os
V1 = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
pats = ["schemas/*.json", "registry/*.json", "data/*.json", "generated/ts/*.d.ts",
        "generated/rust/Cargo.toml", "generated/rust/src/*.rs"]
files = sorted(p for pat in pats for p in glob.glob(os.path.join(V1, pat)))
m = json.load(open(os.path.join(V1, "MANIFEST.json")))
m["files"] = {os.path.relpath(p, V1): "sha256:" + hashlib.sha256(open(p, "rb").read()).hexdigest() for p in files}
open(os.path.join(V1, "MANIFEST.json"), "w").write(json.dumps(m, indent=2, ensure_ascii=False) + "\n")
print(f"hashed {len(files)} files into MANIFEST.json")
