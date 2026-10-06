import { spawn, type ChildProcess, type SpawnOptions, type StdioOptions } from "node:child_process"
import { accessSync, constants } from "node:fs"
import { isAbsolute } from "node:path"

/**
 * Shut down when the process that launched us dies.
 *
 * The desktop shell spawns `gizzi-code serve` / `gizzi-code fabric-worker`
 * with a stdin pipe it never writes to or closes, and sets
 * GIZZI_PARENT_LIFELINE=stdin. When the desktop exits for any reason —
 * clean quit, crash, force quit, SIGKILL — the kernel closes its end of the
 * pipe and our stdin sees EOF. No polling: macOS does not kill children when
 * their parent dies, and before this every such exit leaked a ~600MB server
 * reparented to launchd.
 *
 * Opt-in by env: a launchd-managed always-on daemon or a terminal `gizzi
 * serve` never sets GIZZI_PARENT_LIFELINE, so it is unaffected.
 */
export function onParentExit(callback: () => void): void {
  if (process.env.GIZZI_PARENT_LIFELINE !== "stdin") return

  let fired = false
  const fire = () => {
    if (fired) return
    fired = true
    callback()
  }
  process.stdin.on("end", fire)
  process.stdin.on("close", fire)
  process.stdin.on("error", fire)
  // Flowing mode with no data listener: nothing is buffered, EOF is delivered.
  process.stdin.resume()
}

// Twin of surfaces/allternit-desktop/src/main/process-lifeline.ts — keep the
// watcher body identical (gizzi adds a stdin-lifeline variant for Bun, below).
// The watcher sits in the child's process group (so -$$ can
// never name a reused group), blocks in read(2) on a pipe from us on fd 3
// (the child's own stdin is untouched), and on EOF (we died, or the child
// exited and we closed it) sweeps the group.
const WATCHER = `(
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
) <&3 >/dev/null 2>&1 &`

const LIFELINE_SHIM = `
${WATCHER}
exec "$@" 3<&-
`

// Same watcher, lifeline on the shim's stdin (dup'd to fd 3 for the
// watcher; the command gets /dev/null). Used when the command doesn't read
// stdin: Bun stalls later child I/O for seconds after a detached child with
// an extra stdio pipe (fd 3) exits — the mesh fallback hung ~5s, and
// mesh.test.ts (c) flaked 3–4 in 40 — while a stdin pipe does not.
const LIFELINE_SHIM_STDIN = `
exec 3<&0
${WATCHER}
exec "$@" 3<&- </dev/null
`

/**
 * spawn() for helpers gizzi owns (mesh-node, tailscaled, cloudflared) so
 * they cannot outlive gizzi, even when gizzi itself is
 * SIGKILLed or crashes and never runs its ProcessRegistry cleanup. Same pid,
 * signals, and exit semantics as a plain spawn(); always its own process group.
 */
export function spawnOwnedChild(
  command: string,
  args: readonly string[],
  options: SpawnOptions = {},
): ChildProcess {
  if (process.platform === "win32" || (isAbsolute(command) && !isExecutable(command))) {
    return spawn(command, args, options)
  }
  const [stdin, stdout, stderr] = normalizeStdio(options.stdio)
  // Commands that don't read stdin carry the lifeline on it (see
  // LIFELINE_SHIM_STDIN); the rest keep the fd-3 pipe.
  const viaStdin = stdin === "ignore"
  const proc = spawn("/bin/sh", ["-c", viaStdin ? LIFELINE_SHIM_STDIN : LIFELINE_SHIM, "gizzi-lifeline", command, ...args], {
    ...options,
    stdio: viaStdin ? ["pipe", stdout, stderr] : [stdin, stdout, stderr, "pipe"],
    detached: true,
  })
  const lifeline = (viaStdin ? proc.stdin : proc.stdio[3]) as import("node:stream").Writable | null
  lifeline?.on("error", () => {})
  proc.once("exit", () => lifeline?.destroy())
  return proc
}

function isExecutable(file: string): boolean {
  try {
    accessSync(file, constants.X_OK)
    return true
  } catch {
    return false
  }
}

type StdioEntry = Exclude<StdioOptions, string>[number]

function normalizeStdio(stdio: StdioOptions | undefined): [StdioEntry, StdioEntry, StdioEntry] {
  if (stdio === undefined) return ["pipe", "pipe", "pipe"]
  if (typeof stdio === "string") return [stdio, stdio, stdio]
  return [stdio[0], stdio[1] ?? "pipe", stdio[2] ?? "pipe"]
}

// Blocks on the lifeline (its stdin), then runs the command once gizzi's end
// closes: gizzi exited, crashed, or was SIGKILLed. Ignores the signals a
// terminal sends its process group so a closing tab can't skip the hook.
const EXIT_HOOK = `
trap '' TERM INT HUP
while read -r _; do :; done
exec "$@" </dev/null >/dev/null 2>&1
`

/**
 * Run `command args` when gizzi exits, however it exits. For resources that
 * daemonize out of gizzi's reach (a tmux server forks and calls setsid, so
 * spawnOwnedChild's process-group sweep never sees it). The hook is a
 * detached shell holding a stdin pipe from gizzi, so it doesn't keep gizzi's
 * event loop alive and survives gizzi's process group being killed.
 */
export function runOnGizziExit(command: string, args: readonly string[]): void {
  if (process.platform === "win32") return
  const hook = spawn("/bin/sh", ["-c", EXIT_HOOK, "gizzi-exit-hook", command, ...args], {
    stdio: ["pipe", "ignore", "ignore"],
    detached: true,
  })
  hook.on("error", () => {})
  hook.stdin?.on("error", () => {})
  hook.unref()
  // The pipe handle would otherwise hold the event loop open.
  ;(hook.stdin as unknown as { unref?: () => void } | null)?.unref?.()
}
