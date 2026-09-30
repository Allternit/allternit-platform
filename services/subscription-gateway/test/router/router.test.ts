// §A2/§A4 — FabricRouter table tests (plan verify-list scenarios 1, 2, 4–9).
// Pure: fixed clock, fixed decision ids, no I/O. Provider-agnostic fixtures.
import { describe, expect, it } from "vitest";
import type {
  Account,
  FailureClass,
  QuotaPool,
  RejectReason,
  RouteDecision,
  Task,
  TaskAttempt,
  TaskError,
} from "@allternit/subscription-fabric-contracts";
import { FabricRouter, laneForManifest, type RoutingPolicy } from "../../src/router/resolve.js";
import { sampleTask } from "../helpers.js";
import { makeAccount, makeManifest, makePool, makeSignal, makeSnapshot, NOW, POOL_ID } from "./fixtures.js";

const HOUR = 3_600_000;

function router(overrides: { now?: Date; policy?: Partial<RoutingPolicy> } = {}): FabricRouter {
  let seq = 0;
  return new FabricRouter({
    now: () => overrides.now ?? NOW,
    idGen: () => `dec-${++seq}`,
    policy: overrides.policy,
  });
}

function task(overrides: Partial<Task> = {}): Task {
  return sampleTask({
    options: {},
    routing: {
      mode: "auto",
      allow_fallback: true,
      allow_metered: true,
      allow_thread_migration: false,
    },
    constraints: { sensitivity: "internal", deadline_at: null, max_metered_usd: null, required_export_format: null },
    ...overrides,
  });
}

// One provider+account+pool per adapter, named by `tag`.
function trio(
  tag: string,
  poolOverrides: Partial<QuotaPool> = {},
  manifestOverrides: Parameters<typeof makeManifest>[0] = {},
  accountOverrides: Partial<Account> = {}
) {
  const provider = `prov-${tag}` as Account["provider"];
  const manifest = makeManifest({ adapter_id: `adapter-${tag}`, provider, ...manifestOverrides });
  const account = makeAccount({ account_id: `acct-${tag}`, provider, ...accountOverrides });
  const pool = makePool({ pool_key: `prov-${tag}:acct-${tag}:${POOL_ID}`, ...poolOverrides });
  return { manifest, account, pool };
}

function reasons(decision: RouteDecision, adapterId: string): RejectReason[] {
  return decision.rejected.filter((r) => r.adapter_id === adapterId).map((r) => r.reason);
}

describe("scenario 1 — unknown-pool ranking (§A4)", () => {
  const a = trio("a", { state: "available" });
  const b = trio("b", { state: "estimated" });
  const c = trio("c", { state: "unknown" });
  const metered = makeManifest({ adapter_id: "adapter-m", provider: "prov-m" as Account["provider"], interface: "official" });
  const snapshot = makeSnapshot({
    accounts: [a.account, b.account, c.account],
    manifests: [a.manifest, b.manifest, c.manifest, metered],
    pools: [a.pool, b.pool, c.pool],
  });

  it("available > estimated > unknown on the subscription lane, all above metered", () => {
    const decision = router({ policy: { allow_metered: true } }).resolve(task(), snapshot);
    expect(decision.primary?.adapter_id).toBe("adapter-a");
    expect(decision.fallbacks.map((f) => f.adapter_id)).toEqual(["adapter-b", "adapter-c", "adapter-m"]);
    const meteredCandidate = decision.fallbacks.find((f) => f.adapter_id === "adapter-m");
    expect(meteredCandidate?.lane).toBe("metered");
    expect(meteredCandidate?.account_id).toBeNull();
    expect(meteredCandidate?.requires_approval).toBe(true); // default est ≥ threshold
  });

  it("an unknown pool still beats the metered lane when it is the only subscription candidate", () => {
    const decision = router({ policy: { allow_metered: true } }).resolve(
      task(),
      makeSnapshot({ accounts: [c.account], manifests: [c.manifest, metered], pools: [c.pool] })
    );
    expect(decision.primary?.adapter_id).toBe("adapter-c");
    expect(decision.fallbacks.map((f) => f.adapter_id)).toEqual(["adapter-m"]);
  });

  it("metered is rejected with metered_not_allowed when policy or task disallows it", () => {
    const byPolicy = router().resolve(task(), makeSnapshot({ accounts: [c.account], manifests: [c.manifest, metered], pools: [c.pool] }));
    expect(reasons(byPolicy, "adapter-m")).toEqual(["metered_not_allowed"]);

    const byTask = router({ policy: { allow_metered: true } }).resolve(
      task({ routing: { mode: "auto", allow_fallback: true, allow_metered: false, allow_thread_migration: false } }),
      makeSnapshot({ accounts: [c.account], manifests: [c.manifest, metered], pools: [c.pool] })
    );
    expect(reasons(byTask, "adapter-m")).toEqual(["metered_not_allowed"]);
  });
});

describe("scenario 2 — degraded pool: priority decides (§A4)", () => {
  const d = trio("d", { state: "degraded", last_signal: makeSignal({ kind: "limit_banner" }) });
  const snapshot = makeSnapshot({ accounts: [d.account], manifests: [d.manifest], pools: [d.pool] });

  it.each([
    ["interactive", true],
    ["normal", true], // v1 decision: normal behaves like interactive (see NOTES)
    ["background", false],
  ] as Array<[Task["priority"], boolean]>)("%s priority routable=%s", (priority, routable) => {
    const decision = router().resolve(task({ priority }), snapshot);
    if (routable) {
      expect(decision.primary?.adapter_id).toBe("adapter-d");
      expect(reasons(decision, "adapter-d")).toEqual([]);
    } else {
      expect(decision.primary).toBeNull();
      expect(reasons(decision, "adapter-d")).toEqual(["pool_degraded"]);
    }
  });
});

describe("scenario 4 — circuit breaker consumption (resolve side; state machine in breakers.test.ts)", () => {
  const e = trio("e", { state: "available" });

  it("session_health ui_drift → ui_drift rejection; probe-passed (ready) → eligible again", () => {
    const drifting = router().resolve(
      task(),
      makeSnapshot({
        accounts: [e.account],
        manifests: [e.manifest],
        pools: [e.pool],
        session_health: { [e.account.account_id]: "ui_drift" },
      })
    );
    expect(drifting.primary).toBeNull();
    expect(reasons(drifting, "adapter-e")).toEqual(["ui_drift"]);

    const recovered = router().resolve(
      task(),
      makeSnapshot({
        accounts: [e.account],
        manifests: [e.manifest],
        pools: [e.pool],
        session_health: { [e.account.account_id]: "ready" },
      })
    );
    expect(recovered.primary?.adapter_id).toBe("adapter-e");
  });

  it.each(["auth_required", "challenge_presented", "account_restricted", "provider_down", "profile_locked"] as const)(
    "session_health %s → health_not_ready",
    (health) => {
      const decision = router().resolve(
        task(),
        makeSnapshot({
          accounts: [e.account],
          manifests: [e.manifest],
          pools: [e.pool],
          session_health: { [e.account.account_id]: health },
        })
      );
      expect(reasons(decision, "adapter-e")).toEqual(["health_not_ready"]);
    }
  );
});

describe("scenario 5 — sensitivity exclusion (§A6.4 default deny)", () => {
  const sub = trio("s", { state: "available" });
  const metered = makeManifest({ adapter_id: "adapter-m", provider: "prov-m" as Account["provider"], interface: "official" });
  const local = makeManifest({ adapter_id: "adapter-l", provider: "prov-l" as Account["provider"], interface: "official", lane: "local" });
  const localAcct = makeAccount({ account_id: "acct-l", provider: "prov-l" as Account["provider"] });
  const snapshot = makeSnapshot({
    accounts: [sub.account, localAcct],
    manifests: [sub.manifest, metered, local],
    pools: [sub.pool],
  });

  it.each(["confidential", "local_only"] as const)(
    "%s task: subscription lane blocked, fallbacks carry no ineligible lane, metered/local eligible",
    (sensitivity) => {
      const decision = router({ policy: { allow_metered: true } }).resolve(
        task({ constraints: { sensitivity, deadline_at: null, max_metered_usd: null, required_export_format: null } }),
        snapshot
      );
      expect(reasons(decision, "adapter-s")).toEqual(["sensitivity_blocked"]);
      const routed = [decision.primary, ...decision.fallbacks].filter((c) => c !== null);
      expect(routed.map((c) => c.adapter_id).sort()).toEqual(["adapter-l", "adapter-m"]);
      expect(routed.every((c) => c.lane !== "subscription")).toBe(true);
      // §A2: fallbacks are pre-filtered — no sensitivity-ineligible lane appears.
      expect(decision.fallbacks.some((c) => reasons(decision, c.adapter_id).includes("sensitivity_blocked"))).toBe(false);
    }
  );

  it("internal sensitivity leaves the subscription lane eligible", () => {
    const decision = router().resolve(task(), makeSnapshot({ accounts: [sub.account], manifests: [sub.manifest], pools: [sub.pool] }));
    expect(decision.primary?.adapter_id).toBe("adapter-s");
  });

  it("policy allow-list re-enables a lane for sensitive tasks", () => {
    const decision = router({ policy: { sensitive_allowed_lanes: ["subscription"] } }).resolve(
      task({ constraints: { sensitivity: "confidential", deadline_at: null, max_metered_usd: null, required_export_format: null } }),
      makeSnapshot({ accounts: [sub.account], manifests: [sub.manifest], pools: [sub.pool] })
    );
    expect(decision.primary?.adapter_id).toBe("adapter-s");
  });
});

describe("scenario 6 — rejected[] completeness + explain", () => {
  const missing = trio("m1", {}, { capabilities: [] }); // capability_not_offered
  const disabledCap = trio("m2", {}, {
    capabilities: [{ ...makeManifest().capabilities[0], status: "disabled" as const }],
  });
  const disabledAcct = trio("m3", { state: "available" }, {}, { enabled: false });
  const exhausted1 = trio("m4", { state: "exhausted" });
  const cooling = trio("m5", { state: "cooling_down", cooldown_until: new Date(NOW.getTime() + HOUR).toISOString() });
  const healthy = trio("m6", { state: "available" });
  const exhausted2 = trio("m7", { state: "exhausted" });
  const all = [missing, disabledCap, disabledAcct, exhausted1, cooling, healthy, exhausted2];
  const snapshot = makeSnapshot({
    accounts: all.map((t) => t.account),
    manifests: all.map((t) => t.manifest),
    pools: all.map((t) => t.pool),
  });

  it("every non-routed pair appears with its specific reason; explain names the top one", () => {
    const decision = router().resolve(task(), snapshot);
    expect(decision.primary?.adapter_id).toBe("adapter-m6");
    const byAdapter = Object.fromEntries(decision.rejected.map((r) => [r.adapter_id, r.reason]));
    expect(byAdapter).toEqual({
      "adapter-m1": "capability_not_offered",
      "adapter-m2": "adapter_disabled",
      "adapter-m3": "account_disabled",
      "adapter-m4": "pool_exhausted",
      "adapter-m5": "pool_cooling_down",
      "adapter-m7": "pool_exhausted",
    });
    // Exactly one place per considered pair: 7 pairs = 6 rejected + 1 primary.
    expect(decision.rejected.length + (decision.primary ? 1 : 0) + decision.fallbacks.length).toBe(7);
    expect(decision.explain).toContain("adapter-m6");
    expect(decision.explain).toContain("pool_exhausted");
    expect(decision.explain).toContain("2 of 6");
    expect(decision.policy_version).toBe("p4-v1");
  });

  it("no-route explain names the top rejection reason", () => {
    const decision = router().resolve(
      task(),
      makeSnapshot({ accounts: [exhausted1.account], manifests: [exhausted1.manifest], pools: [exhausted1.pool] })
    );
    expect(decision.primary).toBeNull();
    expect(decision.explain).toContain("no eligible route");
    expect(decision.explain).toContain("pool_exhausted");
  });
});

describe("scenario 7 — model_downgraded pool vs requested model class (§A4)", () => {
  const downgraded = trio("g", {
    state: "degraded",
    last_signal: makeSignal({ kind: "model_downgraded" }),
    reset_at: new Date(NOW.getTime() + 3 * HOUR).toISOString(),
    reset_at_source: "inferred",
  });
  const snapshot = makeSnapshot({ accounts: [downgraded.account], manifests: [downgraded.manifest], pools: [downgraded.pool] });
  const modelTask = (cls: string) => task({ priority: "interactive", options: { model_class: cls } });

  it("reasoning-class task treats the pool as exhausted; below-reasoning still routes", () => {
    for (const cls of ["reasoning", "deep"]) {
      const decision = router().resolve(modelTask(cls), snapshot);
      expect(decision.primary).toBeNull();
      expect(reasons(decision, "adapter-g")).toEqual(["pool_exhausted"]);
    }
    for (const cls of ["standard", "fast"]) {
      const decision = router().resolve(modelTask(cls), snapshot);
      expect(decision.primary?.adapter_id).toBe("adapter-g");
    }
  });

  it("after reset_at the pool reads unknown and reasoning tasks route again", () => {
    const decision = router({ now: new Date(NOW.getTime() + 4 * HOUR) }).resolve(modelTask("reasoning"), snapshot);
    expect(decision.primary?.adapter_id).toBe("adapter-g");
  });
});

describe("scenario 8 — local_budget breach demotes, never excludes (§A4)", () => {
  const breached = trio("h1", { state: "available", local_budget: 5, local_used_in_window: 5 });
  const fresh = trio("h2", { state: "available", local_budget: 5, local_used_in_window: 1 });

  it("breached pool loses to a non-breached pool in the same state", () => {
    const decision = router().resolve(
      task(),
      makeSnapshot({
        accounts: [breached.account, fresh.account],
        manifests: [breached.manifest, fresh.manifest],
        pools: [breached.pool, fresh.pool],
      })
    );
    expect(decision.primary?.adapter_id).toBe("adapter-h2");
    expect(decision.fallbacks.map((f) => f.adapter_id)).toEqual(["adapter-h1"]);
  });

  it("breached pool is still routable when it is the only candidate", () => {
    const decision = router().resolve(
      task(),
      makeSnapshot({ accounts: [breached.account], manifests: [breached.manifest], pools: [breached.pool] })
    );
    expect(decision.primary?.adapter_id).toBe("adapter-h1");
  });
});

describe("scenario 9 — onAttemptFailed: re-route vs stop (§A2 hop re-check)", () => {
  const first = trio("f1", { state: "available" });
  const second = trio("f2", { state: "available" });
  const snapshot = makeSnapshot({
    accounts: [first.account, second.account],
    manifests: [first.manifest, second.manifest],
    pools: [first.pool, second.pool],
  });

  function makeAttempt(err: TaskError | null): TaskAttempt {
    return {
      attempt_no: 1,
      adapter_id: "adapter-f1",
      adapter_version: "1.0.0",
      account_id: "acct-f1",
      pool_key: `prov-f1:acct-f1:${POOL_ID}`,
      submission_state: "acknowledged",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: NOW.toISOString(),
      ended_at: NOW.toISOString(),
      outcome: "failed",
      error: err,
    };
  }

  function makeError(cls: FailureClass, retryable: boolean, fallback: boolean): TaskError {
    return {
      class: cls,
      scope: "task",
      retryable,
      fallback_eligible: fallback,
      cooldown_s: null,
      user_action: null,
      detail: "boom",
      evidence_ref: null,
    };
  }

  it("retryable failure re-resolves to the next candidate and rejects the failed pair", () => {
    const outcome = router().onAttemptFailed(task(), makeAttempt(makeError("network_error", true, true)), snapshot);
    expect(outcome).not.toBe("stop");
    const decision = outcome as RouteDecision;
    expect(decision.primary?.adapter_id).toBe("adapter-f2");
    expect(reasons(decision, "adapter-f1")).toEqual(["health_not_ready"]);
  });

  it("quota_exhausted failure maps the failed pair to pool_exhausted", () => {
    const outcome = router().onAttemptFailed(task(), makeAttempt(makeError("quota_exhausted", true, true)), snapshot);
    const decision = outcome as RouteDecision;
    expect(decision.primary?.adapter_id).toBe("adapter-f2");
    expect(reasons(decision, "adapter-f1")).toEqual(["pool_exhausted"]);
  });

  it.each([
    ["policy_denied", false, false],
    ["approval_required", false, false],
    ["content_refused", false, false],
    ["submission_ambiguous", false, false],
    ["user_intervention_required", false, false], // not retryable, not fallback-eligible
  ] as Array<[FailureClass, boolean, boolean]>)("%s → stop", (cls, retryable, fallback) => {
    expect(router().onAttemptFailed(task(), makeAttempt(makeError(cls, retryable, fallback)), snapshot)).toBe("stop");
  });

  it("no error on the attempt → stop", () => {
    expect(router().onAttemptFailed(task(), makeAttempt(null), snapshot)).toBe("stop");
  });
});

describe("lane derivation (contract gap — manifest has no lane field)", () => {
  it("ui_bridge interfaces → subscription; official → metered; hint wins", () => {
    expect(laneForManifest(makeManifest({ interface: "ui_bridge_web" }))).toBe("subscription");
    expect(laneForManifest(makeManifest({ interface: "ui_bridge_desktop" }))).toBe("subscription");
    expect(laneForManifest(makeManifest({ interface: "official" }))).toBe("metered");
    expect(laneForManifest(makeManifest({ interface: "official", lane: "local" }))).toBe("local");
  });

  it("resolve is deterministic — same inputs, identical decision", () => {
    const a = trio("z1", { state: "available" });
    const b = trio("z2", { state: "available" });
    const snapshot = makeSnapshot({
      accounts: [a.account, b.account],
      manifests: [a.manifest, b.manifest],
      pools: [a.pool, b.pool],
    });
    let seq = 0;
    const r = new FabricRouter({ now: () => NOW, idGen: () => `d-${++seq}` });
    const one = r.resolve(task(), snapshot);
    seq = 0;
    const two = r.resolve(task(), snapshot);
    expect(two).toEqual(one);
    // stable tiebreak: adapter_id ascending
    expect(one.primary?.adapter_id).toBe("adapter-z1");
  });
});

describe("two logins of one subscription (Eoj: switch between them)", () => {
  const provider = "prov-two" as Account["provider"];
  const manifest = makeManifest({ adapter_id: "adapter-two", provider });
  const first = makeAccount({ account_id: "acct-1", provider });
  const second = makeAccount({ account_id: "acct-2", provider, preferred: true });
  const pools = (s1: QuotaPool["state"], s2: QuotaPool["state"]) => [
    makePool({ pool_key: `${provider}:acct-1:${POOL_ID}`, state: s1 }),
    makePool({ pool_key: `${provider}:acct-2:${POOL_ID}`, state: s2 }),
  ];

  it("the preferred login goes first when both are equally healthy", () => {
    const snap = makeSnapshot({ accounts: [first, second], manifests: [manifest], pools: pools("available", "available") });
    const d = router().resolve(task(), snap);
    expect(d.primary?.account_id).toBe("acct-2");
    expect(d.fallbacks.map((f) => f.account_id)).toEqual(["acct-1"]);
  });

  it("a limited preferred login falls back to the other one", () => {
    const snap = makeSnapshot({ accounts: [first, second], manifests: [manifest], pools: pools("available", "estimated") });
    expect(router().resolve(task(), snap).primary?.account_id).toBe("acct-1");
  });
});
