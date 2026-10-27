#!/usr/bin/env python3
"""Route a global hook to the current project's implementation."""
import json
import os
from pathlib import Path
import subprocess
import sys

HOOKS = {
    'git-discipline-gate.sh', 'steer-stop.sh', 'steer-pre-commit-gate.sh',
    'guard-main-checkout.sh', 'guard-build.sh', 'session-worktree.sh',
}
# Optional hooks: a repo whose steering predates them simply skips them.
# Each takes one mode argument.
OPTIONAL = {'tree-hook.sh': {'guard', 'checkpoint'}}


def main():
    name = Path(sys.argv[1]).name if len(sys.argv) in (2, 3) else ''
    args = sys.argv[2:]
    if name in OPTIONAL:
        if len(args) != 1 or args[0] not in OPTIONAL[name]:
            raise ValueError(f'{name} needs one of: {sorted(OPTIONAL[name])}')
    elif name not in HOOKS or args:
        raise ValueError('unknown steering hook')
    payload = json.load(sys.stdin)
    if not isinstance(payload, dict):
        raise ValueError('hook input must be a JSON object')
    cwd = Path(payload.get('cwd') or os.getcwd()).resolve()
    result = subprocess.run(
        ['git', '-C', str(cwd), 'rev-parse', '--show-toplevel'],
        capture_output=True, text=True,
    )
    if result.returncode:
        return 0  # Home directories and non-project sessions have no repo policy.
    root = Path(result.stdout.strip())
    script = root / '.steering' / 'bin' / name
    if not script.is_file() and name in OPTIONAL:
        return 0
    if not script.is_file():
        # A partially installed hook set fails closed. A checkpoint note alone
        # (no .steering/bin) is documentation, not a hook configuration.
        if (root / '.steering' / 'bin').is_dir():
            raise ValueError(f'configured project is missing hook: {script}')
        return 0  # This repository has not configured steering.

    # These legacy hooks look for checkpoints relative to payload.cwd.
    # Only lifecycle/consult hooks need the root; command guards retain the
    # actual tool directory (the build guard uses it to identify the product).
    payload['cwd'] = str(root if name in {
        'steer-stop.sh', 'steer-pre-commit-gate.sh',
        'git-discipline-gate.sh', 'session-worktree.sh',
    } else cwd)
    result = subprocess.run(
        ['bash', str(script), *args], input=json.dumps(payload), text=True, cwd=cwd,
    )
    return result.returncode if result.returncode >= 0 else 128 - result.returncode


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError) as error:
        print(f'[steering launcher] {error}', file=sys.stderr)
        sys.exit(2)
