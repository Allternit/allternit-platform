#!/usr/bin/env python3
"""Lean offline docs check: navigation, internal links and anchors.

  python3 surfaces/docs/scripts/check_links.py            # check every page
  python3 surfaces/docs/scripts/check_links.py architecture api/agency   # limit scope

Checks:
  1. docs.json is valid JSON.
  2. Every navigation page entry points to an existing .mdx/.md file.
  3. Every internal link (markdown `](/x)` and `href="/x"`) resolves to a page or a
     static file under surfaces/docs.
  4. Every `#anchor` on an internal link matches a heading on the target page.
Exits 1 on any failure. No network, no Mintlify install needed.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

DOCS = Path(__file__).resolve().parent.parent
SKIP = {"node_modules", ".mintlify"}


def pages_in_nav(node, out):
    if isinstance(node, dict):
        for k, v in node.items():
            if k == "pages" and isinstance(v, list):
                for p in v:
                    if isinstance(p, str):
                        out.append(p)
                    else:
                        pages_in_nav(p, out)
            else:
                pages_in_nav(v, out)
    elif isinstance(node, list):
        for x in node:
            pages_in_nav(x, out)


def page_file(route: str) -> Path | None:
    route = route.strip("/")
    for cand in (f"{route}.mdx", f"{route}.md", f"{route}/index.mdx"):
        if (DOCS / cand).is_file():
            return DOCS / cand
    return None


def slug(heading: str) -> str:
    h = re.sub(r"`|\*|\[|\]\([^)]*\)", "", heading).strip().lower()
    h = re.sub(r"[^\w\s-]", "", h)
    return re.sub(r"\s+", "-", h)


_anchor_cache: dict[Path, set[str]] = {}


def anchors(path: Path) -> set[str]:
    if path not in _anchor_cache:
        text = re.sub(r"```.*?```", "", path.read_text(), flags=re.S)
        _anchor_cache[path] = {slug(m) for m in re.findall(r"(?m)^#{1,6}\s+(.+?)\s*$", text)}
    return _anchor_cache[path]


LINK = re.compile(r"\]\((/[^)\s]*)\)|href=\"(/[^\"]*)\"")


def main() -> int:
    scope = [a.strip("/") for a in sys.argv[1:]]
    errors = []
    try:
        cfg = json.loads((DOCS / "docs.json").read_text())
    except json.JSONDecodeError as e:
        print(f"docs.json invalid JSON: {e}")
        return 1
    nav: list[str] = []
    pages_in_nav(cfg.get("navigation", {}), nav)
    for p in nav:
        if page_file(p) is None:
            errors.append(f"docs.json: nav entry '{p}' has no file")
    # A page that exists locally but is gitignored (e.g. under build/) never
    # reaches the publish runner; flag nav pages git would not commit.
    try:
        import subprocess
        ignored = subprocess.run(
            ["git", "check-ignore", "--stdin"], cwd=DOCS, text=True, capture_output=True,
            input="\n".join(str(page_file(p).relative_to(DOCS)) for p in nav if page_file(p)),
        ).stdout.split()
        for f in ignored:
            errors.append(f"docs.json: nav page '{f}' is gitignored and will not publish")
    except (OSError, ValueError):
        pass
    files = [
        f for f in DOCS.rglob("*.mdx")
        if not SKIP & set(f.parts)
        and (not scope or any(str(f.relative_to(DOCS)).startswith(s) for s in scope))
    ]
    for f in files:
        text = re.sub(r"```.*?```", "", f.read_text(), flags=re.S)
        for m in LINK.finditer(text):
            target = m.group(1) or m.group(2)
            path, _, frag = target.partition("#")
            if path in ("", "/"):
                dest = f if path == "" else page_file("introduction")
            elif (DOCS / path.strip("/")).is_file():
                continue  # static file (yaml, image, ...)
            else:
                dest = page_file(path)
            rel = f.relative_to(DOCS)
            if dest is None:
                errors.append(f"{rel}: broken link {target}")
            elif frag and frag not in anchors(dest):
                errors.append(f"{rel}: missing anchor {target}")
    for e in errors:
        print(e)
    print(f"checked {len(nav)} nav entries, {len(files)} pages: {len(errors)} problem(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
