import type { BotExecutionBinding } from "@allternit/subscription-fabric-contracts";
import {
  fail,
  type AaiProvider, type AaiResult, type AgentCapabilityManifest, type MessageResult,
} from "./types.js";

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
 *  - enforces declared maxParallel on what runs at once (CONTEXT_BUSY): conversations with an open or message in
 *    flight. Contexts that merely exist do not count (every thread keeps its conversation open).
 *  - refuses to resolve a vendor approval unless the actor is an explicit human
 *  - converts raw throws into AAIError UNKNOWN
 */
export class AaiRouter {
  private providers = new Map<string, AaiProvider>();
  private manifests = new Map<string, AgentCapabilityManifest>();
  private idem = new Map<string, Promise<AaiResult<MessageResult>>>();
  /** binding.id -> conversation (contextId, or a pending open) -> calls in flight. */
  private running = new Map<string, Map<string, number>>();

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
    /** Claim a running slot for `conv` (synchronously, so concurrent calls can't overshoot); undefined = busy. */
    const occupy = (conv: string, max: number): (() => void) | undefined => {
      if (!self.running.has(binding.id)) self.running.set(binding.id, new Map());
      const m = self.running.get(binding.id)!;
      if (max > 0 && !m.has(conv) && m.size >= max) return undefined;
      m.set(conv, (m.get(conv) ?? 0) + 1);
      return () => { const n = (m.get(conv) ?? 1) - 1; if (n > 0) m.set(conv, n); else m.delete(conv); };
    };
    const busy = (max: number) => fail("CONTEXT_BUSY", `maxParallel=${max} conversations already running for this binding`, { retryable: true });
    const idemKey = (cid: string, corr: string) => `${binding.id}|${cid}|${corr}`;

    const view: AaiProvider = {
      adapterId: binding.adapterId ?? "allternit-loopback",
      list: () => guard((p) => p.list()),
      get: (a) => guard((p) => p.get(agentOf(a))),
      capabilities: (a) => guard((p) => p.capabilities(agentOf(a))),
      identity: (a) => guard((p) => p.identity(agentOf(a))),
      contextOpen: (i) => guard(async (p) => {
        const max = (await manifest(p))?.context.maxParallel ?? 0;
        const release = occupy(`pending:${Math.random()}`, max);
        if (!release) return busy(max);
        try { return await p.contextOpen({ ...i, agentId: agentOf(i.agentId) }); }
        finally { release(); }
      }),
      contextMessage: (i) => guard(async (p) => {
        const key = idemKey(i.contextId, i.correlationId);
        const prior = self.idem.get(key);
        if (prior) return prior;
        const run = (async (): Promise<AaiResult<MessageResult>> => {
          const max = (await manifest(p))?.context.maxParallel ?? 0;
          const release = occupy(i.contextId, max);
          if (!release) return busy(max);
          try { return await p.contextMessage(i); }
          finally { release(); }
        })().catch((e) =>
          fail("UNKNOWN", `provider threw: ${(e as Error)?.message ?? String(e)}`, { retryable: false }) as AaiResult<MessageResult>);
        self.idem.set(key, run);
        const r = await run;
        if (!r.ok && r.error.retryable) self.idem.delete(key); // transient failure: allow a real retry
        return r;
      }),
      contextSteer: (i) => guard((p) => p.contextSteer(i)),
      contextCancel: (i) => guard((p) => p.contextCancel(i)),
      contextClose: (i) => guard((p) => p.contextClose(i)),
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
