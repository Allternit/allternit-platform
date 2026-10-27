#!/usr/bin/env python3
"""Repair known Codex steering commands without altering trust or other hooks."""
import argparse
import json
from pathlib import Path
import shlex
import shutil
import sys
import time

import runpy

launcher = Path(__file__).resolve().with_name('run-hook.py')
_launcher = runpy.run_path(str(launcher))
known = _launcher['HOOKS']
optional = _launcher.get('OPTIONAL', {})


def repaired(config):
    for groups in config.get('hooks', {}).values():
        for group in groups:
            for hook in group.get('hooks', []):
                command = hook.get('command', '')
                if hook.get('type') != 'command':
                    continue
                tokens = shlex.split(command)
                # Only replace exact legacy commands or our own launcher.
                if len(tokens) in (2, 3) and tokens[0] == 'bash':
                    target, args = tokens[1], tokens[2:]
                    if target != '.steering/bin/' + Path(target).name:
                        continue
                elif len(tokens) in (3, 4) and tokens[1] == str(launcher):
                    target, args = tokens[2], tokens[3:]
                else:
                    continue
                name = Path(target).name
                if (name in known and not args) or (name in optional and len(args) == 1 and args[0] in optional[name]):
                    hook['command'] = shlex.join([
                        '/usr/bin/python3', str(launcher), '.steering/bin/' + name, *args,
                    ])
    return config


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, default=Path.home() / '.codex/hooks.json')
    parser.add_argument('--write', action='store_true')
    args = parser.parse_args()
    original = args.config.read_text()
    updated = json.dumps(repaired(json.loads(original)), indent=2) + '\n'
    if not args.write:
        print(updated, end='')
    elif updated == original:
        print('Hook commands already current.')
    else:
        backup = args.config.with_name(args.config.name + f'.backup-{time.time_ns()}')
        shutil.copy2(args.config, backup)
        temporary = args.config.with_name(args.config.name + f'.tmp-{time.time_ns()}')
        try:
            temporary.write_text(updated)
            temporary.chmod(args.config.stat().st_mode & 0o777)
            temporary.replace(args.config)
        finally:
            temporary.unlink(missing_ok=True)
        print(f'Updated {args.config}; backup: {backup}')
        print('Review changed definitions in Codex /hooks. Trust settings were not changed.')
