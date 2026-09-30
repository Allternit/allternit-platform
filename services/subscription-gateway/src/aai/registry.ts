// AAI host: provider registry + one AaiRouter per process. Vendor adapters (grok-bot, claude,
// chatgpt-dots, muse, openclaw) plug in through `registerAaiProvider` — see registerVendorAdapters().
// Kill switch + pacing live in `guardProvider`, which wraps each provider BEFORE the router sees it,
// so router idempotency replays never consume pacing budget.
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import {
  AaiRouter,
  LoopbackProvider,
  fail,
  runConformance,
  type AaiProvider,
  type AaiResult,
  type AgentCapabilityManifest,
  type ConformanceFixtures,
  type ConformanceReport,
} from "@allternit/agent-gateway";
import { aaiOperationSchema, botExecutionBindingSchema, type AAIOperation, type BotExecutionBinding } from "@allternit/subscription-fabric-contracts";
import type { Config } from "../config.js";

export interface AaiPacing {
  /** Minimum gap between context.open / context.message calls to the vendor. */
  minGapMs?: number;
  /** Sliding one-hour cap on context.open + context.message calls. */
  maxPerHour?: number;
}

export interface AaiRegistration {
  provider: AaiProvider;
  pacing?: AaiPacing;
  /** Recorded-session fixtures for POST /aai/conformance/:adapterId. */
  fixtures?: ConformanceFixtures;
  /** Start disabled (kill switch on). */
  disabled?: boolean;
}

// Ops that never reach the vendor's account/lane and stay available while the lane is killed.
const KILL_EXEMPT = new Set<keyof AaiProvider>(["list", "get", "capabilities", "identity", "health", "events", "contextCancel", "contextClose"]);
const PACED = new Set<keyof AaiProvider>(["contextOpen", "contextMessage"]);

export interface AaiHostState {
  isDisabled(id: string): boolean;
  pace(id: string, pacing: AaiPacing | undefined, now: number): AaiResult<never> | undefined;
}

export function guardProvider(provider: AaiProvider, host: AaiHostState, pacing?: AaiPacing): AaiProvider {
  const id = provider.adapterId;
  return new Proxy(provider, {
    get(target, prop, receiver) {
      const v = Reflect.get(target, prop, receiver);
      if (typeof v !== "function" || typeof prop !== "string") return v;
      const key = prop as keyof AaiProvider;
      return (...args: unknown[]) => {
        if (host.isDisabled(id) && !KILL_EXEMPT.has(key)) {
          return Promise.resolve(fail("LANE_BLOCKED", `adapter ${id} is disabled (kill switch)`, { retryable: false }));
        }
        if (PACED.has(key)) {
          const blocked = host.pace(id, pacing, Date.now());
          if (blocked) return Promise.resolve(blocked);
        }
        return (v as (...a: unknown[]) => unknown).apply(target, args);
      };
    },
  });
}

export class AaiHost {
  readonly router = new AaiRouter();
  /** Resolves with the vendor adapter ids registered at boot. */
  vendorsReady: Promise<string[]> = Promise.resolve([]);
  private regs = new Map<string, AaiRegistration & { guarded: AaiProvider }>();
  private disabled = new Set<string>();
  private lastCall = new Map<string, number>();
  private history = new Map<string, number[]>();

  constructor(disabledIds: Iterable<string> = []) {
    for (const d of disabledIds) this.disabled.add(d);
  }

  register(reg: AaiRegistration): this {
    const id = reg.provider.adapterId;
    if (reg.disabled) this.disabled.add(id);
    const guarded = guardProvider(reg.provider, this, reg.pacing);
    this.regs.set(id, { ...reg, guarded });
    this.router.register(guarded);
    return this;
  }

  isDisabled(id: string): boolean { return this.disabled.has(id); }
  setDisabled(id: string, disabled: boolean): void { if (disabled) this.disabled.add(id); else this.disabled.delete(id); }

  pace(id: string, pacing: AaiPacing | undefined, now: number): AaiResult<never> | undefined {
    if (!pacing) return undefined;
    const last = this.lastCall.get(id);
    if (pacing.minGapMs && last !== undefined && now - last < pacing.minGapMs) {
      return fail("LANE_BLOCKED", `pacing: min gap ${pacing.minGapMs}ms not elapsed for ${id}`, { retryable: true, retryAfterMs: pacing.minGapMs - (now - last) });
    }
    const hist = (this.history.get(id) ?? []).filter((t) => now - t < 3_600_000);
    if (pacing.maxPerHour && hist.length >= pacing.maxPerHour) {
      return fail("LANE_BLOCKED", `pacing: hourly cap ${pacing.maxPerHour} reached for ${id}`, { retryable: true, retryAfterMs: 3_600_000 - (now - hist[0]) });
    }
    hist.push(now);
    this.history.set(id, hist);
    this.lastCall.set(id, now);
    return undefined;
  }

  async providers(): Promise<Array<{ adapterId: string; disabled: boolean; pacing?: AaiPacing; manifest: AgentCapabilityManifest | null; error?: string }>> {
    const out = [];
    for (const [adapterId, r] of this.regs) {
      let manifest: AgentCapabilityManifest | null = null;
      let error: string | undefined;
      try {
        const agents = await r.provider.list();
        const agentId = agents.ok ? agents.value[0]?.agentId : undefined;
        const m = await r.provider.capabilities(agentId ?? "");
        if (m.ok) manifest = m.value; else error = m.error.humanMessage;
      } catch (e) { error = (e as Error).message; }
      out.push({ adapterId, disabled: this.disabled.has(adapterId), ...(r.pacing ? { pacing: r.pacing } : {}), manifest, ...(error ? { error } : {}) });
    }
    return out;
  }

  async conformance(adapterId: string): Promise<ConformanceReport | undefined> {
    const r = this.regs.get(adapterId);
    if (!r) return undefined;
    let fixtures = r.fixtures;
    if (!fixtures) {
      const agents = await r.provider.list();
      fixtures = { agentId: agents.ok ? agents.value[0]?.agentId ?? "" : "" };
    }
    return runConformance(r.provider, fixtures); // raw provider: harness must not consume pacing/kill-switch
  }

  async call(op: string, binding: BotExecutionBinding, input: Record<string, unknown>): Promise<AaiResult<unknown>> {
    const parsed = aaiOperationSchema.safeParse(op);
    if (!parsed.success) return fail("UNSUPPORTED", `unknown AAI op ${op}`, { retryable: false });
    const p = this.router.bind(binding);
    const i = input as any;
    const dispatch: Record<AAIOperation, () => Promise<AaiResult<unknown>>> = {
      "agent.list": () => p.list(),
      "agent.get": () => p.get(i.agentId),
      "agent.capabilities": () => p.capabilities(i.agentId),
      "agent.identity": () => p.identity(i.agentId),
      "agent.context.open": () => p.contextOpen(i),
      "agent.context.message": () => p.contextMessage(i),
      "agent.context.steer": () => p.contextSteer(i),
      "agent.context.cancel": () => p.contextCancel(i),
      "agent.context.close": () => p.contextClose(i),
      "agent.events": async () => {
        const r = await p.events(i);
        return r.ok ? { ok: true, value: { events: r.value.events, cursor: r.value.nextCursor } } : r;
      },
      "agent.tasks": () => p.tasks(i),
      "agent.memory": () => p.memory(i),
      "agent.computer": () => p.computer(i),
      "agent.artifacts": () => p.artifacts(i),
      "agent.approvals": () => p.approvals(i),
      "agent.snapshot": () => p.snapshot(i),
      "agent.sync": () => p.sync(i),
      "agent.health": () => p.health(i),
    };
    const fn = dispatch[parsed.data];
    if (!fn) return fail("UNSUPPORTED", `AAI op ${op} has no host dispatch`, { retryable: false });
    try { return await fn(); }
    catch (e) { return fail("UNKNOWN", `host error: ${(e as Error).message}`, { retryable: false }); }
  }
}

export { botExecutionBindingSchema };

/**
 * Registration point for vendor adapters. Next waves add here (each a `host.register({ provider, pacing, fixtures })`):
 * grok-bot, claude, chatgpt-dots, muse, openclaw.
 */
/**
 * Vendor adapters register themselves: any `adapters/<id>/aai.js` (or `aai.ts`) that exports
 * `createAaiRegistration(env)` is loaded at boot (same runtime-import convention as the
 * subscription worker's `adapter.ts`, so `src/` never imports adapter code statically).
 * Until it finishes, calls for that adapter return UNSUPPORTED.
 */
export async function registerVendorAdapters(host: AaiHost, adaptersDir: string, env: NodeJS.ProcessEnv): Promise<string[]> {
  const loaded: string[] = [];
  if (!existsSync(adaptersDir)) return loaded;
  for (const id of readdirSync(adaptersDir).sort()) {
    const file = ["aai.js", "aai.ts"].map((n) => join(adaptersDir, id, n)).find((f) => existsSync(f));
    if (!file) continue;
    const mod = (await import(pathToFileURL(file).href)) as { createAaiRegistration?: (env: NodeJS.ProcessEnv) => AaiRegistration };
    if (typeof mod.createAaiRegistration !== "function") {
      throw new Error(`adapter ${id}: aai module has no createAaiRegistration()`);
    }
    host.register(mod.createAaiRegistration(env));
    loaded.push(id);
  }
  return loaded;
}

export function createAaiHost(config: Config, env: NodeJS.ProcessEnv = process.env, fetchImpl?: typeof fetch): AaiHost {
  const host = new AaiHost(config.aai.disabled);
  const token = env.SUBS_GATEWAY_AAI_LOOPBACK_TOKEN;
  host.register({
    provider: new LoopbackProvider({
      baseUrl: config.aai.loopbackBaseUrl,
      botIds: config.aai.loopbackBots,
      fetch: fetchImpl,
      auth: token ? { token } : undefined,
    }),
  });
  host.vendorsReady = registerVendorAdapters(host, config.adaptersDir, env).catch((e) => {
    console.error("[aai] vendor adapter registration failed:", e);
    return [];
  });
  return host;
}
