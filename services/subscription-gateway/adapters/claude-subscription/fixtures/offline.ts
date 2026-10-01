// Offline AAI registration for POST /aai/conformance/claude-subscription: a fake in-process task API stands in for the
// gateway's own /v1/tasks (no claude.ai, no browser).
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { ClaudeSubscriptionProvider, type GatewayTasks } from "../index.js";

function fake(fault?: "vendor_down" | "rate_limited" | "auth_revoked"): GatewayTasks {
  const store = new Map<string, Record<string, unknown>>();
  let n = 0;
  return {
    accountState: async () => {
      if (fault === "vendor_down") throw new Error("gateway socket unavailable");
      if (fault === "auth_revoked") return { health: "auth_required" };
      return { health: "ready", remainingPct: fault === "rate_limited" ? 0 : 50, resetsAt: null };
    },
    submit: async (body) => {
      if (fault === "vendor_down") throw new Error("gateway socket unavailable");
      const id = `t${++n}`;
      store.set(id, fault === "rate_limited"
        ? { task_id: id, status: "failed", error: { class: "rate_limited", detail: "You've reached your usage limit.", cooldown_s: 60, user_action: null } }
        : { task_id: id, status: "completed", result: { artifact_ids: [], text: `ok: ${String(body.prompt)}` } });
      return { status: 202, body: { task_id: id, status: "queued" } };
    },
    get: async (id) => ({ status: 200, body: store.get(id) ?? { status: "failed" } }),
  };
}
const mk = (f?: Parameters<typeof fake>[0]) => new ClaudeSubscriptionProvider({ tasks: fake(f), pollMs: 1, sleep: async () => {} });

export async function createOfflineAaiRegistration(): Promise<{ registration: AaiRegistration; close: () => Promise<void> }> {
  return {
    registration: { provider: mk(), fixtures: {
      agentId: "claude", settleMs: 10,
      faulty: { vendor_down: () => mk("vendor_down"), rate_limited: () => mk("rate_limited"), auth_revoked: () => mk("auth_revoked") },
    } },
    close: async () => {},
  };
}
