#!/usr/bin/env python3
"""Write pack.json for an engine pack directory.

The shape is a superset of the Desktop runtime package manifest (RuntimeManifest in
surfaces/allternit-desktop/src/main/runtime-package.ts): version, build, platform, shellApi, files{path:{sha256,size}}.
The extra engine fields are ignored by the existing runtime client. See spec/on-device/engine-pack-spike.md.

Usage: make-manifest.py <pack-dir> --engine llama.cpp --backend metal --version b12414 --os macos --arch arm64 \
         --min-os 13.0 [--source-commit SHA] [--build UNIX_TS]
"""
import argparse, hashlib, json, os, sys, time

ap = argparse.ArgumentParser()
ap.add_argument("dir")
ap.add_argument("--engine", required=True)
ap.add_argument("--backend", required=True)
ap.add_argument("--version", required=True)
ap.add_argument("--os", required=True)
ap.add_argument("--arch", required=True)
ap.add_argument("--min-os", required=True)
ap.add_argument("--source-commit", default="")
ap.add_argument("--build", type=int, default=int(time.time()))
ap.add_argument("--entrypoint", default="llama-server")
a = ap.parse_args()

PLATFORM = {"macos": "darwin", "windows": "win32", "linux": "linux"}[a.os] + "-" + a.arch  # node process.platform-arch
files = {}
for root, _, names in os.walk(a.dir):
    for n in sorted(names):
        if n in ("pack.json", "pack.json.sig"):
            continue
        p = os.path.join(root, n)
        rel = os.path.relpath(p, a.dir)
        h = hashlib.sha256()
        with open(p, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        files[rel] = {"sha256": h.hexdigest(), "size": os.path.getsize(p)}

manifest = {
    "format": 1,
    "kind": "engine-pack",
    "pack_id": f"{a.engine}-{a.backend}-{a.os}-{a.arch}",
    "engine": a.engine,
    "backend": a.backend,
    "version": a.version,
    "build": a.build,
    "platform": PLATFORM,
    "shellApi": 1,
    "os": a.os,
    "arch": a.arch,
    "min_os": a.min_os,
    "entrypoint": a.entrypoint,
    "source_commit": a.source_commit,
    "files": dict(sorted(files.items())),
}
with open(os.path.join(a.dir, "pack.json"), "w") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
print(manifest["pack_id"], a.version, len(files), "files")
