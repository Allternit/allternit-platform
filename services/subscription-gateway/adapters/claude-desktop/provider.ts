// Claude AAI provider (lane ui_bridge, guarantee best_effort, mode linked).
// Observes the renderer DOM through a ClaudeDesktopDriver (live CDP or offline replay) and emits normalized envelopes.
// Event guarantees are best_effort / inferred, NEVER exact. Drift and bot-detection latch the provider (stop);
// rate limits set a cooldown. Approvals are only ever answered by a human actor.
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type AAIError, type Approval,
  type ApprovalsInput, type ApprovalsResult, type CancelResult, type CursoredEvent, type EventsInput, type EventsResult, type GatewayEvent,
  type HealthResult, type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult,
} from "@allternit/agent-gateway";
import { ADAPTER_ID, AGENT_ID, APP_NAME, CAPABILITIES, COWORK_AGENT_ID, PACING } from "./manifest.js";
import { DriverError, type ClaudeDesktopDriver } from "./driver.js";
import { classify, type PageState } from "./observe.js";
import { NAMES, SELECTORS_VERSION } from "./selectors.js";

export interface ClaudeDesktopProviderOptions {
  driver: ClaudeDesktopDriver;
  /** false disables human-like gaps (tests/replay only). Default true. */
  pacing?: boolean;
  pollMs?: number;
  replyTimeoutMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  random?: () => number;
}

interface Ctx {
  id: string; threadId: string; closed: boolean;
  events: CursoredEvent[]; seq: number; baseline: number;
  seen: Map<number, { text: string; completed: boolean }>;
  cues: Set<string>; cowork: boolean; currentCorr?: string; msgN: number; activity: boolean;
  done: Map<string, Promise<AaiResult<MessageResult>>>; lock: Promise<unknown>;
}
interface Stored { approval: Approval; id: string }

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export class ClaudeDesktopProvider extends BaseAaiProvider {
  readonly adapterId = ADAPTER_ID;
  private ctxs = new Map<string, Ctx>();
  private n = 0;
  private halted: AAIError | undefined;
  private cooldownUntil = 0;
  private everLoggedIn = false;
  private sendTimes: number[] = [];
  private approvalsStore = new Map<string, Stored>();
  private o: Required<Omit<ClaudeDesktopProviderOptions, "driver">> & { driver: ClaudeDesktopDriver };

  constructor(opts: ClaudeDesktopProviderOptions) {
    super();
    this.o = {
      pacing: true, pollMs: 300, replyTimeoutMs: 120_000,
      now: Date.now, sleep: (ms) => new Promise((r) => setTimeout(r, ms)), random: Math.random, ...opts,
    };
  }

  /** After a selector-pack update / app fix, the user (or wizard) clears a latched drift/block. */
  clearHalt() { this.halted = undefined; }

  // ---------- identity ----------
  private summary(): AgentSummary { return { agentId: AGENT_ID, displayName: APP_NAME, vendor: "claude", state: this.halted ? "blocked" : "linked" }; }
  async list(): Promise<AaiResult<AgentSummary[]>> { return ok([this.summary()]); }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    if (agentId !== AGENT_ID) return fail("UNKNOWN", `No such agent ${agentId}`);
    return ok({ ...this.summary(), remoteIds: {}, capabilities: CAPABILITIES });
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    return agentId === AGENT_ID ? ok(CAPABILITIES) : fail("UNKNOWN", `No such agent ${agentId}`);
  }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    if (agentId !== AGENT_ID) return fail("UNKNOWN", `No such agent ${agentId}`);
    return ok({ agentId: AGENT_ID, displayName: APP_NAME, vendor: "claude", lookPack: "claude-desktop" });
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
        return fail("VENDOR_UNAVAILABLE", e.fault === "not_running" ? `${APP_NAME} is not running. Open it and sign in, then try again.` : e.message);
      }
      return fail("VENDOR_UNAVAILABLE", `Could not read ${APP_NAME}: ${(e as Error).message}`);
    }
    const st = classify(html);
    switch (st.kind) {
      case "ok": this.everLoggedIn = true; return ok(st);
      case "unreachable": return fail("VENDOR_UNAVAILABLE", `${APP_NAME} is not ready (${st.detail}).`);
      case "logged_out":
        return fail(this.everLoggedIn ? "AUTH_REVOKED" : "AUTH_REQUIRED", `${APP_NAME} is signed out. Sign in inside the app, then reconnect.`);
      case "rate_limited": {
        this.cooldownUntil = this.o.now() + (st.retryAfterMs ?? 60_000);
        if (allowRateLimited) return ok(st);
        return fail("RATE_LIMITED", `${APP_NAME} reports a rate or usage limit. Allternit will wait before sending again.`, { retryAfterMs: st.retryAfterMs, details: { banner: st.detail } });
      }
      case "blocked":
        this.halted = { code: "LANE_BLOCKED", retryable: false, humanMessage: `${APP_NAME} is asking for verification or reports unusual activity. Allternit has stopped; please resolve it in the app yourself.`, details: { banner: st.detail } };
        return { ok: false, error: this.halted };
      case "drift":
        this.halted = { code: "ADAPTER_DRIFT", retryable: false, humanMessage: `${APP_NAME}'s screen no longer matches what this adapter expects (selectors ${SELECTORS_VERSION}). Allternit has stopped driving it until the adapter is updated.`, details: { missing: st.missing, selectorsVersion: SELECTORS_VERSION, detail: st.detail } };
        return { ok: false, error: this.halted };
    }
  }
  private cooldown(): AaiResult<never> | undefined {
    const left = this.cooldownUntil - this.o.now();
    return left > 0 ? fail("RATE_LIMITED", `${APP_NAME} rate limit cooldown in effect.`, { retryAfterMs: left }) : undefined;
  }
  private async pace() {
    if (!this.o.pacing) return;
    const [lo, hi] = PACING.min_action_gap_ms;
    await this.o.sleep(lo + this.o.random() * (hi - lo));
  }

  // ---------- events ----------
  private push(c: Ctx, type: GatewayEvent["type"], guarantee: GatewayEvent["guarantee"], payload: Record<string, unknown>, source: GatewayEvent["source"] = "vendor", corr?: string) {
    c.seq += 1;
    const correlationId = corr ?? c.currentCorr ?? `obs-${c.id}`;
    c.events.push({
      cursor: String(c.seq),
      event: {
        type, botId: AGENT_ID, threadId: c.threadId, generationId: String(c.msgN), source, vendor: "claude", adapter: ADAPTER_ID, lane: "ui_bridge",
        remoteEventId: `${c.id}:${c.seq}`, remoteContextId: c.id, causationId: correlationId, correlationId, guarantee,
        at: new Date(this.o.now()).toISOString(), payload,
      },
    });
  }

  /** Diff a page snapshot into events (idempotent: repeated calls with the same page emit nothing). */
  private observe(c: Ctx, st: PageState) {
    st.turns.forEach((t, i) => {
      if (i < c.baseline || t.role !== "assistant") return;
      const prev = c.seen.get(i) ?? { text: "", completed: false };
      if (t.text !== prev.text) {
        if (!c.activity) { c.activity = true; this.push(c, "agent.activity.started", "inferred", { state: "streaming" }); }
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
    // Tool use and artifacts are only visible as cue blocks (markup unverified): surfaced as inferred tool.called with a kind.
    for (const cue of st.toolCues) if (!c.cues.has("t:" + cue)) { c.cues.add("t:" + cue); this.push(c, "agent.tool.called", "inferred", { kind: "tool_use", label: cue }); }
    for (const cue of st.artifactCues) if (!c.cues.has("a:" + cue)) { c.cues.add("a:" + cue); this.push(c, "agent.tool.called", "inferred", { kind: "artifact", label: cue }); }
    this.syncApprovals(st, c);
  }

  private syncApprovals(st: PageState, c?: Ctx) {
    const present = new Set(st.approvals.map((a) => a.id));
    for (const a of st.approvals) {
      const cur = this.approvalsStore.get(a.id);
      if (!cur || cur.approval.state !== "pending") {
        this.approvalsStore.set(a.id, { id: a.id, approval: { authority: "vendor", actor: AGENT_ID, action: a.text, threadId: c?.threadId ?? "", remoteRef: a.id, state: "pending" } });
        if (c) this.push(c, "agent.approval.requested", "inferred", { approvalId: a.id, action: a.text });
      }
    }
    for (const s of this.approvalsStore.values()) {
      if (s.approval.state === "pending" && !present.has(s.id)) {
        s.approval = { ...s.approval, state: "cancelled" }; // answered inside the app; outcome not observable
        if (c) this.push(c, "agent.approval.resolved", "inferred", { approvalId: s.id, outcome: "unknown", where: "in_app" });
      }
    }
  }

  // ---------- context ----------
  async contextOpen(input: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (input.adoptContextId) return fail("UNSUPPORTED", "Claude chats and Cowork tasks cannot be adopted; a new one is always started.");
    // One provider, two entry points on the same app: "claude-desktop" = new Chat, "claude-desktop:cowork" = new Cowork task.
    const [base, ...rest] = input.agentId.split(":");
    if (base !== AGENT_ID) return fail("CONTEXT_NOT_FOUND", `No such agent ${input.agentId}`);
    const sub = rest.join(":");
    if (sub && input.agentId !== COWORK_AGENT_ID) return fail("CONTEXT_NOT_FOUND", `No such agent ${input.agentId}`);
    const cowork = input.agentId === COWORK_AGENT_ID;
    if ([...this.ctxs.values()].some((c) => !c.closed)) return fail("CONTEXT_BUSY", `${APP_NAME} drives one conversation at a time. Close the open one first.`);
    const cd = this.cooldown(); if (cd) return cd;
    const g = await this.check(); if (!g.ok) return g;
    await this.pace();
    if (cowork) {
      if (!g.value.cowork && !(await this.o.driver.clickButton(NAMES.cowork))) return fail("CONTEXT_NOT_FOUND", `${APP_NAME} has no Cowork mode on this account or version.`);
      await this.pace();
      if (!(await this.o.driver.clickButton(NAMES.newTask))) return fail("CONTEXT_NOT_FOUND", `${APP_NAME} Cowork has no New task control.`);
    } else {
      await this.o.driver.newChat();
    }
    const after = await this.check(); if (!after.ok) return after;
    const id = `cd-ctx-${++this.n}`;
    const c: Ctx = { id, threadId: input.threadId ?? id, closed: false, events: [], seq: 0, baseline: after.value.turns.length, seen: new Map(), cues: new Set(), cowork, msgN: 0, activity: false, done: new Map(), lock: Promise.resolve() };
    this.ctxs.set(id, c);
    this.push(c, "agent.context.opened", "best_effort", { title: input.title ?? null, mode: cowork ? "cowork" : "chat" }, "allternit", id);
    return ok({ contextId: id, isolation: CAPABILITIES.context.isolation, guarantee: CAPABILITIES.guarantee, resumed: false });
  }

  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    return c && !c.closed ? ok(c) : fail("CONTEXT_NOT_FOUND", "No such open Claude conversation.");
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
    const now = this.o.now();
    this.sendTimes = this.sendTimes.filter((t) => now - t < 86_400_000);
    const hour = this.sendTimes.filter((t) => now - t < 3_600_000).length;
    if (hour >= PACING.max_tasks_per_hour) return fail("RATE_LIMITED", "Allternit's own hourly pacing limit for Claude was reached.", { retryAfterMs: 600_000 });
    if (this.sendTimes.length >= PACING.max_tasks_per_day) return fail("RATE_LIMITED", "Allternit's own daily pacing limit for Claude was reached.", { retryAfterMs: 3_600_000 });
    const g = await this.check(); if (!g.ok) return g;
    const sendIndex = g.value.turns.length;
    if (this.o.pacing && this.sendTimes.length) await this.o.sleep(Math.max(0, PACING.min_task_gap_s * 1000 - (now - this.sendTimes[this.sendTimes.length - 1])));
    await this.pace();
    if (!(await this.o.driver.typeText(input.text))) return this.drift("composer");
    await this.pace();
    if (!(await this.o.driver.clickButton(NAMES.send))) return this.drift("send_button");
    this.sendTimes.push(this.o.now());
    c.msgN += 1; c.currentCorr = input.correlationId;
    this.push(c, "agent.activity.started", "best_effort", { state: "sent" }, "allternit", input.correlationId);
    const messageId = `cd-msg-${c.id}-${c.msgN}`;
    const deadline = this.o.now() + this.o.replyTimeoutMs;
    let reply: string | undefined;
    for (;;) {
      const st = await this.check(true); if (!st.ok) return st;
      this.observe(c, st.value);
      const a = st.value.turns.slice(sendIndex + 1).find((t) => t.role === "assistant");
      if (a && !st.value.streaming && a.text) { reply = a.text; break; }
      if (this.o.now() >= deadline) break; // reply still streaming: caller keeps reading events()
      await this.o.sleep(this.o.pollMs);
    }
    return ok({ messageId, correlationId: input.correlationId, reply, guarantee: CAPABILITIES.guarantee });
  }
  private drift(missing: string): AaiResult<never> {
    this.halted = { code: "ADAPTER_DRIFT", retryable: false, humanMessage: `${APP_NAME}'s ${missing.replace("_", " ")} control was not found. Allternit has stopped driving it until the adapter is updated.`, details: { missing: [missing], selectorsVersion: SELECTORS_VERSION } };
    return { ok: false, error: this.halted };
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
    if (!c) return fail("CONTEXT_NOT_FOUND", "No such Claude conversation.");
    c.closed = true; // UI is left as-is: closing never deletes the user's chat
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

  // ---------- approvals ----------
  async approvals(input: ApprovalsInput): Promise<AaiResult<ApprovalsResult>> {
    if (input.op === "list") {
      const g = await this.check(true); if (!g.ok) return g;
      const c = input.contextId ? this.ctxs.get(input.contextId) : [...this.ctxs.values()].find((x) => !x.closed);
      this.syncApprovals(g.value, c);
      return ok({ approvals: [...this.approvalsStore.values()].map((s) => s.approval) });
    }
    if (input.actor.type !== "human") return fail("APPROVAL_REQUIRED", "Only a person can answer a Claude permission prompt. Allternit never auto-approves.");
    const s = this.approvalsStore.get(input.approvalId);
    if (!s) return fail("UNKNOWN", "No such approval is known. List approvals first.");
    if (s.approval.state !== "pending") return fail("SYNC_CONFLICT", `That approval is already ${s.approval.state}.`);
    const g = await this.check(true); if (!g.ok) return g;
    if (!g.value.approvals.some((a) => a.id === s.id)) { this.syncApprovals(g.value); return fail("SYNC_CONFLICT", "That approval is no longer showing in Claude."); }
    const clicked = await this.o.driver.clickButton(input.decision === "approve" ? NAMES.approve : NAMES.deny, { withinText: escapeRe(s.approval.action.slice(0, 60)) });
    if (!clicked) return this.drift("approval_button");
    s.approval = { ...s.approval, state: input.decision === "approve" ? "approved" : "denied" };
    const c = [...this.ctxs.values()].find((x) => !x.closed);
    if (c) this.push(c, "agent.approval.resolved", "inferred", { approvalId: s.id, outcome: s.approval.state, actorId: input.actor.id }, "allternit");
    return ok({ resolved: s.approval });
  }

  // ---------- health ----------
  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const g = await this.check(true);
    if (!g.ok) return ok({ status: "down", lane: "ui_bridge", detail: `${g.error.code}: ${g.error.humanMessage}` });
    return ok({ status: this.cooldownUntil > this.o.now() ? "degraded" : "healthy", lane: "ui_bridge", detail: this.cooldownUntil > this.o.now() ? "rate limit cooldown" : undefined });
  }
}
