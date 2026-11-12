#!/usr/bin/env python3
"""Copy the Platform API OpenAPI contract into the docs site.

  python3 surfaces/docs/scripts/sync_openapi.py          # copy
  python3 surfaces/docs/scripts/sync_openapi.py --check  # exit 1 on drift

Source of truth: cmd/allternit-cloud-api/openapi/platform-v1.yaml
Target:          surfaces/docs/api-reference/platform-v1.yaml
(inference-v1.yaml is written by hand in the docs folder and is not synced.)
"""
from __future__ import annotations

import shutil
import sys
from pathlib import Path

DOCS = Path(__file__).resolve().parent.parent
REPO = DOCS.parent.parent
SRC = REPO / "cmd" / "allternit-cloud-api" / "openapi" / "platform-v1.yaml"
DST = DOCS / "api-reference" / "platform-v1.yaml"


def main() -> int:
    if not SRC.is_file():
        print(f"missing source: {SRC}", file=sys.stderr)
        return 2
    if "--check" in sys.argv[1:]:
        if DST.is_file() and DST.read_bytes() == SRC.read_bytes():
            print("openapi in sync")
            return 0
        print(f"openapi drift: run python3 surfaces/docs/scripts/sync_openapi.py to update {DST.relative_to(REPO)}", file=sys.stderr)
        return 1
    DST.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SRC, DST)
    print(f"copied {SRC.relative_to(REPO)} -> {DST.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
