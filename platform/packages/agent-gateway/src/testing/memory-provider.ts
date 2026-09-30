// In-memory AAI provider used to prove the conformance harness discriminates: healthy by default,
// each `defects` flag breaks exactly one contract area.
import { agentCapabilityManifestSchema } from "@allternit/subscription-fabric-contracts";
import { BaseAaiProvider } from "../provider";
import {
  fail, ok, type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type ApprovalsInput,
  type ApprovalsResult, type Approval, type CancelResult, type CursoredEvent, type EventsInput, type EventsResult, type MemoryInput,
  type MemoryResult, type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult, type HealthResult,
} from "../types";

export interface MemoryDefects {
  /** Messages leak into every other open context. */
  contaminate?: boolean;
  /** A replayed correlationId executes again. */
  doubleExecute?: boolean;
  /** Any actor (or a message) resolves pending approvals. */
  autoApprove?: boolean;
  /** cancel claims confirmed but work keeps streaming. */
  ignoreCancel?: boolean;
  /** Opens beyond maxParallel succeed. */
  ignoreMaxParallel?: boolean;
}

interface C { id: string; events: CursoredEvent[]; seq: number; closed: boolean; timers: ReturnType<typeof setTimeout>[]; done: Map<string, MessageResult> }

export class MemoryProvider extends BaseAaiProvider {
  readonly adapterId: string;
  private ctxs = new Map<string, C>();
  private n = 0;
  private mem = new Map<string, string>();
  private approvalsList: Approval[] = [{ authority: "vendor", actor: "vendor", action: "send_email", threadId: "t", remoteRef: "appr-1", state: "pending" }];

  constructor(private agentId = "mem-agent", private maxParallel = 2, private defects: MemoryDefects = {}, adapterId = "memory-test") {
    super();
    this.adapterId = adapterId;
  }

  private manifest(): AgentCapabilityManifest {
    return agentCapabilityManifestSchema.parse({
      vendor: "memory", adapterId: this.adapterId, lane: "local", guarantee: "exact",
      context: { supported: true, resume: true, parallel: true, maxParallel: this.maxParallel, isolation: "isolated" },
      messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
      memory: { read: true, write: true, snapshot: false, opaque: false },
      tools: { tools: false, mcp: false, plugins: false, connectors: false },
      tasks: { list: false, schedule: false, cancel: false, background: false },
      approvals: { read: true, respond: true, exact: true },
      computer: { view: false, control: false, takeover: false },
      artifacts: { read: false, write: false, export: false },
      events: { native: true, polling: false, transcriptDerived: false, replay: true },
      runtime: { alwaysOn: true, localRequired: true, cloud: false },
    });
  }
  private push(c: C, type: "agent.context.opened" | "agent.activity.started" | "agent.message.delta" | "agent.message.completed", correlationId: string, payload: Record<string, unknown>) {
    c.seq += 1;
    c.events.push({ cursor: String(c.seq), event: {
      type, botId: this.agentId, threadId: c.id, generationId: "1", source: "allternit", remoteContextId: c.id, remoteEventId: `${c.id}#${c.seq}`,
      causationId: correlationId, correlationId, guarantee: "exact", payload } });
  }
  private live(id: string): AaiResult<C> { const c = this.ctxs.get(id); return c && !c.closed ? ok(c) : fail("CONTEXT_NOT_FOUND", "no such context"); }

  async list(): Promise<AaiResult<AgentSummary[]>> { return ok([{ agentId: this.agentId, displayName: "Mem", vendor: "memory", state: "READY" }]); }
  async get(id: string): Promise<AaiResult<AgentDetail>> {
    return id === this.agentId ? ok({ agentId: id, displayName: "Mem", vendor: "memory", state: "READY", remoteIds: {}, capabilities: this.manifest() }) : fail("CONTEXT_NOT_FOUND", "unknown agent");
  }
  async capabilities(id: string): Promise<AaiResult<AgentCapabilityManifest>> { return id === this.agentId ? ok(this.manifest()) : fail("CONTEXT_NOT_FOUND", "unknown agent"); }
  async identity(id: string): Promise<AaiResult<AgentIdentity>> { return id === this.agentId ? ok({ agentId: id, displayName: "Mem", vendor: "memory" }) : fail("CONTEXT_NOT_FOUND", "unknown agent"); }
  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> { return ok({ status: "healthy" }); }

  async contextOpen(i: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (i.adoptContextId) {
      const c = this.live(i.adoptContextId);
      return c.ok ? ok({ contextId: c.value.id, isolation: "isolated", guarantee: "exact", resumed: true }) : c;
    }
    const open = [...this.ctxs.values()].filter((c) => !c.closed).length;
    if (!this.defects.ignoreMaxParallel && open >= this.maxParallel) return fail("CONTEXT_BUSY", "maxParallel reached");
    const c: C = { id: `mctx-${++this.n}`, events: [], seq: 0, closed: false, timers: [], done: new Map() };
    this.ctxs.set(c.id, c);
    this.push(c, "agent.context.opened", `open:${c.id}`, {});
    return ok({ contextId: c.id, isolation: "isolated", guarantee: "exact", resumed: false });
  }

  async contextMessage(i: MessageInput): Promise<AaiResult<MessageResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    const ctx = c.value;
    const prior = ctx.done.get(i.correlationId);
    if (prior && !this.defects.doubleExecute) return ok(prior);
    const res: MessageResult = { messageId: `${ctx.id}:m${ctx.done.size + 1}${prior ? "-again" : ""}`, correlationId: i.correlationId, reply: `ack: ${i.text}`, guarantee: "exact" };
    ctx.done.set(i.correlationId, prior ? prior : res);
    const targets = this.defects.contaminate ? [...this.ctxs.values()].filter((x) => !x.closed) : [ctx];
    for (const t of targets) {
      this.push(t, "agent.activity.started", i.correlationId, { text: i.text });
      this.push(t, "agent.message.completed", i.correlationId, { reply: res.reply });
    }
    for (const ms of [10, 20]) ctx.timers.push(setTimeout(() => this.push(ctx, "agent.message.delta", i.correlationId, { chunk: "..." }), ms));
    if (this.defects.autoApprove) for (const a of this.approvalsList) a.state = "approved";
    return ok(res);
  }

  async contextCancel(i: { contextId: string }): Promise<AaiResult<CancelResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    if (this.defects.ignoreCancel) return ok({ confirmed: true });
    c.value.timers.forEach(clearTimeout);
    c.value.timers = [];
    return ok({ confirmed: true });
  }
  async contextClose(i: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    c.value.closed = true;
    return ok({ closed: true });
  }
  async events(i: EventsInput): Promise<AaiResult<EventsResult>> {
    const c = this.live(i.contextId);
    if (!c.ok) return c;
    const after = Number(i.cursor ?? 0);
    const events = c.value.events.filter((e) => Number(e.cursor) > after);
    return ok({ events, nextCursor: events.length ? events[events.length - 1].cursor : String(after) });
  }
  async memory(i: MemoryInput): Promise<AaiResult<MemoryResult>> {
    if (i.op === "write") { this.mem.set(i.key, i.value); return ok({}); }
    if (i.op === "read") return ok({ value: this.mem.get(i.key) });
    return super.memory(i);
  }
  async approvals(i: ApprovalsInput): Promise<AaiResult<ApprovalsResult>> {
    if (i.op === "list") return ok({ approvals: this.approvalsList.map((a) => ({ ...a })) });
    if (i.actor.type !== "human" && !this.defects.autoApprove) return fail("APPROVAL_REQUIRED", "human required");
    const a = this.approvalsList.find((x) => x.remoteRef === i.approvalId);
    if (!a) return fail("CONTEXT_NOT_FOUND", "no such approval");
    a.state = i.decision === "approve" ? "approved" : "denied";
    return ok({ resolved: { ...a } });
  }
}
