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
