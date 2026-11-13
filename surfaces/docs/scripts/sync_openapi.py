#!/usr/bin/env python3
"""Copy OpenAPI contracts into the docs site.

  python3 surfaces/docs/scripts/sync_openapi.py          # copy
  python3 surfaces/docs/scripts/sync_openapi.py --check  # exit 1 on drift

Sources of truth -> targets:
  cmd/allternit-cloud-api/openapi/platform-v1.yaml -> surfaces/docs/api-reference/platform-v1.yaml
  cmd/allternit-api/openapi/selfhost-v1.yaml       -> surfaces/docs/api-reference/selfhost-v1.yaml
(inference-v1.yaml is written by hand in the docs folder and is not synced.)

The self-hosted copy gets `x-excluded: true` next to every `x-internal: true`,
so Mintlify leaves the loopback-only route out of the reference. Nothing else
changes. No dependencies.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

DOCS = Path(__file__).resolve().parent.parent
REPO = DOCS.parent.parent


def exclude_internal(text: str) -> str:
    return re.sub(r"(?m)^(\s*)x-internal: true\s*$", r"\g<0>\n\1x-excluded: true", text)


SPECS = [
    (REPO / "cmd" / "allternit-cloud-api" / "openapi" / "platform-v1.yaml", DOCS / "api-reference" / "platform-v1.yaml", None),
    (REPO / "cmd" / "allternit-api" / "openapi" / "selfhost-v1.yaml", DOCS / "api-reference" / "selfhost-v1.yaml", exclude_internal),
]


def main() -> int:
    check = "--check" in sys.argv[1:]
    drift = []
    for src, dst, transform in SPECS:
        if not src.is_file():
            print(f"missing source: {src}", file=sys.stderr)
            return 2
        text = src.read_text()
        want = transform(text) if transform else text
        if check:
            if not dst.is_file() or dst.read_text() != want:
                drift.append(dst)
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text(want)
        print(f"copied {src.relative_to(REPO)} -> {dst.relative_to(REPO)}")
    if check:
        if drift:
            names = ", ".join(str(d.relative_to(REPO)) for d in drift)
            print(f"openapi drift: run python3 surfaces/docs/scripts/sync_openapi.py to update {names}", file=sys.stderr)
            return 1
        print("openapi in sync")
    return 0


if __name__ == "__main__":
    sys.exit(main())
