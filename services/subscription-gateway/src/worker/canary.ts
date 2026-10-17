// Lane health canary: every few minutes, re-probe each connected account's
// resident lane with the same free, non-spending probe POST /v1/accounts/:id/connect
// uses (nothing is sent, clicked or solved). A pass feeds recordAdapterProbePassed so
// an open drift breaker closes; a fail leaves the account's session_health red for
// GET /v1/lanes. An account a task holds is skipped (the task is the live proof), and
// a lane with no resident browser is left closed: the canary never launches Chrome.
import { recordAdapterProbePassed } from "../breakers.js";
import type { Db } from "../store/db.js";
import { accountHasActiveTasks, insertCanaryRun, listAccounts, pruneCanaryRuns } from "../store/queries.js";
import type { WorkerPool } from "./pool.js";

export const CANARY_INTERVAL_MS = 3 * 60_000;
const KEEP_MS = 7 * 24 * 3_600_000;

export interface CanaryDeps {
  db: Db;
  pool: Pick<WorkerPool, "probeResident">;
  now?: () => Date;
  logger?: (line: string) => void;
}

export interface CanaryTickResult {
  probed: number;
  skipped: number;
}

export async function canaryTick(deps: CanaryDeps): Promise<CanaryTickResult> {
  const result: CanaryTickResult = { probed: 0, skipped: 0 };
  const now = deps.now ?? (() => new Date());
  for (const account of listAccounts(deps.db)) {
    if (!account.enabled || accountHasActiveTasks(deps.db, account.account_id)) {
      result.skipped += 1;
      continue;
    }
    const started = now();
    let probed;
    try {
      probed = await deps.pool.probeResident({ provider: account.provider, account_id: account.account_id });
    } catch (err) {
      deps.logger?.(`subscription-gateway: canary for ${account.account_id} threw: ${err instanceof Error ? err.message : String(err)}`);
      result.skipped += 1;
      continue;
    }
    if (!probed) {
      result.skipped += 1;
      continue;
    }
    const ended = now();
    const pass = probed.health === "ready";
    insertCanaryRun(deps.db, {
      account_id: account.account_id,
      adapter_id: probed.adapter_id,
      kind: "probe",
      started_at: started.toISOString(),
      ended_at: ended.toISOString(),
      outcome: pass ? "pass" : "fail",
      health: probed.health,
      latency_ms: ended.getTime() - started.getTime(),
      error: pass ? null : probed.health,
      evidence_ref: null,
    });
    if (pass) recordAdapterProbePassed(deps.db, probed.adapter_id, probed.adapter_version, ended);
    result.probed += 1;
  }
  pruneCanaryRuns(deps.db, new Date(now().getTime() - KEEP_MS).toISOString());
  return result;
}

/** Starts the interval; returns stop(). intervalMs <= 0 disables it. */
export function startCanary(deps: CanaryDeps & { intervalMs?: number }): () => void {
  const intervalMs = deps.intervalMs ?? CANARY_INTERVAL_MS;
  if (intervalMs <= 0) return () => {};
  let busy = false;
  const timer = setInterval(() => {
    if (busy) return;
    busy = true;
    void canaryTick(deps)
      .catch((err) => deps.logger?.(`subscription-gateway: canary tick failed: ${err instanceof Error ? err.message : String(err)}`))
      .finally(() => {
        busy = false;
      });
  }, intervalMs);
  timer.unref?.();
  return () => clearInterval(timer);
}
