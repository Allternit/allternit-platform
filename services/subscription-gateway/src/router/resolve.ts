// §A2/§A4 — the real capability router (replaces the P1 static stub). Pure,
// deterministic, no I/O, no clocks read directly: `resolve` is a total
// function over (task, FabricSnapshot, policy) and every (manifest, account)
// pair it considers lands in exactly one place — `primary`/`fallbacks` or
// `rejected[]` with a specific reason. Manifest-driven only (HARDENING §6):
// no provider-name branching anywhere in this file.
import { randomUUID } from "node:crypto";
import type {
  Account,
  AdapterManifest,
  CapabilityRouter,
  FabricSnapshot,
  FailureClass,
  ManifestCapability,
  QuotaPool,
  RejectedRoute,
  RejectReason,
  RouteCandidate,
  RouteDecision,
  RouteLane,
  SessionHealth,
  Task,
  TaskAttempt,
} from "@allternit/subscription-fabric-contracts";
import { effectiveState } from "../pools.js";

// ---------------------------------------------------------------------------
// RoutingPolicy — gateway-local (not a contract type). Model-class and lane
// orderings are data, not code, so "reasoning ≥ X" comparisons never mention
// class names in logic.
// ---------------------------------------------------------------------------

export interface RoutingPolicy {
  // Stamped onto every RouteDecision.
  version: string;
  // §32 default deny: the metered lane is rejected with metered_not_allowed
  // unless BOTH this and task.routing.allow_metered are true.
  allow_metered: boolean;
  // Kept metered candidates carry requires_approval when est ≥ this.
  metered_approval_threshold_usd: number;
  // Estimate used when the task carries no options.est_metered_usd. Defaults
  // above the approval threshold: an unpriced metered call always asks first.
  metered_default_est_usd: number;
  // e.g. { fast: 0, standard: 1, reasoning: 2, deep: 3 }.
  model_class_rank: Record<string, number>;
  // The model class whose rank is the §A4 model_downgraded threshold: pools
  // with a live downgrade count as exhausted for tasks at or above its rank.
  reasoning_model_class: string;
  // Cost class order: subscription < local < credits < metered.
  lane_cost_rank: Record<RouteLane, number>;
  // §A6.4 default deny: local_only/confidential tasks may not use
  // ui_bridge-derived lanes unless the lane is explicitly listed here.
  sensitive_allowed_lanes: RouteLane[];
}

export const DEFAULT_ROUTING_POLICY: RoutingPolicy = {
  version: "p4-v1",
  allow_metered: false,
  metered_approval_threshold_usd: 0.5,
  metered_default_est_usd: 1.0,
  model_class_rank: { fast: 0, standard: 1, reasoning: 2, deep: 3 },
  reasoning_model_class: "reasoning",
  lane_cost_rank: { subscription: 0, local: 1, credits: 2, metered: 3 },
  sensitive_allowed_lanes: [],
};

// The contract manifest has no lane field (contract gap, see P4 Phase 1
// NOTES): the route lane is derived from the adapter interface. UI bridges
// drive a paid subscription; an official API is metered by definition. A
// gateway-local `lane` hint on the manifest object (not in the schema today —
// zod strips it at load, so only programmatic manifests can carry it) wins
// over the derivation; that is how the local/credits lanes become routable
// once adapters for them exist.
export function laneForManifest(manifest: AdapterManifest): RouteLane {
  const hint = (manifest as AdapterManifest & { lane?: RouteLane }).lane;
  if (hint === "subscription" || hint === "local" || hint === "credits" || hint === "metered") return hint;
  return manifest.interface === "official" ? "metered" : "subscription";
}

// Matches the worker's pool-key construction (worker.ts): provider:account:pool.
export function poolKeyFor(provider: string, accountId: string, poolId: string): string {
  return `${provider}:${accountId}:${poolId}`;
}

// The Task schema carries no requested_model_class field (contract gap); the
// worker already reads it from options.model_class, so the router does too.
export function requestedModelClass(task: Task): string | null {
  return typeof task.options.model_class === "string" ? task.options.model_class : null;
}

// Pool-state rank within a lane (§A4): available > estimated > unknown >
// degraded. cooling_down/exhausted never rank — they are rejections.
const POOL_STATE_RANK: Record<QuotaPool["state"], number> = {
  available: 0,
  estimated: 1,
  unknown: 2,
  degraded: 3,
  cooling_down: 4,
  exhausted: 5,
};

// §A2: health values that make a pair ineligible. `ui_drift` gets its own
// reason (circuit breaker); ready/degraded sessions are servable.
function healthRejectReason(health: SessionHealth): RejectReason | null {
  if (health === "ready" || health === "degraded") return null;
  if (health === "ui_drift") return "ui_drift";
  return "health_not_ready";
}

// §A9 — an attempt failure that poisons the (adapter, account) pair for this
// resolution maps to the informational rejection reason below. Stop classes
// never reach this: they end the task instead.
const NON_RETRYABLE_STOP_CLASSES: ReadonlySet<FailureClass> = new Set([
  "policy_denied", // router policy, not an adapter error — never re-routed
  "approval_required", // approval declined/absent — needs the human, not a hop
  "content_refused", // §A9: no auto-shopping a refusal across providers
  "submission_ambiguous", // Critical #2: never retry, never fall back
]);

function failureRejectReason(cls: FailureClass): RejectReason {
  if (cls === "quota_exhausted") return "pool_exhausted";
  if (cls === "rate_limited") return "pool_cooling_down";
  return "health_not_ready";
}

interface ScoredCandidate {
  candidate: RouteCandidate;
  laneRank: number;
  poolRank: number;
  budgetRank: number;
  preferRank?: number;
}

export interface FabricRouterOptions {
  now?: () => Date;
  idGen?: () => string;
  policy?: Partial<RoutingPolicy>;
}

export class FabricRouter implements CapabilityRouter {
  private readonly now: () => Date;
  private readonly idGen: () => string;
  private readonly policy: RoutingPolicy;

  constructor(opts: FabricRouterOptions = {}) {
    this.now = opts.now ?? (() => new Date());
    this.idGen = opts.idGen ?? randomUUID;
    this.policy = { ...DEFAULT_ROUTING_POLICY, ...opts.policy };
  }

  resolve(task: Task, snapshot: FabricSnapshot): RouteDecision {
    return this.resolveInternal(task, snapshot, []);
  }

  onAttemptFailed(
    task: Task,
    attempt: TaskAttempt,
    snapshot: FabricSnapshot
  ): RouteDecision | "stop" {
    const error = attempt.error;
    if (!error) return "stop";
    if (NON_RETRYABLE_STOP_CLASSES.has(error.class)) return "stop";
    if (!error.retryable && !error.fallback_eligible) return "stop";
    // Policy re-check per hop (§A2): a fresh resolve on the post-failure
    // snapshot, with the failed (adapter, account) pair rejected so the next
    // candidate is chosen. Hard-limit signals reach the pools via the worker;
    // the router itself stays pure.
    return this.resolveInternal(task, snapshot, [
      {
        adapter_id: attempt.adapter_id,
        account_id: attempt.account_id,
        reason: failureRejectReason(error.class),
      },
    ]);
  }

  private resolveInternal(
    task: Task,
    snapshot: FabricSnapshot,
    preRejected: RejectedRoute[]
  ): RouteDecision {
    const now = this.now();
    const policy = this.policy;
    const rejected: RejectedRoute[] = [...preRejected];
    const scored: ScoredCandidate[] = [];
    // §A2 hop re-check: pairs already rejected by the caller (e.g. the
    // just-failed pair from onAttemptFailed) are excluded from candidacy —
    // they appear in rejected[] exactly once and can never be re-elected.
    const excluded = new Set(preRejected.map((r) => `${r.adapter_id}${r.account_id ?? ""}`));

    const reasoningRank = policy.model_class_rank[policy.reasoning_model_class] ?? Number.POSITIVE_INFINITY;
    const requestedRank = (() => {
      const cls = requestedModelClass(task);
      return cls === null ? null : (policy.model_class_rank[cls] ?? null);
    })();
    const sensitivityBlocks =
      (task.constraints.sensitivity === "local_only" || task.constraints.sensitivity === "confidential");

    for (const manifest of snapshot.manifests) {
      const lane = laneForManifest(manifest);
      const cap = manifest.capabilities.find((c) => c.id === task.capability);
      const accounts = snapshot.accounts.filter((a) => a.provider === manifest.provider);
      // Subscription lanes need an account to drive; a metered adapter may
      // legitimately have none (RouteCandidate.account_id is nullable).
      const pairs: Array<Account | null> =
        accounts.length > 0 ? accounts : lane === "metered" ? [null] : [];

      for (const account of pairs) {
        if (excluded.has(`${manifest.adapter_id}${account?.account_id ?? ""}`)) continue;
        const reject = (reason: RejectReason): void => {
          rejected.push({
            adapter_id: manifest.adapter_id,
            ...(account ? { account_id: account.account_id } : {}),
            reason,
          });
        };

        if (!cap || cap.min_capability_version > task.capability_version) {
          reject("capability_not_offered");
          continue;
        }
        if (cap.status === "disabled") {
          reject("adapter_disabled");
          continue;
        }
        if (account && !account.enabled) {
          reject("account_disabled");
          continue;
        }
        if (account && cap.plans.length > 0 && account.plan !== null && !cap.plans.includes(account.plan)) {
          reject("plan_lacks_capability");
          continue;
        }
        const health: SessionHealth = account
          ? (snapshot.session_health[account.account_id] ?? account.session_health)
          : "ready";
        const healthReason = healthRejectReason(health);
        if (healthReason) {
          reject(healthReason);
          continue;
        }
        // §A6.4 — default deny: sensitive tasks never touch ui_bridge-derived
        // lanes unless the policy explicitly lists the lane. The interface
        // check keeps a ui_bridge adapter blocked even under a lane hint;
        // metered/local lanes are unaffected.
        if (
          sensitivityBlocks &&
          (lane === "subscription" || manifest.interface !== "official") &&
          !policy.sensitive_allowed_lanes.includes(lane)
        ) {
          reject("sensitivity_blocked");
          continue;
        }
        if (lane === "metered") {
          if (!policy.allow_metered || !task.routing.allow_metered) {
            reject("metered_not_allowed");
            continue;
          }
          const est = estMeteredUsd(task, policy);
          scored.push({
            candidate: {
              adapter_id: manifest.adapter_id,
              account_id: account?.account_id ?? null,
              pool_key: null,
              lane,
              est_metered_usd: est,
              requires_approval: est >= policy.metered_approval_threshold_usd,
            },
            laneRank: policy.lane_cost_rank[lane],
            poolRank: 0,
            budgetRank: 0,
          });
          continue;
        }

        // Pool-bearing lanes (subscription today): §A4 eligibility.
        const poolKey = account ? poolKeyFor(manifest.provider, account.account_id, cap.pool_id) : null;
        const pool = poolKey ? snapshot.pools.find((p) => p.pool_key === poolKey) : undefined;
        // A missing pool row reads as unknown — and unknown pools ARE eligible.
        const state = pool ? effectiveState(pool, now) : "unknown";
        if (state === "exhausted") {
          reject("pool_exhausted");
          continue;
        }
        if (state === "cooling_down") {
          reject("pool_cooling_down");
          continue;
        }
        // §A4 model_downgraded: the pool counts as exhausted for tasks at or
        // above the reasoning rank until reset_at (effectiveState has already
        // mapped an expired window back to unknown).
        if (
          state === "degraded" &&
          pool?.last_signal?.kind === "model_downgraded" &&
          requestedRank !== null &&
          requestedRank >= reasoningRank
        ) {
          reject("pool_exhausted");
          continue;
        }
        // §A4 soft signal: degraded pools still serve interactive work;
        // background tasks skip. (normal behaves like interactive in v1.)
        if (state === "degraded" && task.priority === "background") {
          reject("pool_degraded");
          continue;
        }
        // §A4 local soft budget: breach drops rank, never excludes.
        const budgetBreached =
          pool !== undefined && pool.local_budget !== null && pool.local_used_in_window >= pool.local_budget;
        scored.push({
          candidate: {
            adapter_id: manifest.adapter_id,
            account_id: account?.account_id ?? null,
            pool_key: poolKey,
            lane,
            est_metered_usd: 0,
            requires_approval: false,
          },
          laneRank: policy.lane_cost_rank[lane],
          poolRank: POOL_STATE_RANK[state],
          budgetRank: budgetBreached ? 1 : 0,
          preferRank: account?.preferred ? 0 : 1,
        });
      }
    }

    // Deterministic order: lane cost class → pool state rank → budget-breach
    // demotion → the provider's preferred account → stable tiebreak
    // (adapter_id, account_id). No randomness.
    scored.sort((a, b) =>
      a.laneRank - b.laneRank ||
      a.poolRank - b.poolRank ||
      a.budgetRank - b.budgetRank ||
      (a.preferRank ?? 1) - (b.preferRank ?? 1) ||
      a.candidate.adapter_id.localeCompare(b.candidate.adapter_id) ||
      (a.candidate.account_id ?? "").localeCompare(b.candidate.account_id ?? "")
    );

    const primary = scored[0]?.candidate ?? null;
    const fallbacks = scored.slice(1).map((s) => s.candidate);
    return {
      decision_id: this.idGen(),
      primary,
      fallbacks,
      rejected,
      policy_version: policy.version,
      explain: explainDecision(task, primary, rejected),
    };
  }
}

function estMeteredUsd(task: Task, policy: RoutingPolicy): number {
  const fromTask = task.options.est_metered_usd;
  return typeof fromTask === "number" && Number.isFinite(fromTask)
    ? fromTask
    : policy.metered_default_est_usd;
}

// Most-frequent rejection reason; ties break by first appearance (stable).
function topRejectionReason(rejected: RejectedRoute[]): RejectReason | null {
  let best: RejectReason | null = null;
  let bestCount = 0;
  const counts = new Map<RejectReason, number>();
  for (const r of rejected) {
    const n = (counts.get(r.reason) ?? 0) + 1;
    counts.set(r.reason, n);
    if (n > bestCount) {
      best = r.reason;
      bestCount = n;
    }
  }
  return best;
}

function explainDecision(task: Task, primary: RouteCandidate | null, rejected: RejectedRoute[]): string {
  const top = topRejectionReason(rejected);
  const suffix = top ? `top rejection: ${top} (${rejected.filter((r) => r.reason === top).length} of ${rejected.length})` : "no rejections";
  if (!primary) {
    return `no eligible route for ${task.capability}; ${suffix}`;
  }
  const where = primary.account_id ? `${primary.adapter_id}/${primary.account_id}` : primary.adapter_id;
  const approval = primary.requires_approval ? ", requires approval" : "";
  return `routed ${task.capability} to ${where} (${primary.lane} lane${approval}); ${suffix}`;
}

// ---------------------------------------------------------------------------
// /v1/capabilities support — per-entitlement pool state without a task
// context (sensitivity/priority/model-class rules only apply at route time).
// ---------------------------------------------------------------------------

export interface PoolEntitlement {
  account_id: string;
  pool_key: string;
  pool_state: QuotaPool["state"];
  available: boolean;
  reason_unavailable?: RejectReason;
}

export function entitlementForAccount(
  manifest: AdapterManifest,
  cap: ManifestCapability,
  account: Account,
  pool: QuotaPool | undefined,
  health: SessionHealth,
  now: Date
): PoolEntitlement {
  const poolKey = poolKeyFor(manifest.provider, account.account_id, cap.pool_id);
  const state = pool ? effectiveState(pool, now) : "unknown";
  const base = { account_id: account.account_id, pool_key: poolKey, pool_state: state };
  const deny = (reason: RejectReason): PoolEntitlement => ({ ...base, available: false, reason_unavailable: reason });
  if (!account.enabled) return deny("account_disabled");
  if (cap.status === "disabled") return deny("adapter_disabled");
  if (cap.plans.length > 0 && account.plan !== null && !cap.plans.includes(account.plan)) {
    return deny("plan_lacks_capability");
  }
  const healthReason = healthRejectReason(health);
  if (healthReason) return deny(healthReason);
  if (state === "exhausted") return deny("pool_exhausted");
  if (state === "cooling_down") return deny("pool_cooling_down");
  return { ...base, available: true };
}
