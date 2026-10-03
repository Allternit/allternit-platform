// Offline AAI registration for POST /aai/conformance/copilot-subscription: a fake in-process task API stands in for the
// gateway's own /v1/tasks (no copilot.microsoft.com, no browser).
import type { AaiRegistration } from "../../../src/aai/registry.js";
import { create } from "../index.js";
import type { GatewayTasks } from "../../_shared/subscription-agent.js";

function fake(fault?: "vendor_down" | "rate_limited" | "auth_revoked"): GatewayTasks {
  const store = new Map<string, Record<string, unknown>>();
  const subs = new Map<string, (e: { kind?: string; payload?: unknown }) => void>();
  const streamed = new Set<string>();
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
    get: async (id) => {
      const t = store.get(id);
      // Stream the reply out to subscribers once, as the worker would.
      if (t && !streamed.has(id) && subs.has(id)) { streamed.add(id); const text = String((t.result as { text?: string } | undefined)?.text ?? ""); for (const ch of [text.slice(0, 3), text.slice(3)]) if (ch) subs.get(id)!({ kind: "reply", payload: { event: { type: "reply.text.delta", delta: ch } } }); }
      return { status: 200, body: t ?? { status: "failed" } };
    },
    subscribe: (id, cb) => { subs.set(id, cb); return () => subs.delete(id); },
    cancel: async (id) => { const t = store.get(id); if (!t) return { status: 404, body: {} }; store.set(id, { ...t, status: "cancelled" }); return { status: 200, body: {} }; },
  };
}
const mk = (f?: Parameters<typeof fake>[0]) => create({ tasks: fake(f), pollMs: 1, sleep: async () => {} });

export async function createOfflineAaiRegistration(): Promise<{ registration: AaiRegistration; close: () => Promise<void> }> {
  return {
    registration: { provider: mk(), fixtures: {
      agentId: "copilot", settleMs: 10,
      faulty: { vendor_down: () => mk("vendor_down"), rate_limited: () => mk("rate_limited"), auth_revoked: () => mk("auth_revoked") },
    } },
    close: async () => {},
  };
}
