import type { BotExecutionBinding } from "@allternit/subscription-fabric-contracts";
import {
  fail,
  type AaiProvider, type AaiResult, type AgentCapabilityManifest, type MessageResult,
} from "./types";

const DEAD_STATES: Record<string, [Parameters<typeof fail>[0], string]> = {
  UNBOUND: ["LANE_BLOCKED", "binding is not bound"],
  NEEDS_AUTH: ["AUTH_REQUIRED", "binding needs re-authentication"],
  PAUSED: ["LANE_BLOCKED", "binding is paused"],
  DISABLED: ["LANE_BLOCKED", "binding is disabled (kill switch)"],
  FAILED: ["VENDOR_UNAVAILABLE", "binding has failed"],
};

/**
 * Registry of AAI providers by adapterId. `bind(binding)` returns an AaiProvider view that:
 *  - routes to the provider named by binding.adapterId
 *  - enforces idempotency on context.message by correlationId (replay -> first result, no second call)
 *  - enforces declared maxParallel contexts (CONTEXT_BUSY)
 *  - refuses to resolve a vendor approval unless the actor is an explicit human
 *  - converts raw throws into AAIError UNKNOWN
 */
export class AaiRouter {
  private providers = new Map<string, AaiProvider>();
  private manifests = new Map<string, AgentCapabilityManifest>();
  private idem = new Map<string, Promise<AaiResult<MessageResult>>>();
  private slots = new Map<string, Set<string>>();

  register(provider: AaiProvider): this {
    this.providers.set(provider.adapterId, provider);
    return this;
  }

  bind(binding: BotExecutionBinding): AaiProvider {
    const self = this;
    const agentOf = (a?: string) => binding.externalAgentId ?? a ?? binding.botId;
    const resolve = (): AaiResult<AaiProvider> => {
      const id = binding.adapterId ?? (binding.type === "allternit" ? "allternit-loopback" : undefined);
      const p = id ? self.providers.get(id) : undefined;
      if (!p) return fail("UNSUPPORTED", `no provider registered for adapter ${id ?? "(none)"}`);
      const dead = DEAD_STATES[binding.state];
      if (dead) return fail(dead[0], dead[1]);
      return { ok: true, value: p };
    };
    const guard = async <T>(fn: (p: AaiProvider) => Promise<AaiResult<T>>): Promise<AaiResult<T>> => {
      const r = resolve();
      if (!r.ok) return r;
      try { return await fn(r.value); }
      catch (e) { return fail("UNKNOWN", `provider threw: ${(e as Error)?.message ?? String(e)}`, { retryable: false }); }
    };
    const manifest = async (p: AaiProvider): Promise<AgentCapabilityManifest | undefined> => {
      const key = `${p.adapterId}:${agentOf()}`;
      if (!self.manifests.has(key)) {
        const m = await p.capabilities(agentOf());
        if (m.ok) self.manifests.set(key, m.value);
      }
      return self.manifests.get(key);
    };
    const slotSet = () => {
      if (!self.slots.has(binding.id)) self.slots.set(binding.id, new Set());
      return self.slots.get(binding.id)!;
    };
    const idemKey = (cid: string, corr: string) => `${binding.id}|${cid}|${corr}`;

    const view: AaiProvider = {
      adapterId: binding.adapterId ?? "allternit-loopback",
      list: () => guard((p) => p.list()),
      get: (a) => guard((p) => p.get(agentOf(a))),
      capabilities: (a) => guard((p) => p.capabilities(agentOf(a))),
      identity: (a) => guard((p) => p.identity(agentOf(a))),
      contextOpen: (i) => guard(async (p) => {
        const max = (await manifest(p))?.context.maxParallel ?? 0;
        const slots = slotSet();
        if (max > 0 && slots.size >= max) {
          return fail("CONTEXT_BUSY", `maxParallel=${max} contexts already open for this binding`, { retryable: true });
        }
        const placeholder = `pending:${Math.random()}`;
        slots.add(placeholder); // reserve synchronously so concurrent opens can't overshoot
        try {
          const r = await p.contextOpen({ ...i, agentId: agentOf(i.agentId) });
          slots.delete(placeholder);
          if (r.ok) slots.add(r.value.contextId);
          return r;
        } catch (e) { slots.delete(placeholder); throw e; }
      }),
      contextMessage: (i) => guard(async (p) => {
        const key = idemKey(i.contextId, i.correlationId);
        const prior = self.idem.get(key);
        if (prior) return prior;
        const run = p.contextMessage(i).catch((e) =>
          fail("UNKNOWN", `provider threw: ${(e as Error)?.message ?? String(e)}`, { retryable: false }) as AaiResult<MessageResult>);
        self.idem.set(key, run);
        const r = await run;
        if (!r.ok && r.error.retryable) self.idem.delete(key); // transient failure: allow a real retry
        return r;
      }),
      contextSteer: (i) => guard((p) => p.contextSteer(i)),
      contextCancel: (i) => guard((p) => p.contextCancel(i)),
      contextClose: (i) => guard(async (p) => {
        const r = await p.contextClose(i);
        if (r.ok) slotSet().delete(i.contextId);
        return r;
      }),
      events: (i) => guard((p) => p.events(i)),
      tasks: (i) => guard((p) => p.tasks({ ...i, agentId: agentOf(i.agentId) })),
      memory: (i) => guard((p) => p.memory(i)),
      computer: (i) => guard((p) => p.computer(i)),
      artifacts: (i) => guard((p) => p.artifacts({ ...i, agentId: agentOf(i.agentId) })),
      approvals: (i) => guard(async (p) => {
        if (i.op === "respond" && i.actor.type !== "human") {
          // Vendor (and Allternit) approvals are never auto-resolved by the runtime.
          return fail("APPROVAL_REQUIRED", "approval responses require an explicit human actor", { retryable: false });
        }
        return p.approvals(i);
      }),
      snapshot: (i) => guard((p) => p.snapshot({ agentId: agentOf(i.agentId) })),
      sync: (i) => guard((p) => p.sync({ agentId: agentOf(i.agentId) })),
      health: (i) => guard((p) => p.health({ agentId: agentOf(i.agentId) })),
    };
    return view;
  }
}
