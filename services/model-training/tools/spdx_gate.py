#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""License gate for allternit-tune.

    python3 tools/spdx_gate.py [ROOT]

ROOT defaults to services/model-training (this file's parent folder). Exits 1
and lists every problem when:
- a file has no SPDX-License-Identifier header in its first 10 lines, or the
  header names a license that is not Apache-2.0-compatible (permissive);
- Python code imports the AGPL-licensed trainer library or its zoo package
  (import, from-import, importlib.import_module, __import__);
- a requirements / pyproject / setup file lists either package.

LICENSE and NOTICE are exempt from the header rule. Standard library only and
Python 3.9+, so it runs on a stock macOS python3 and in CI.
"""

from __future__ import annotations

import ast
import re
import sys
from pathlib import Path
from typing import Iterable, List

# Written split so a repo-wide text search for the name stays clean: the only
# place the name may appear is the NOTICE line saying it is not used.
FORBIDDEN = ("uns" "loth", "uns" "loth_zoo")

ALLOWED_LICENSES = {
    "Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "CC0-1.0",
}
EXEMPT_NAMES = {"LICENSE", "NOTICE"}
SKIP_DIRS = {
    ".scratch", ".venv", "venv", "__pycache__", ".pytest_cache", "build", "dist",
    ".mypy_cache", ".ruff_cache",
}
DEPENDENCY_FILES = re.compile(r"^(requirements.*\.txt|pyproject\.toml|setup\.cfg|setup\.py|constraints.*\.txt)$")
HEADER = re.compile(r"SPDX-License-Identifier:\s*(.+?)\s*(?:-->|\*/)?\s*$")
HEADER_LINES = 10


def _skip(path: Path, root: Path) -> bool:
    parts = path.relative_to(root).parts
    return any(p in SKIP_DIRS or p.endswith(".egg-info") for p in parts[:-1])


def iter_files(root: Path) -> Iterable[Path]:
    for path in sorted(root.rglob("*")):
        if path.is_file() and not _skip(path, root):
            yield path


def _is_forbidden(module: str) -> bool:
    top = module.split(".")[0].replace("-", "_").lower()
    return top in FORBIDDEN


def license_ids(expression: str) -> List[str]:
    tokens = re.split(r"[\s()]+", expression)
    ids, skip_next = [], False
    for token in tokens:
        if not token or token in ("AND", "OR"):
            continue
        if token == "WITH":
            skip_next = True  # the exception after WITH is not a license
            continue
        if skip_next:
            skip_next = False
            continue
        ids.append(token)
    return ids


def check_header(path: Path, text: str) -> List[str]:
    if path.name in EXEMPT_NAMES:
        return []
    for line in text.splitlines()[:HEADER_LINES]:
        m = HEADER.search(line)
        if m:
            bad = [i for i in license_ids(m.group(1)) if i not in ALLOWED_LICENSES]
            if bad:
                return [f"{path}: license {', '.join(bad)} is not Apache-2.0-compatible"]
            return []
    return [f"{path}: no SPDX-License-Identifier header in the first {HEADER_LINES} lines"]


def check_python_imports(path: Path, text: str) -> List[str]:
    try:
        tree = ast.parse(text, filename=str(path))
    except SyntaxError as exc:
        return [f"{path}: cannot parse ({exc.msg} at line {exc.lineno})"]
    problems = []
    for node in ast.walk(tree):
        names: List[str] = []
        if isinstance(node, ast.Import):
            names = [a.name for a in node.names]
        elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
            names = [node.module]
        elif isinstance(node, ast.Call) and node.args:
            func = node.func
            fname = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", "")
            arg = node.args[0]
            if fname in ("import_module", "__import__") and isinstance(arg, ast.Constant) and isinstance(arg.value, str):
                names = [arg.value]
        for name in names:
            if _is_forbidden(name):
                problems.append(f"{path}:{node.lineno}: imports {name} (AGPL; not allowed in allternit-tune)")
    return problems


def check_dependencies(path: Path, text: str) -> List[str]:
    problems = []
    pattern = re.compile(r"(?<![\w.-])(" + "|".join(re.escape(f).replace("_", "[-_]") for f in FORBIDDEN) + r")(?![\w.-])", re.I)
    for number, line in enumerate(text.splitlines(), 1):
        if line.lstrip().startswith("#"):
            continue
        if pattern.search(line):
            problems.append(f"{path}:{number}: lists {pattern.search(line).group(1)} as a dependency (AGPL)")
    return problems


def check(root: Path) -> List[str]:
    root = root.resolve()
    problems: List[str] = []
    for path in iter_files(root):
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            problems.append(f"{path}: binary file (cannot carry an SPDX header)")
            continue
        problems += check_header(path, text)
        if path.suffix == ".py":
            problems += check_python_imports(path, text)
        if DEPENDENCY_FILES.match(path.name):
            problems += check_dependencies(path, text)
    return problems


def main(argv: List[str]) -> int:
    root = Path(argv[0]) if argv else Path(__file__).resolve().parent.parent
    if not root.is_dir():
        print(f"spdx_gate: {root} is not a folder", file=sys.stderr)
        return 2
    problems = check(root)
    if problems:
        print(f"spdx_gate: {len(problems)} problem(s) in {root}")
        for problem in problems:
            print("  " + problem)
        return 1
    count = sum(1 for _ in iter_files(root.resolve()))
    print(f"spdx_gate: ok ({count} files checked in {root})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
