/**
 * A cloud computer has no one at its screen to restart the app, so a staged
 * runtime package would wait there forever (it applies on the next launch).
 * While one is staged, check every minute whether Gizzi is running any
 * session; after two idle checks in a row, restart to apply it, the same
 * restart as Install update.
 */

export const IDLE_CHECK_EVERY_MS = 60_000;
export const IDLE_CHECKS_BEFORE_APPLY = 2;

/** Gizzi's `/v1/session/status` lists sessions that aren't idle; none (or all idle) means nothing is running. */
export function gizziSessionsIdle(statuses: unknown): boolean {
  if (!statuses || typeof statuses !== 'object' || Array.isArray(statuses)) return false;
  return Object.values(statuses as Record<string, unknown>).every(
    (s) => Boolean(s) && typeof s === 'object' && (s as { type?: unknown }).type === 'idle',
  );
}

export interface IdleApplyDeps {
  hasPending: () => boolean;
  /** Gizzi's session statuses; throws when Gizzi doesn't answer (treated as busy). */
  fetchStatuses: () => Promise<unknown>;
  apply: () => void | Promise<void>;
  everyMs?: number;
  idleChecks?: number;
  setTimer?: (fn: () => void, ms: number) => { unref?: () => void } | unknown;
}

/** Starts the check loop; returns a stop function. */
export function startIdleApply(deps: IdleApplyDeps): () => void {
  const everyMs = deps.everyMs ?? IDLE_CHECK_EVERY_MS;
  const needed = deps.idleChecks ?? IDLE_CHECKS_BEFORE_APPLY;
  const setTimer = deps.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  let streak = 0;
  let stopped = false;

  const tick = async () => {
    if (stopped) return;
    if (!deps.hasPending()) {
      streak = 0;
    } else {
      let idle = false;
      try {
        idle = gizziSessionsIdle(await deps.fetchStatuses());
      } catch {
        idle = false;
      }
      streak = idle ? streak + 1 : 0;
      if (streak >= needed) {
        stopped = true;
        await deps.apply();
        return;
      }
    }
    schedule();
  };
  const schedule = () => {
    const handle = setTimer(() => void tick(), everyMs) as { unref?: () => void } | undefined;
    handle?.unref?.();
  };

  schedule();
  return () => {
    stopped = true;
  };
}
