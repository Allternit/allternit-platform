/**
 * spawn() for long-running sidecars that must not outlive this process.
 *
 * macOS and Linux do not kill children when their parent dies, so a crashed,
 * force-quit, or relaunched desktop used to leave every sidecar running under
 * launchd (connector sidecar, office engine, voice, mesh-node, …), and each
 * relaunch stacked another copy.
 *
 * The child is started through a tiny /bin/sh shim that forks a watcher
 * blocked on a pipe from us (fd 3), then `exec`s the real command in place.
 * The pid, signals, and exit code/signal seen by the caller are the real
 * child's, exactly as with a plain spawn(). When this process dies for any
 * reason — including SIGKILL — the kernel closes the pipe, the watcher reads
 * EOF and terminates the child's process group (SIGTERM, then SIGKILL after
 * 5s). No polling: the watcher sleeps in read(2). The pipe is also closed
 * once the child exits, which sweeps any orphaned grandchildren.
 *
 * gizzi-code serve / fabric-worker do not use this: they watch the same
 * pipe in-process (GIZZI_PARENT_LIFELINE). cmd/gizzi-code has a twin of this
 * shim for the children it spawns itself.
 */

import { spawn } from 'node:child_process';
import type { ChildProcess, SpawnOptions, StdioOptions } from 'node:child_process';
import * as fs from 'fs';
import * as path from 'path';

// $1.. is the real command. fd 3 is the lifeline pipe: only the watcher
// keeps it, so the command's own stdin stays whatever the caller asked for
// (MCP stdio servers keep their protocol pipe). The watcher is a member of the child's process group, so that group id cannot
// be reused while it waits: signalling -$$ only ever reaches this sidecar's
// tree, including grandchildren a start script left behind after the
// manager killed only the leader.
const LIFELINE_SHIM = `
(
  trap '' TERM INT HUP
  while read -r _; do :; done
  # EOF doesn't always mean the parent is gone: a stray close, or the
  # parent closing its fds during quit a moment before it exits. Wait while
  # the command is alive and still our parent's child, then sweep once
  # either stops being true. Exiting here instead leaked every sidecar on a
  # normal quit. (Without ps the check fails at once and it sweeps.)
  while [ "$(ps -o ppid= -p $$ 2>/dev/null | tr -d ' ')" = "$PPID" ]; do sleep 1; done
  kill -TERM -$$ 2>/dev/null
  sleep 5
  kill -KILL -$$ 2>/dev/null
) <&3 >/dev/null 2>&1 &
exec "$@" 3<&-
`;

export function spawnSidecar(
  command: string,
  args: readonly string[],
  options: SpawnOptions = {},
): ChildProcess {
  // Windows: no /bin/sh. A missing absolute binary: keep spawn()'s native
  // ENOENT 'error' event, which managers report, instead of a shell exit 127.
  if (process.platform === 'win32' || (path.isAbsolute(command) && !isExecutable(command))) {
    return spawn(command, args, options);
  }

  const [target, targetArgs] = options.shell
    ? ['/bin/sh', ['-c', [command, ...args].join(' ')]]
    : [command, [...args]];
  const [stdin, stdout, stderr] = normalizeStdio(options.stdio);

  const proc = spawn('/bin/sh', ['-c', LIFELINE_SHIM, 'allternit-lifeline', target, ...targetArgs], {
    ...options,
    shell: false,
    stdio: [stdin, stdout, stderr, 'pipe'],
    // Always its own process group: the watcher's group kill depends on it.
    detached: true,
  });
  // Never written. Closing it after exit releases the watcher without a kill.
  const lifeline = proc.stdio[3] as import('node:stream').Duplex | null;
  lifeline?.on('error', () => {});
  proc.once('exit', () => lifeline?.destroy());
  return proc;
}

function isExecutable(file: string): boolean {
  try {
    fs.accessSync(file, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

type StdioEntry = Exclude<StdioOptions, string>[number];

function normalizeStdio(stdio: StdioOptions | undefined): [StdioEntry, StdioEntry, StdioEntry] {
  if (stdio === undefined) return ['pipe', 'pipe', 'pipe'];
  if (typeof stdio === 'string') return [stdio, stdio, stdio];
  return [stdio[0], stdio[1] ?? 'pipe', stdio[2] ?? 'pipe'];
}
