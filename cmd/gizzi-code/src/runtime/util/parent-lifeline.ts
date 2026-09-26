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
// shim identical. The watcher sits in the child's process group (so -$$ can
// never name a reused group), blocks in read(2) on a pipe from us, and on EOF
// (we died, or the child exited and we closed it) sweeps the group.
const LIFELINE_SHIM = `
exec 3<&0
(
  trap '' TERM INT HUP
  while read -r _; do :; done
  kill -TERM -$$ 2>/dev/null
  sleep 5
  kill -KILL -$$ 2>/dev/null
) <&3 >/dev/null 2>&1 &
exec "$@" </dev/null 3<&-
`

/**
 * spawn() for helpers gizzi owns (mesh-node, tailscaled, cloudflared,
 * allternit-mux) so they cannot outlive gizzi, even when gizzi itself is
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
  const [, stdout, stderr] = normalizeStdio(options.stdio)
  const proc = spawn("/bin/sh", ["-c", LIFELINE_SHIM, "gizzi-lifeline", command, ...args], {
    ...options,
    stdio: ["pipe", stdout, stderr],
    detached: true,
  })
  proc.stdin?.on("error", () => {})
  proc.once("exit", () => proc.stdin?.destroy())
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
