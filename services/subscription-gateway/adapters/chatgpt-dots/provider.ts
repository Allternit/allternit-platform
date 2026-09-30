// ChatGPT dots AAI provider (lane ui_bridge, guarantee best_effort, mode linked).
// Observes the ChatGPT web page through a DotsDriver (live browser or offline replay) and emits normalized envelopes.
// Event guarantees are best_effort / inferred, NEVER exact. Drift and challenges latch the provider (stop); rate limits
// set a cooldown. Approvals are only ever answered by a human actor; "Hand off" items are never answerable from here.
// Pacing reuses the SDK Pacer (same caps machinery chatgpt-web's worker uses), fed chatgpt-web's own pacing profile.
import { createPacer, PacingCapExceeded } from "@allternit/subscription-adapter-sdk";
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type AAIError, type Approval,
  type ApprovalsInput, type ApprovalsResult, type CancelResult, type CursoredEvent, type EventsInput, type EventsResult, type GatewayEvent,
  type HealthResult, type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult, type TaskInfo,
} from "@allternit/agent-gateway";
import { ADAPTER_ID, AGENT_ID, APP_NAME, CAPABILITIES, PACING } from "./manifest.js";
import { DriverError, type DotsDriver } from "./driver.js";
import { classify, type DotRef, type PageState, type Policy } from "./observe.js";
import { NAMES, SELECTORS_VERSION } from "./selectors.js";

export interface ChatGPTDotsProviderOptions {
  driver: DotsDriver;
  /** false disables human-like waits (tests/replay only; caps still apply). Default true. */
  pacing?: boolean;
  /** Dot to open when the binding names none (dot id or display name). With exactly one dot it is picked automatically. */
  dotRef?: string;
  pollMs?: number;
  replyTimeoutMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  random?: () => number;
}

interface Ctx {
  id: string; agentId: string; dotId: string; dotName: string; threadId: string; closed: boolean;
  events: CursoredEvent[]; seq: number; baseline: number;
  seen: Map<number, { text: string; completed: boolean }>;
  tasks: Map<string, string>; currentCorr?: string; msgN: number; activity: boolean;
  done: Map<string, Promise<AaiResult<MessageResult>>>; lock: Promise<unknown>;
}
interface Stored { approval: Approval; id: string; policy: Policy; ctxId?: string }

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const LABEL: Record<Policy, string> = { ask_first: "Ask first", hand_off: "Hand off" };

export class ChatGPTDotsProvider extends BaseAaiProvider {
  readonly adapterId = ADAPTER_ID;
  private ctxs = new Map<string, Ctx>();
  private n = 0;
  private halted: AAIError | undefined;
  private cooldownUntil = 0;
  private everLoggedIn = false;
  private approvalsStore = new Map<string, Stored>();
  private pacer: ReturnType<typeof createPacer>;
  private o: Required<Omit<ChatGPTDotsProviderOptions, "driver">> & { driver: DotsDriver };

  constructor(opts: ChatGPTDotsProviderOptions) {
    super();
    this.o = { pacing: true, dotRef: "", pollMs: 300, replyTimeoutMs: 120_000, now: Date.now, sleep: (ms) => new Promise((r) => setTimeout(r, ms)), random: Math.random, ...opts };
    this.pacer = createPacer(PACING, { now: this.o.now, rng: this.o.random, sleep: this.o.pacing ? this.o.sleep : async () => {} });
  }

  /** After a selector-pack update / the user resolving a challenge, clear a latched drift/block. */
  clearHalt() { this.halted = undefined; }

  // ---------- identity ----------
  private state(): "blocked" | "linked" { return this.halted ? "blocked" : "linked"; }
  private summary(d: DotRef): AgentSummary { return { agentId: `${AGENT_ID}:${d.id}`, displayName: d.name, vendor: "openai", state: this.state() }; }
  private isOurs(agentId: string) { return agentId === AGENT_ID || agentId.startsWith(`${AGENT_ID}:`); }
  private refOf(agentId: string) { return agentId.slice(AGENT_ID.length + 1) || this.o.dotRef; }

  /** The user's dots, read from the dots list view. */
  async list(): Promise<AaiResult<AgentSummary[]>> {
    if ([...this.ctxs.values()].some((c) => !c.closed)) { // never navigate away from a live conversation: answer from what we know
      return ok([...this.ctxs.values()].filter((c) => !c.closed).map((c) => this.summary({ id: c.dotId, name: c.dotName })));
    }
    const dots = await this.readDots(); if (!dots.ok) return dots;
    return ok(dots.value.map((d) => this.summary(d)));
  }
  private async readDots(): Promise<AaiResult<DotRef[]>> {
    const g = await this.check(); if (!g.ok) return g;
    await this.pacer.beforeAction();
    if (!(await this.o.driver.showDotList())) return fail("VENDOR_UNAVAILABLE", `${APP_NAME}: could not open the dots list.`);
    const st = await this.check(); if (!st.ok) return st;
    return ok(st.value.dots);
  }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    if (!this.isOurs(agentId)) return fail("UNKNOWN", `No such agent ${agentId}`);
    return ok({ agentId, displayName: this.refOf(agentId) || APP_NAME, vendor: "openai", state: this.state(), remoteIds: {}, capabilities: CAPABILITIES });
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    return this.isOurs(agentId) ? ok(CAPABILITIES) : fail("UNKNOWN", `No such agent ${agentId}`);
  }
  /** Dot name / handle / avatar from the open dot's header, falling back to the dots list row. */
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    if (!this.isOurs(agentId)) return fail("UNKNOWN", `No such agent ${agentId}`);
    const open = [...this.ctxs.values()].find((c) => !c.closed);
    let name = open?.dotName ?? "", avatar: string | undefined, handle: string | undefined;
    if (open) {
      const g = await this.check(true);
      if (g.ok && g.value.header) { name = g.value.header.name || name; avatar = g.value.header.avatar; handle = g.value.header.handle; }
    } else {
      const dots = await this.readDots();
      if (dots.ok) { const d = this.pick(dots.value, this.refOf(agentId)); if (d) ({ name, avatar, handle } = { name: d.name, avatar: d.avatar, handle: d.handle }); }
    }
    return ok({ agentId, displayName: name || APP_NAME, vendor: "openai", lookPack: "chatgpt-dots", ...(avatar ? { avatarUrl: avatar } : {}), ...(handle ? { handle } : {}) } as AgentIdentity);
  }
  private pick(dots: DotRef[], ref: string): DotRef | undefined {
    if (!ref) return dots.length === 1 ? dots[0] : undefined;
    const r = ref.toLowerCase();
    return dots.find((d) => d.id.toLowerCase() === r) ?? dots.find((d) => d.name.toLowerCase() === r) ?? dots.find((d) => d.handle?.toLowerCase() === r);
  }

  // ---------- page gate ----------
  private async check(allowRateLimited = false): Promise<AaiResult<PageState>> {
    if (this.halted) return { ok: false, error: this.halted };
    let html: string;
    try {
      await this.o.driver.connect();
      html = await this.o.driver.html();
    } catch (e) {
      if (e instanceof DriverError) {
        if (e.fault === "consent_required") return fail("LANE_BLOCKED", e.message);
        return fail("VENDOR_UNAVAILABLE", e.fault === "not_running" ? `${APP_NAME}: the ChatGPT browser session is not open. Connect it and sign in, then try again.` : e.message);
      }
      return fail("VENDOR_UNAVAILABLE", `Could not read ChatGPT: ${(e as Error).message}`);
    }
    const st = classify(html);
    switch (st.kind) {
      case "ok": this.everLoggedIn = true; return ok(st);
      case "unreachable": return fail("VENDOR_UNAVAILABLE", `ChatGPT is not ready (${st.detail}).`);
      case "logged_out":
        return fail(this.everLoggedIn ? "AUTH_REVOKED" : "AUTH_REQUIRED", "ChatGPT is signed out. Sign in in the ChatGPT browser window yourself, then reconnect.");
      case "rate_limited": {
        this.cooldownUntil = this.o.now() + (st.retryAfterMs ?? 60_000);
        if (allowRateLimited) return ok(st);
        return fail("RATE_LIMITED", "ChatGPT reports a usage limit. Allternit will wait before sending again.", { retryAfterMs: st.retryAfterMs, details: { banner: st.detail } });
      }
      case "blocked":
        this.halted = { code: "LANE_BLOCKED", retryable: false, humanMessage: "ChatGPT is asking for verification or reports unusual activity. Allternit has stopped and will not attempt it; please resolve it in the browser yourself.", details: { banner: st.detail } };
        return { ok: false, error: this.halted };
      case "paused":
        return fail("LANE_BLOCKED", "ChatGPT shows this dot as paused. Resume it in ChatGPT yourself; Allternit will not.", { details: { banner: st.detail } });
      case "drift":
        this.halted = { code: "ADAPTER_DRIFT", retryable: false, humanMessage: `ChatGPT's screen no longer matches what this adapter expects (selectors ${SELECTORS_VERSION}). Allternit has stopped driving it until the adapter is updated.`, details: { missing: st.missing, selectorsVersion: SELECTORS_VERSION, detail: st.detail } };
        return { ok: false, error: this.halted };
    }
  }
  private cooldown(): AaiResult<never> | undefined {
    const left = this.cooldownUntil - this.o.now();
    return left > 0 ? fail("RATE_LIMITED", "ChatGPT usage-limit cooldown in effect.", { retryAfterMs: left }) : undefined;
  }
  private drift(missing: string): AaiResult<never> {
    this.halted = { code: "ADAPTER_DRIFT", retryable: false, humanMessage: `ChatGPT's ${missing.replace("_", " ")} control was not found. Allternit has stopped driving it until the adapter is updated.`, details: { missing: [missing], selectorsVersion: SELECTORS_VERSION } };
    return { ok: false, error: this.halted };
  }

  // ---------- events ----------
  private push(c: Ctx, type: GatewayEvent["type"], guarantee: GatewayEvent["guarantee"], payload: Record<string, unknown>, source: GatewayEvent["source"] = "vendor", corr?: string) {
    c.seq += 1;
    const correlationId = corr ?? c.currentCorr ?? `obs-${c.id}`;
    c.events.push({
      cursor: String(c.seq),
      event: {
        type, botId: c.agentId, threadId: c.threadId, generationId: String(c.msgN), source, vendor: "openai", adapter: ADAPTER_ID, lane: "ui_bridge",
        remoteEventId: `${c.id}:${c.seq}`, remoteContextId: c.id, causationId: correlationId, correlationId, guarantee,
        at: new Date(this.o.now()).toISOString(), payload,
      },
    });
  }

  /** Diff a page snapshot into events (idempotent: repeated calls with the same page emit nothing). */
  private observe(c: Ctx, st: PageState) {
    if (st.view !== "dot") return;
    st.turns.forEach((t, i) => {
      if (i < c.baseline || t.role !== "assistant") return;
      const prev = c.seen.get(i) ?? { text: "", completed: false };
      if (t.text !== prev.text) {
        if (!c.activity) { c.activity = true; this.push(c, "agent.activity.started", "inferred", { state: "streaming", ...(st.activity ? { label: st.activity } : {}) }); }
        const grew = t.text.startsWith(prev.text);
        this.push(c, "agent.message.delta", "best_effort", { text: grew ? t.text.slice(prev.text.length) : t.text, turnIndex: i, ...(grew ? {} : { replace: true }) });
        prev.text = t.text;
      }
      const last = i === st.turns.length - 1;
      if (!prev.completed && !(last && st.streaming) && t.text) {
        prev.completed = true;
        this.push(c, "agent.message.completed", "inferred", { text: t.text, turnIndex: i });
        c.activity = false;
      }
      c.seen.set(i, prev);
    });
    for (const t of st.tasks) {
      if (c.tasks.get(t.id) === t.state) continue;
      c.tasks.set(t.id, t.state);
      this.push(c, "agent.task.updated", "inferred", { taskId: t.id, title: t.title, state: t.state });
    }
    this.syncApprovals(st, c);
  }

  private syncApprovals(st: PageState, c?: Ctx) {
    const present = new Set(st.confirmations.map((a) => a.id));
    for (const a of st.confirmations) {
      const cur = this.approvalsStore.get(a.id);
      if (!cur || cur.approval.state !== "pending") {
        const action = `${LABEL[a.policy]}: ${a.text}`;
        this.approvalsStore.set(a.id, { id: a.id, policy: a.policy, ctxId: c?.id, approval: { authority: "vendor", actor: c?.agentId ?? AGENT_ID, action, threadId: c?.threadId ?? "", remoteRef: a.id, state: "pending" } });
        // Ask first = the dot wants a yes/no. Hand off = the dot cannot proceed; it needs the person to do it (needsYou).
        if (c) this.push(c, "agent.approval.requested", "inferred", { approvalId: a.id, action, policy: a.policy, label: LABEL[a.policy], needsYou: a.policy === "hand_off" });
      }
    }
    // A card can vanish because it was answered in ChatGPT (or a dot page was left); only judge from a dot view.
    if (st.view !== "dot") return;
    for (const s of this.approvalsStore.values()) {
      if (s.approval.state === "pending" && !present.has(s.id)) {
        s.approval = { ...s.approval, state: "cancelled" };
        if (c) this.push(c, "agent.approval.resolved", "inferred", { approvalId: s.id, outcome: "unknown", where: "in_app", policy: s.policy });
      }
    }
  }

  // ---------- context ----------
  async contextOpen(input: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (input.adoptContextId) return fail("UNSUPPORTED", "A dot's conversation cannot be adopted by id; the dot's own conversation is opened.");
    // One provider serves every dot: the binding's externalAgentId is "chatgpt-dots:<dot id or name>".
    if (!this.isOurs(input.agentId)) return fail("CONTEXT_NOT_FOUND", `No such agent ${input.agentId}`);
    if ([...this.ctxs.values()].some((c) => !c.closed)) return fail("CONTEXT_BUSY", `${APP_NAME} drives one conversation at a time. Close the open one first.`);
    const cd = this.cooldown(); if (cd) return cd;
    const dots = await this.readDots(); if (!dots.ok) return dots;
    const ref = this.refOf(input.agentId);
    const dot = this.pick(dots.value, ref);
    if (!dot) {
      if (!ref && dots.value.length > 1) return fail("POLICY_DENIED", `You have ${dots.value.length} dots. Choose which one this connection uses.`);
      return fail("CONTEXT_NOT_FOUND", ref ? `No dot named "${ref}" in your ChatGPT account.` : "No dot found in your ChatGPT account (a plan with a dot is required, and dots are created in the desktop app).");
    }
    await this.pacer.beforeAction();
    if (!(await this.o.driver.openDot(dot.id))) return fail("CONTEXT_NOT_FOUND", `Could not open dot "${dot.name}".`);
    const after = await this.check(); if (!after.ok) return after;
    if (after.value.view !== "dot") return fail("CONTEXT_NOT_FOUND", `Dot "${dot.name}" did not open.`);
    const id = `cd-ctx-${++this.n}`;
    const c: Ctx = {
      id, agentId: `${AGENT_ID}:${dot.id}`, dotId: dot.id, dotName: dot.name, threadId: input.threadId ?? id, closed: false, events: [], seq: 0,
      baseline: after.value.turns.length, seen: new Map(), tasks: new Map(), msgN: 0, activity: false, done: new Map(), lock: Promise.resolve(),
    };
    this.ctxs.set(id, c);
    this.push(c, "agent.context.opened", "best_effort", { title: input.title ?? null, dot: dot.name }, "allternit", id);
    this.syncApprovals(after.value, c);
    return ok({ contextId: id, isolation: CAPABILITIES.context.isolation, guarantee: CAPABILITIES.guarantee, resumed: false });
  }

  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    return c && !c.closed ? ok(c) : fail("CONTEXT_NOT_FOUND", "No such open dot conversation.");
  }

  async contextMessage(input: MessageInput): Promise<AaiResult<MessageResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const prior = c.done.get(input.correlationId);
    if (prior) return prior; // idempotent: replays (sequential or concurrent) return the first result
    const run = c.lock.then(() => this.sendOne(c, input));
    c.lock = run.catch(() => undefined);
    c.done.set(input.correlationId, run);
    return run;
  }

  private async sendOne(c: Ctx, input: MessageInput): Promise<AaiResult<MessageResult>> {
    const cd = this.cooldown(); if (cd) return cd;
    const g = await this.check(); if (!g.ok) return g;
    try { await this.pacer.beforeTask(); }
    catch (e) {
      if (e instanceof PacingCapExceeded) return fail("RATE_LIMITED", `Allternit's own pacing limit for ChatGPT (${e.cap.replace(/_/g, " ")}) was reached.`, { retryAfterMs: (e.retryAfterS ?? 600) * 1000 });
      throw e;
    }
    const sendIndex = g.value.turns.length;
    await this.pacer.beforeAction();
    if (!(await this.o.driver.typeText(input.text))) return this.drift("composer");
    await this.pacer.beforeAction();
    if (!(await this.o.driver.clickButton(NAMES.send))) return this.drift("send_button");
    c.msgN += 1; c.currentCorr = input.correlationId;
    this.push(c, "agent.activity.started", "best_effort", { state: "sent" }, "allternit", input.correlationId);
    const messageId = `cd-msg-${c.id}-${c.msgN}`;
    const deadline = this.o.now() + this.o.replyTimeoutMs;
    let reply: string | undefined;
    for (;;) {
      const st = await this.check(true); if (!st.ok) return st;
      this.observe(c, st.value);
      const a = st.value.turns.slice(sendIndex + 1).find((t) => t.role === "assistant");
      if (a && a.text && !st.value.streaming) { reply = a.text; break; }
      // A dot waiting on an Ask first / Hand off card may never finish: the deadline returns with no reply and the card is in events().
      if (this.o.now() >= deadline) break; // reply still streaming: caller keeps reading events()
      await this.o.sleep(this.o.pollMs);
    }
    return ok({ messageId, correlationId: input.correlationId, reply, guarantee: CAPABILITIES.guarantee });
  }

  async contextCancel(input: { contextId: string }): Promise<AaiResult<CancelResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const g = await this.check(true); if (!g.ok) return g;
    if (g.value.streaming) await this.o.driver.clickButton(NAMES.stop);
    const after = await this.check(true); if (!after.ok) return after;
    this.observe(l.value, after.value);
    return ok({ confirmed: !after.value.streaming });
  }

  async contextClose(input: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const c = this.ctxs.get(input.contextId);
    if (!c) return fail("CONTEXT_NOT_FOUND", "No such dot conversation.");
    c.closed = true; // UI is left as-is: closing never deletes the dot's conversation, pauses it, or resets its memory
    return ok({ closed: true });
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const g = await this.check(true); if (!g.ok) return g;
    this.observe(c, g.value);
    const from = Number(input.cursor ?? 0) || 0;
    const evs = c.events.filter((e) => Number(e.cursor) > from).slice(0, input.limit ?? 500);
    return ok({ events: evs, nextCursor: evs.length ? evs[evs.length - 1].cursor : String(from) });
  }

  // ---------- tasks (read-only, from the dot's profile panel) ----------
  async tasks(input: { agentId: string }): Promise<AaiResult<TaskInfo[]>> {
    if (!this.isOurs(input.agentId)) return fail("UNKNOWN", `No such agent ${input.agentId}`);
    const open = [...this.ctxs.values()].find((c) => !c.closed);
    const ref = this.refOf(input.agentId);
    if (open && ref && ref !== open.dotId && ref.toLowerCase() !== open.dotName.toLowerCase()) return fail("CONTEXT_BUSY", `${APP_NAME} is busy with another dot's conversation.`);
    if (!open) {
      const dots = await this.readDots(); if (!dots.ok) return dots;
      const dot = this.pick(dots.value, ref);
      if (!dot) return fail("CONTEXT_NOT_FOUND", ref ? `No dot named "${ref}".` : "Could not tell which dot to read tasks for.");
      await this.pacer.beforeAction();
      if (!(await this.o.driver.openDot(dot.id))) return fail("CONTEXT_NOT_FOUND", `Could not open dot "${dot.name}".`);
    }
    await this.pacer.beforeAction();
    await this.o.driver.showTasks();
    const g = await this.check(true); if (!g.ok) return g;
    if (open) this.observe(open, g.value);
    return ok(g.value.tasks.map((t) => ({ taskId: t.id, title: t.title, state: t.state })));
  }

  // ---------- approvals ----------
  async approvals(input: ApprovalsInput): Promise<AaiResult<ApprovalsResult>> {
    if (input.op === "list") {
      const g = await this.check(true); if (!g.ok) return g;
      const c = input.contextId ? this.ctxs.get(input.contextId) : [...this.ctxs.values()].find((x) => !x.closed);
      this.syncApprovals(g.value, c);
      return ok({ approvals: [...this.approvalsStore.values()].map((s) => s.approval) });
    }
    if (input.actor.type !== "human") return fail("APPROVAL_REQUIRED", "Only a person can answer a dot's Ask first or Hand off item. Allternit never auto-approves.");
    const s = this.approvalsStore.get(input.approvalId);
    if (!s) return fail("UNKNOWN", "No such approval is known. List approvals first.");
    if (s.approval.state !== "pending") return fail("SYNC_CONFLICT", `That approval is already ${s.approval.state}.`);
    if (s.policy === "hand_off") return fail("POLICY_DENIED", "This is a Hand off item: the dot has handed it to you (for example a password change or moving money). Do it yourself in ChatGPT; Allternit cannot.");
    const g = await this.check(true); if (!g.ok) return g;
    if (!g.value.confirmations.some((a) => a.id === s.id)) { this.syncApprovals(g.value); return fail("SYNC_CONFLICT", "That item is no longer showing in ChatGPT."); }
    const clicked = await this.o.driver.clickButton(input.decision === "approve" ? NAMES.approve : NAMES.deny, { withinText: escapeRe(s.approval.action.replace(/^Ask first: /, "").slice(0, 60)) });
    if (!clicked) return this.drift("approval_button");
    s.approval = { ...s.approval, state: input.decision === "approve" ? "approved" : "denied" };
    const c = s.ctxId ? this.ctxs.get(s.ctxId) : undefined;
    if (c) this.push(c, "agent.approval.resolved", "inferred", { approvalId: s.id, outcome: s.approval.state, actorId: input.actor.id, policy: s.policy }, "allternit");
    return ok({ resolved: s.approval });
  }

  // ---------- health ----------
  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const g = await this.check(true);
    if (!g.ok) return ok({ status: "down", lane: "ui_bridge", detail: `${g.error.code}: ${g.error.humanMessage}` });
    const cool = this.cooldownUntil > this.o.now();
    return ok({ status: cool ? "degraded" : "healthy", lane: "ui_bridge", detail: cool ? "usage-limit cooldown" : undefined });
  }
}
