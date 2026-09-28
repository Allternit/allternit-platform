// §A2 — router wire-in. Policy is re-checked on every hop: a task is resolved
// against a fresh snapshot when it enters the queue (resolveForNewTask, used
// by POST /v1/tasks), and re-resolved after a failed attempt
// (requeueAfterFailure, used by the worker boundary). The scheduler itself
// stays a pure FIFO — the resolved (provider, account_id) pair keys its lane
// via the task's routing fields, so no scheduler redesign.
import type { CapabilityRouter, FabricSnapshot, RouteDecision, Task } from "@allternit/subscription-fabric-contracts";
import type { AdapterRegistry } from "../adapters/registry.js";
import type { Scheduler } from "../queue/scheduler.js";
import type { Db } from "../store/db.js";
import { getTask, recordRouteRejections, updateTaskRoutingDecision, updateTaskStatus } from "../store/queries.js";
import { buildSnapshot } from "./snapshot.js";

export interface DispatchDeps {
  db: Db;
  registry: AdapterRegistry;
  router: CapabilityRouter;
  scheduler: Scheduler;
}

// True when the task left the account pick to the fabric: auto mode with no
// account pin. A provider-only pin still resolves (within that provider) —
// otherwise it sat in the (provider, UNROUTED) lane no worker drains.
// prefer/force pins keep their existing queue behavior.
export function needsResolution(task: Task): boolean {
  return task.routing.mode === "auto" && task.routing.account_id === undefined;
}

// The routing view for a task: a provider-pinned task only sees accounts of
// that provider, so neither the first pick nor a failure re-route leaves it.
function snapshotFor(deps: Pick<DispatchDeps, "db" | "registry">, task: Task): FabricSnapshot {
  const snapshot = buildSnapshot(deps.db, deps.registry);
  const pinned = task.routing.provider_pinned ? task.routing.provider : undefined;
  if (pinned === undefined) return snapshot;
  return { ...snapshot, accounts: snapshot.accounts.filter((a) => a.provider === pinned) };
}

// Resolve before insert/enqueue. Returns the task with route_decision set
// and — when a primary exists — routing pinned to the primary's
// (provider, account_id), which places it in that worker's scheduler lane.
export function resolveForNewTask(
  deps: Pick<DispatchDeps, "db" | "registry" | "router">,
  task: Task
): { task: Task; decision: RouteDecision } {
  if (task.routing.provider !== undefined && !task.routing.provider_pinned) {
    task = { ...task, routing: { ...task.routing, provider_pinned: true } };
  }
  const decision = deps.router.resolve(task, snapshotFor(deps, task));
  let routed = task;
  if (decision.primary?.account_id) {
    const manifest = deps.registry.byId(decision.primary.adapter_id)?.manifest;
    if (manifest) {
      routed = {
        ...task,
        routing: {
          ...task.routing,
          provider: manifest.provider,
          account_id: decision.primary.account_id,
        },
      };
    }
  }
  return { task: { ...routed, route_decision: decision }, decision };
}

// §A2 hop re-check at the worker boundary: the attempt's failure class decides
// stop vs re-route. A fresh decision is persisted and the task moves to the
// new primary's lane; with no eligible route it stays failed with the
// rejected[] list recording why. Returns null when the task vanished.
export function requeueAfterFailure(deps: DispatchDeps, taskId: string): RouteDecision | "stop" | null {
  const task = getTask(deps.db, taskId);
  if (!task) return null;
  const attempt = task.attempts[task.attempts.length - 1];
  if (!attempt) return "stop";
  const outcome = deps.router.onAttemptFailed(task, attempt, snapshotFor(deps, task));
  if (outcome === "stop") return "stop";

  let routing = task.routing;
  if (outcome.primary?.account_id) {
    const manifest = deps.registry.byId(outcome.primary.adapter_id)?.manifest;
    if (manifest) {
      routing = { ...routing, provider: manifest.provider, account_id: outcome.primary.account_id };
    }
  } else {
    // No eligible route: the task stays failed (§A2 primary null → needs_user
    // or failed) with rejected[] recording why — nothing to re-enqueue.
    updateTaskRoutingDecision(deps.db, taskId, routing, outcome);
    recordRouteRejections(deps.db, taskId, outcome);
    return outcome;
  }
  updateTaskRoutingDecision(deps.db, taskId, routing, outcome);
  recordRouteRejections(deps.db, taskId, outcome);
  // The worker marked the task failed; a fresh primary puts it back in line.
  updateTaskStatus(deps.db, taskId, "queued");
  deps.scheduler.remove(taskId);
  deps.scheduler.enqueue({ ...task, routing, route_decision: outcome });
  return outcome;
}
