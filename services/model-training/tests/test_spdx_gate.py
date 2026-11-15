# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""tools/spdx_gate.py: passes on this package, catches what it must catch."""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GATE = ROOT / "tools" / "spdx_gate.py"
BANNED = "uns" + "loth"  # split so a repo-wide text search stays clean
HEADER = "# SPDX-License-Identifier: Apache-2.0\n"


def run_gate(root: Path) -> subprocess.CompletedProcess:
    return subprocess.run([sys.executable, str(GATE), str(root)], capture_output=True, text=True)


def test_gate_passes_on_this_package():
    result = run_gate(ROOT)
    assert result.returncode == 0, result.stdout


def test_gate_catches_planted_import(tmp_path):
    (tmp_path / "ok.py").write_text(HEADER + "import json\n")
    (tmp_path / "trainer.py").write_text(HEADER + f"def go():\n    from {BANNED} import FastLanguageModel\n")
    (tmp_path / "zoo.py").write_text(HEADER + f"import importlib\nimportlib.import_module('{BANNED}_zoo')\n")
    result = run_gate(tmp_path)
    assert result.returncode == 1
    assert "trainer.py:3: imports" in result.stdout
    assert f"zoo.py:3: imports {BANNED}_zoo" in result.stdout
    assert "ok.py" not in result.stdout


def test_gate_catches_dependency_and_headers(tmp_path):
    (tmp_path / "requirements.txt").write_text(HEADER + f"torch>=2\n{BANNED}>=2024.12\n")
    (tmp_path / "noheader.py").write_text("x = 1\n")
    (tmp_path / "gpl.py").write_text("# SPDX-License-Identifier: GPL-3.0-only\n")
    (tmp_path / "dual.py").write_text("# SPDX-License-Identifier: (Apache-2.0 OR MIT)\n")
    (tmp_path / "LICENSE").write_text("Apache License\n")
    result = run_gate(tmp_path)
    assert result.returncode == 1
    out = result.stdout
    assert "requirements.txt:3: lists" in out
    assert "noheader.py: no SPDX-License-Identifier header" in out
    assert "gpl.py: license GPL-3.0-only is not Apache-2.0-compatible" in out
    assert "dual.py" not in out and "LICENSE" not in out
