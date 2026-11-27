/**
 * Guards for starting a session on a computer (Start anywhere, D8). Pure, so
 * the tricky rules are testable on their own: the mode a caller may grant
 * never exceeds its own, and sessions started from sessions go at most three
 * levels deep.
 */
export const START_MODES = ["plan", "default", "auto", "bypass"] as const
export type StartMode = (typeof START_MODES)[number]

export const MAX_START_DEPTH = 3

const RANK: Record<StartMode, number> = { plan: 0, default: 1, auto: 2, bypass: 3 }

export function isStartMode(v: unknown): v is StartMode {
  return typeof v === "string" && (START_MODES as readonly string[]).includes(v)
}

/** The contract mode that matches this session's own permission mode (Ink's names). */
export function callerStartMode(permissionMode: string | undefined): StartMode {
  switch (permissionMode) {
    case "plan":
      return "plan"
    case "acceptEdits":
    case "auto":
      return "auto"
    case "bypassPermissions":
      return "bypass"
    default:
      return "default"
  }
}

/** The depth of a session started from a session at `parentDepth`. */
export function nextDepth(parentDepth: number | undefined): number {
  const d = Number.isFinite(parentDepth) && (parentDepth as number) >= 0 ? Math.floor(parentDepth as number) : 0
  return d + 1
}

export type StartGuard =
  | { ok: true; mode: StartMode; depth: number }
  | { ok: false; error: "depth_exceeded" | "mode_not_allowed"; message: string }

/**
 * `requested` defaults to the caller's own mode; a higher one is refused
 * (plan < default < auto < bypass); depth above 3 is refused.
 */
export function checkStart(args: { callerMode: StartMode; requestedMode?: StartMode; parentDepth?: number }): StartGuard {
  const depth = nextDepth(args.parentDepth)
  if (depth > MAX_START_DEPTH) {
    return { ok: false, error: "depth_exceeded", message: "Sessions started from sessions can only go three levels deep. Start the work from a top-level session instead." }
  }
  const mode = args.requestedMode ?? args.callerMode
  if (RANK[mode] > RANK[args.callerMode]) {
    return { ok: false, error: "mode_not_allowed", message: `This session runs in ${args.callerMode} mode and can't start a session in ${mode} mode. Pick ${args.callerMode} or lower.` }
  }
  return { ok: true, mode, depth }
}
