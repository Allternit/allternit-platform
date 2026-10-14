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
  isPersistentContexts,
  runConformance,
  type AaiProvider,
  type AaiResult,
  type AgentCapabilityManifest,
  type ConformanceFixtures,
  type ConformanceReport,
} from "@allternit/agent-gateway";
import { aaiOperationSchema, botExecutionBindingSchema, type AAIOperation, type BotExecutionBinding } from "@allternit/subscription-fabric-contracts";
import type { Config } from "../config.js";
import type { Db } from "../store/db.js";
import { AaiContextStore } from "./context-store.js";
import { TurnQueue, type TurnQueueOptions } from "./turn-queue.js";

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
/** Ops on one context: the host restores a persisted context before them and records their effect after. */
const CONTEXT_OPS = new Set<AAIOperation>([
  "agent.context.open", "agent.context.message", "agent.context.steer", "agent.context.cancel", "agent.context.close", "agent.events",
]);

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

/**
 * allternit-api reads each event flat (`type`, `payload`, and the snake_case `remote_event_id`,
 * `remote_context_id`, `correlation_id`, `causation_id`), while the provider returns `{ cursor, event }` with
 * a camelCase GatewayEvent. Emit the flat event with both spellings so neither reader loses fields.
 */
export function wireEvent(c: { cursor: string; event: Record<string, any> }): Record<string, unknown> {
  const e = c.event ?? {};
  const out: Record<string, unknown> = { ...e, cursor: c.cursor };
  const alias: Array<[string, string]> = [["remoteEventId", "remote_event_id"], ["remoteContextId", "remote_context_id"], ["correlationId", "correlation_id"], ["causationId", "causation_id"]];
  for (const [camel, snake] of alias) if (e[camel] !== undefined && out[snake] === undefined) out[snake] = e[camel];
  return out;
}

export class AaiHost {
  readonly router = new AaiRouter();
  /** Resolves with the vendor adapter ids registered at boot. */
  vendorsReady: Promise<string[]> = Promise.resolve([]);
  private regs = new Map<string, AaiRegistration & { guarded: AaiProvider }>();
  private disabled = new Set<string>();
  private lastCall = new Map<string, number>();
  private history = new Map<string, number[]>();
  /** Vendor adapters whose aai module failed to load at boot: id -> why (shown by GET /aai/providers). */
  private loadErrors = new Map<string, string>();

  /** Persisted context state (state.db); absent in tests that need no restart story. */
  readonly contexts?: AaiContextStore;
  /** Contexts opened or restored by this process: `${adapterId}\u0000${contextId}`. */
  private liveContexts = new Set<string>();

  /** Per-account FIFO for UI-bridge turns (see queueGate). */
  readonly turns: TurnQueue;
  /** How often a turn holding the account re-tries a provider that still answers CONTEXT_BUSY (its reply streams on). */
  private busyRetryMs: number;
  private manifestCache = new Map<string, AgentCapabilityManifest>();
  /** Queue status notices not yet read, per `${adapterId}\u0000${contextId}` (delivered once, by agent.events). */
  private notices = new Map<string, Array<Record<string, unknown>>>();
  /** Messages in flight by adapter/binding/context/correlationId. */
  private inflight = new Map<string, Promise<AaiResult<unknown>>>();

  constructor(disabledIds: Iterable<string> = [], opts: { contexts?: AaiContextStore; queue?: TurnQueueOptions & { busyRetryMs?: number } } = {}) {
    for (const d of disabledIds) this.disabled.add(d);
    this.contexts = opts.contexts;
    this.turns = new TurnQueue(opts.queue);
    this.busyRetryMs = opts.queue?.busyRetryMs ?? 1000;
  }

  register(reg: AaiRegistration): this {
    const id = reg.provider.adapterId;
    if (reg.disabled) this.disabled.add(id);
    const guarded = guardProvider(reg.provider, this, reg.pacing);
    this.regs.set(id, { ...reg, guarded });
    this.router.register(guarded);
    return this;
  }

  recordLoadError(id: string, error: string): void { this.loadErrors.set(id, error); }

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
        if (m.ok) manifest = m.value;
        // No agent to ask about: say why (vendor not running / signed out) rather than "No such agent ".
        else if (!agentId) error = `no agent available${agents.ok ? " (the vendor lists none; is it running and signed in?)" : `: ${agents.error.humanMessage}`}`;
        else error = m.error.humanMessage;
      } catch (e) { error = (e as Error).message; }
      out.push({ adapterId, disabled: this.disabled.has(adapterId), ...(r.pacing ? { pacing: r.pacing } : {}), manifest, ...(error ? { error } : {}) });
    }
    for (const [adapterId, error] of this.loadErrors) {
      if (!this.regs.has(adapterId)) out.push({ adapterId, disabled: this.disabled.has(adapterId), manifest: null, error: `failed to load: ${error}` });
    }
    return out;
  }

  /**
   * Conformance against the adapter's shipped offline fixtures (`adapters/<id>/fixtures/offline.ts`): a
   * throwaway provider backed by local fakes, so no vendor is needed and the live registration is untouched.
   * `undefined` when the adapter is not registered here; `null` when it ships no offline fixtures.
   */
  async offlineConformance(adapterId: string, adaptersDir: string): Promise<ConformanceReport | null | undefined> {
    if (!this.regs.has(adapterId)) return undefined;
    const file = ["offline.ts", "offline.js"].map((n) => join(adaptersDir, adapterId, "fixtures", n)).find((f) => existsSync(f));
    if (!file) return null;
    const mod = (await import(pathToFileURL(file).href)) as { createOfflineAaiRegistration: () => Promise<{ registration: AaiRegistration; close?: () => Promise<void> }> };
    const { registration, close } = await mod.createOfflineAaiRegistration();
    try {
      const agents = registration.fixtures ? undefined : await registration.provider.list();
      const fixtures = registration.fixtures ?? { agentId: agents?.ok ? agents.value[0]?.agentId ?? "" : "" };
      return await runConformance(registration.provider, fixtures);
    } finally {
      await close?.();
    }
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

  private async manifestOf(adapterId: string, agentId: string): Promise<AgentCapabilityManifest | undefined> {
    const key = `${adapterId}\u0000${agentId}`;
    const hit = this.manifestCache.get(key);
    if (hit) return hit;
    const reg = this.regs.get(adapterId);
    const m = reg ? await reg.provider.capabilities(agentId).catch(() => undefined) : undefined;
    if (m?.ok) this.manifestCache.set(key, m.value);
    return m?.ok ? m.value : undefined;
  }

  /**
   * Website/desktop subscriptions (manifest lane `ui_bridge`) drive one signed-in window or tab per account, so turns
   * on the same account take turns: up to `context.maxParallel` run, the rest wait in arrival order (TurnQueue) and
   * report "queued … (position N)" as an `agent.activity.started` event on their context. Opens wait too on lanes
   * whose one conversation surface is shared (isolation `shared`: opening drives the app). The account is the adapter:
   * each of these adapters drives the one signed-in app/login on this computer. undefined = not queued.
   */
  private async queueGate(adapterId: string | undefined, op: AAIOperation, binding: BotExecutionBinding, i: any): Promise<{ account: string; release?: () => void; deadline: number; error?: AaiResult<never> } | undefined> {
    if (!adapterId || !this.regs.has(adapterId) || this.disabled.has(adapterId)) return undefined;
    if (op !== "agent.context.message" && op !== "agent.context.open") return undefined;
    if (op === "agent.context.open" && i.adoptContextId) return undefined;
    const contextId = op === "agent.context.message" && typeof i.contextId === "string" ? i.contextId : undefined;
    const agentId = binding.externalAgentId ?? i.agentId ?? (contextId ? this.contexts?.get(adapterId, contextId)?.agentId : undefined) ?? binding.botId;
    const m = await this.manifestOf(adapterId, agentId);
    if (!m || m.lane !== "ui_bridge") return undefined;
    if (op === "agent.context.open" && m.context.isolation !== "shared") return undefined;
    const account = adapterId;
    const label = `your ${vendorName(m.vendor)} ${m.runtime.localRequired ? "app" : "tab"}`;
    const corr = typeof i.correlationId === "string" ? i.correlationId : "";
    const deadline = Date.now() + this.turns.maxWaitMs;
    const r = await this.turns.acquire(account, m.context.maxParallel || 1, {
      contextId, label,
      onPosition: contextId ? (pos) => this.notice(adapterId, contextId, agentId, m.vendor, corr, pos, label) : undefined,
    });
    return r.ok ? { account, release: r.value, deadline } : { account, deadline, error: r };
  }

  private notice(adapterId: string, contextId: string, agentId: string, vendor: string, corr: string, position: number, label: string): void {
    const key = `${adapterId}\u0000${contextId}`;
    const list = this.notices.get(key) ?? [];
    const queued = position > 0;
    list.push({
      type: "agent.activity.started", botId: agentId, threadId: contextId, generationId: "1", source: "allternit", vendor, adapter: adapterId,
      lane: "ui_bridge", remoteContextId: contextId, remoteEventId: `${contextId}#queue:${corr}:${position}`, correlationId: corr, causationId: corr,
      guarantee: "exact", at: new Date().toISOString(),
      payload: {
        activityId: `queue:${corr}`, kind: "queued", label: queued ? "Queued" : "Starting", state: queued ? "queued" : "started", position,
        detail: queued ? `queued: waiting for ${label} (position ${position})` : `starting: ${label} is free`,
      },
    });
    this.notices.set(key, list);
  }

  /**
   * A context this process has not seen but the store has (the gateway restarted since it opened): restore it into a
   * provider that persists its contexts, let a provider that revives its own (manifest `context.resume`) handle it, and
   * otherwise answer that it was lost. CONTEXT_NOT_FOUND is the code callers already hand off on (allternit-api starts a
   * new generation from its checkpoint); `details.reason: "CONTEXT_LOST"` says why. undefined = carry on.
   */
  private async revive(adapterId: string | undefined, contextId: unknown): Promise<AaiResult<never> | undefined> {
    if (!this.contexts || !adapterId || typeof contextId !== "string" || !contextId) return undefined;
    const key = `${adapterId}\u0000${contextId}`;
    if (this.liveContexts.has(key)) return undefined;
    const row = this.contexts.get(adapterId, contextId);
    const reg = this.regs.get(adapterId);
    if (!row || !reg) return undefined; // not a context we handed out: the provider decides
    if (row.closed) return fail("CONTEXT_NOT_FOUND", "This conversation was closed.", { retryable: false, details: { contextId, closed: true } });
    const p = reg.provider;
    if (isPersistentContexts(p)) {
      let restored = false;
      try { restored = p.restoreContext(contextId, { agentId: row.agentId ?? undefined, conversationKeys: row.conversationKeys ?? undefined, state: row.state ?? undefined }); }
      catch (e) { console.error(`[aai] ${adapterId} could not restore context ${contextId}:`, e); }
      if (restored) { this.liveContexts.add(key); return undefined; }
    } else {
      const m = await p.capabilities(row.agentId ?? row.binding.externalAgentId ?? row.binding.botId).catch(() => undefined);
      if (m?.ok && m.value.context.resume) { this.liveContexts.add(key); return undefined; }
    }
    return fail("CONTEXT_NOT_FOUND", "This conversation was lost when the gateway restarted and can't be resumed; a new one has to be started.", {
      retryable: false, details: { contextId, reason: "CONTEXT_LOST", lastCursor: row.lastCursor },
    });
  }

  /** Bookkeeping after an op on a context: what the store needs to bring it back after a restart. */
  private remember(adapterId: string | undefined, op: AAIOperation, binding: BotExecutionBinding, i: any, r: AaiResult<unknown>): void {
    const store = this.contexts;
    if (!store || !adapterId) return;
    const p = this.regs.get(adapterId)?.provider;
    const snap = (id: string) => { try { return p && isPersistentContexts(p) ? p.exportContext(id) : undefined; } catch { return undefined; } };
    try {
      if (op === "agent.context.open" && r.ok) {
        const id = (r.value as { contextId: string }).contextId;
        store.opened(adapterId, id, binding, binding.externalAgentId ?? i.agentId ?? binding.botId ?? null, snap(id));
        this.liveContexts.add(`${adapterId}\u0000${id}`);
      } else if (op === "agent.context.message" && typeof i.contextId === "string") {
        const s = snap(i.contextId);
        if (s) store.saveSnapshot(adapterId, i.contextId, s);
      } else if (op === "agent.events" && r.ok && typeof i.contextId === "string") {
        const cursor = (r.value as { cursor?: unknown }).cursor;
        if (typeof cursor === "string") store.cursor(adapterId, i.contextId, cursor);
      } else if (op === "agent.context.close" && r.ok && typeof i.contextId === "string") {
        store.closed(adapterId, i.contextId);
      }
    } catch (e) { console.error(`[aai] could not persist ${op} for ${adapterId}:`, e); }
  }

  async call(op: string, binding: BotExecutionBinding, input: Record<string, unknown>): Promise<AaiResult<unknown>> {
    const parsed = aaiOperationSchema.safeParse(op);
    if (!parsed.success) return fail("UNSUPPORTED", `unknown AAI op ${op}`, { retryable: false });
    const p = this.router.bind(binding);
    const i = input as any;
    // allternit-api names the target agent `externalAgentId` (its binding field); the AAI input is `agentId`.
    if (i.agentId === undefined && typeof i.externalAgentId === "string") i.agentId = i.externalAgentId;
    const adapterId = binding.adapterId ?? (binding.type === "allternit" ? "allternit-loopback" : undefined);
    if (CONTEXT_OPS.has(parsed.data)) {
      const lost = await this.revive(adapterId, parsed.data === "agent.context.open" ? i.adoptContextId : i.contextId);
      if (lost) return lost;
    }
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
        if (!r.ok) return r;
        const events = r.value.events.map(wireEvent);
        // Queue notices ride at the reader's current position (they never move the provider's cursor).
        const key = `${adapterId}\u0000${i.contextId}`;
        const pending = this.notices.get(key);
        if (pending) { this.notices.delete(key); for (const n of pending) events.push(wireEvent({ cursor: r.value.nextCursor, event: n })); }
        return { ok: true, value: { events, cursor: r.value.nextCursor } };
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
    // Cancel of a turn that is only waiting: take it out of line without touching the vendor (a Stop click there
    // would stop whichever reply is running, maybe another thread's).
    if (parsed.data === "agent.context.cancel" && adapterId && typeof i.contextId === "string") {
      const dropped = this.turns.cancel(adapterId, i.contextId);
      if (dropped && !this.turns.isRunning(adapterId, i.contextId)) return { ok: true, value: { confirmed: true } };
    }
    // A replayed message (same correlationId) joins the first one's run instead of taking a second place in line.
    const replayKey = parsed.data === "agent.context.message" ? `${adapterId}\u0000${binding.id}\u0000${i.contextId}\u0000${i.correlationId}` : undefined;
    const first = replayKey ? this.inflight.get(replayKey) : undefined;
    if (first) return first;
    const run = this.run(parsed.data, adapterId, binding, i, fn);
    if (replayKey) { this.inflight.set(replayKey, run); void run.finally(() => this.inflight.delete(replayKey)); }
    return run;
  }

  private async run(op: AAIOperation, adapterId: string | undefined, binding: BotExecutionBinding, i: any, fn: () => Promise<AaiResult<unknown>>): Promise<AaiResult<unknown>> {
    const gate = await this.queueGate(adapterId, op, binding, i);
    if (gate?.error) return gate.error;
    let r: AaiResult<unknown>;
    try {
      r = await fn();
      // Holding the account, a provider can still be busy finishing the previous reply (it streams on past its
      // turn's deadline): keep this turn's place and retry until the queue wait runs out.
      while (gate && !r.ok && r.error.code === "CONTEXT_BUSY" && Date.now() + this.busyRetryMs < gate.deadline) {
        await new Promise((res) => setTimeout(res, this.busyRetryMs));
        r = await fn();
      }
    }
    catch (e) { return fail("UNKNOWN", `host error: ${(e as Error).message}`, { retryable: false }); }
    finally { gate?.release?.(); }
    if (CONTEXT_OPS.has(op)) this.remember(adapterId, op, binding, i, r);
    return r;
  }
}

const VENDOR_NAMES: Record<string, string> = {
  grok: "Grok", xai: "Grok", openai: "ChatGPT", chatgpt: "ChatGPT", anthropic: "Claude", claude: "Claude",
  kimi: "Kimi", moonshot: "Kimi", google: "Gemini", gemini: "Gemini", copilot: "Copilot", microsoft: "Copilot", github: "Copilot",
};
const vendorName = (v: string) => VENDOR_NAMES[v.toLowerCase()] ?? (v ? v[0].toUpperCase() + v.slice(1) : "vendor");

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
/**
 * What vendor adapters may ask the gateway for. subscriptionProfile: the
 * Chrome profile of the provider's preferred signed-in subscription account
 * (Settings → Subscriptions), freed from the chat worker first so the vendor
 * adapter can open it (one Chrome per profile). null when none is ready.
 */
export interface VendorContext {
  subscriptionProfile?: (provider: string) => Promise<{ dir: string; account_id: string } | null>;
  /** This gateway's own task API (submit/get over its UDS) plus "is a subscription login Ready". */
  gatewayTasks?: {
    submit(body: Record<string, unknown>): Promise<{ status: number; body: Record<string, unknown> }>;
    get(taskId: string): Promise<{ status: number; body: Record<string, unknown> }>;
    cancel?(taskId: string): Promise<{ status: number; body: Record<string, unknown> }>;
    subscribe?(taskId: string, onEvent: (event: { kind?: string; payload?: unknown }) => void): () => void;
    accountState(provider: string): Promise<{ health: string; remainingPct?: number | null; resetsAt?: string | null; agents?: { id: string; name: string; kind?: string; kindLabel?: string; avatarUrl?: string }[] | null } | null>;
  };
}

export async function registerVendorAdapters(
  host: AaiHost,
  adaptersDir: string,
  env: NodeJS.ProcessEnv,
  ctx: VendorContext = {}
): Promise<string[]> {
  const loaded: string[] = [];
  if (!existsSync(adaptersDir)) return loaded;
  for (const id of readdirSync(adaptersDir).sort()) {
    const file = ["aai.js", "aai.ts"].map((n) => join(adaptersDir, id, n)).find((f) => existsSync(f));
    if (!file) continue;
    // Each adapter loads on its own: one whose import or factory throws is recorded (GET /aai/providers shows it
    // with an `error`) and skipped, so it never stops the adapters after it from registering.
    try {
      const mod = (await import(pathToFileURL(file).href)) as {
        createAaiRegistration?: (env: NodeJS.ProcessEnv, ctx: VendorContext) => AaiRegistration;
      };
      if (typeof mod.createAaiRegistration !== "function") {
        throw new Error("aai module has no createAaiRegistration()");
      }
      host.register(mod.createAaiRegistration(env, ctx));
      loaded.push(id);
    } catch (e) {
      const msg = (e as Error)?.message ?? String(e);
      console.error(`[aai] adapter ${id} failed to load:`, e);
      host.recordLoadError(id, msg);
    }
  }
  return loaded;
}

export function createAaiHost(
  config: Config,
  env: NodeJS.ProcessEnv = process.env,
  fetchImpl?: typeof fetch,
  ctx: VendorContext = {},
  db?: Db
): AaiHost {
  const host = new AaiHost(config.aai.disabled, {
    contexts: db ? new AaiContextStore(db) : undefined,
    queue: { maxWaitMs: config.aai.queueWaitMs, maxQueued: config.aai.queueMax },
  });
  const token = env.SUBS_GATEWAY_AAI_LOOPBACK_TOKEN;
  host.register({
    provider: new LoopbackProvider({
      baseUrl: config.aai.loopbackBaseUrl,
      botIds: config.aai.loopbackBots,
      fetch: fetchImpl,
      auth: token ? { token } : undefined,
    }),
  });
  host.vendorsReady = registerVendorAdapters(host, config.adaptersDir, env, ctx).catch((e) => {
    console.error("[aai] vendor adapter registration failed:", e);
    return [];
  });
  return host;
}
