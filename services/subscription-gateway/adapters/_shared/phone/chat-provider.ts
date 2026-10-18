// AAI provider for an AI chat app on a Phone-Linked phone (routes android-app / iphone-app-ictrl).
// It drives the user's own signed-in app through Allternit Desktop's phone tools (open_app,
// read_ui, find_and_tap, type) — no vendor login, cookies or tokens. One spec per (vendor,
// platform) names the app id and the controls; everything else is shared.
//
// Finished means the stop control is gone and the send/composer control is back, with the
// reply text unchanged across two reads — never just "the screen stopped changing".
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type AAIError,
  type CancelResult, type CursoredEvent, type EventsInput, type EventsResult, type GatewayEvent,
  type HealthResult, type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult,
} from "@allternit/agent-gateway";
import { linkedPhones, type PhoneDevice, type PhoneGateway, type PhoneRoute, type PhoneToolResult } from "../../../src/phone/gateway.js";
import { findByLabel, nodeText, parseUiTree, type UiNode } from "./ui-tree.js";

export interface PhoneChatAppSpec {
  vendor: string;
  adapterId: string;
  agentId: string;
  appName: string;
  route: PhoneRoute;
  platform: "ios" | "android";
  /** Bundle id (iOS) or package (Android) passed to phone.open_app. */
  appId: string;
  /** Label of the message field (find_and_tap target). */
  composer: string;
  /** Matches the send control's label or id. */
  send: RegExp;
  /** Matches the stop-generating control while a reply streams. */
  stop: RegExp;
  /** Matches a "new chat" control, when the app has one on the main screen. */
  newChat?: RegExp;
  /** Matches a control only shown when signed out (e.g. "Log in"). */
  signedOut: RegExp;
  /** Matches on-screen text for a usage/rate limit. */
  rateLimit: RegExp;
  /** Matches on-screen text for a verification wall / unusual-activity block. */
  verification: RegExp;
  /** Text that is app chrome, never part of a reply. */
  chrome: RegExp;
  capabilities: AgentCapabilityManifest;
}

export interface PhoneChatProviderOptions {
  gateway: PhoneGateway;
  pollMs?: number;
  replyTimeoutMs?: number;
  /** Gap after taps so the app's screen settles. */
  settleMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

interface Ctx {
  id: string;
  threadId: string;
  closed: boolean;
  serial: string;
  events: CursoredEvent[];
  seq: number;
  msgN: number;
  done: Map<string, Promise<AaiResult<MessageResult>>>;
  lock: Promise<unknown>;
}

export class PhoneChatProvider extends BaseAaiProvider {
  readonly adapterId: string;
  private ctxs = new Map<string, Ctx>();
  private n = 0;
  private halted: AAIError | undefined;
  private o: Required<PhoneChatProviderOptions>;

  constructor(readonly spec: PhoneChatAppSpec, opts: PhoneChatProviderOptions) {
    super();
    this.adapterId = spec.adapterId;
    this.o = { pollMs: 1500, replyTimeoutMs: 180_000, settleMs: 800, now: Date.now, sleep: (ms) => new Promise((r) => setTimeout(r, ms)), ...opts };
  }

  clearHalt() { this.halted = undefined; }

  // ---------- identity ----------
  private summary(): AgentSummary {
    return { agentId: this.spec.agentId, displayName: this.spec.appName, vendor: this.spec.vendor, state: this.halted ? "blocked" : "linked" };
  }
  async list(): Promise<AaiResult<AgentSummary[]>> { return ok([this.summary()]); }
  async get(agentId: string): Promise<AaiResult<AgentDetail>> {
    if (agentId !== this.spec.agentId) return fail("UNKNOWN", `No such agent ${agentId}`);
    return ok({ ...this.summary(), remoteIds: {}, capabilities: this.spec.capabilities });
  }
  async capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>> {
    return agentId === this.spec.agentId ? ok(this.spec.capabilities) : fail("UNKNOWN", `No such agent ${agentId}`);
  }
  async identity(agentId: string): Promise<AaiResult<AgentIdentity>> {
    if (agentId !== this.spec.agentId) return fail("UNKNOWN", `No such agent ${agentId}`);
    return ok({ agentId: this.spec.agentId, displayName: this.spec.appName, vendor: this.spec.vendor, lookPack: this.spec.vendor });
  }

  // ---------- phone access ----------
  /** The linked phone this route uses: the first online one for this route. */
  private async phone(): Promise<AaiResult<PhoneDevice>> {
    const devices = await linkedPhones(this.o.gateway);
    const mine = devices.filter((d) => d.route === this.spec.route && d.platform === this.spec.platform);
    const online = mine.find((d) => !d.health?.state || d.health.state === "HEALTHY" || d.health.state === "DEVICE_LOCKED");
    if (online) return ok(online);
    if (mine.length) {
      const h = mine[0].health;
      return fail("VENDOR_UNAVAILABLE", h?.action ?? `Your ${this.spec.platform === "ios" ? "iPhone" : "Android phone"} isn't reachable right now (${h?.state ?? "offline"}).`);
    }
    return fail("VENDOR_UNAVAILABLE", `No ${this.spec.platform === "ios" ? "iPhone" : "Android phone"} is linked. Link one in Allternit Desktop › Settings › Subscriptions › Link a phone.`);
  }

  /** A phone tool error, typed for AAI. */
  private toolError(r: Extract<PhoneToolResult, { ok: false }>): AaiResult<never> {
    switch (r.error) {
      case "device_locked": return fail("VENDOR_UNAVAILABLE", "Unlock your phone, then try again.", { retryAfterMs: 30_000 });
      case "pairing_invalid": return fail("AUTH_REQUIRED", "Pair your phone again in Allternit Desktop › Settings › Subscriptions › Linked phones.");
      case "device_offline":
      case "desktop_unavailable": return fail("VENDOR_UNAVAILABLE", r.message);
      case "unknown_app": return fail("AUTH_REQUIRED", `${this.spec.appName} isn't installed on your phone. Install it and sign in, then try again.`);
      default: return fail("VENDOR_UNAVAILABLE", r.message);
    }
  }

  private async tool(name: string, args: Record<string, unknown>): Promise<AaiResult<PhoneToolResult & { ok: true }>> {
    const r = await this.o.gateway.call(name, args);
    if (!r.ok) return this.toolError(r);
    return ok(r);
  }

  private async screen(serial: string): Promise<AaiResult<UiNode[]>> {
    const r = await this.tool("phone.read_ui", { serial, visible_only: true });
    if (!r.ok) return r;
    return ok(parseUiTree(String((r.value as { text?: unknown }).text ?? "")));
  }

  /** Typed screen states the contract requires: signed out, usage limit, verification wall. */
  private gate(nodes: UiNode[]): AaiResult<UiNode[]> {
    if (this.halted) return { ok: false, error: this.halted };
    const texts = nodes.map(nodeText).filter((t): t is string => !!t);
    if (texts.some((t) => this.spec.verification.test(t))) {
      this.halted = { code: "LANE_BLOCKED", retryable: false, humanMessage: `${this.spec.appName} on your phone is asking for verification. Allternit has stopped; please resolve it in the app yourself.` };
      return { ok: false, error: this.halted };
    }
    if (findByLabel(nodes, this.spec.signedOut)) return fail("AUTH_REQUIRED", `${this.spec.appName} on your phone is signed out. Sign in inside the app, then try again.`);
    if (texts.some((t) => this.spec.rateLimit.test(t))) return fail("RATE_LIMITED", `${this.spec.appName} reports a usage limit. Allternit will wait before sending again.`, { retryAfterMs: 3_600_000 });
    return ok(nodes);
  }

  private push(c: Ctx, type: GatewayEvent["type"], payload: Record<string, unknown>, corr: string) {
    c.seq += 1;
    c.events.push({
      cursor: String(c.seq),
      event: {
        type, botId: this.spec.agentId, threadId: c.threadId, generationId: String(c.msgN), source: type === "agent.context.opened" ? "allternit" : "vendor",
        vendor: this.spec.vendor, adapter: this.spec.adapterId, lane: "ui_bridge", remoteEventId: `${c.id}:${c.seq}`, remoteContextId: c.id,
        causationId: corr, correlationId: corr, guarantee: "best_effort", at: new Date(this.o.now()).toISOString(), payload,
      },
    });
  }

  // ---------- context ----------
  async contextOpen(input: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (input.adoptContextId) return fail("UNSUPPORTED", `${this.spec.appName} chats on a phone cannot be adopted; a new one is started.`);
    if (input.agentId !== this.spec.agentId) return fail("CONTEXT_NOT_FOUND", `No such agent ${input.agentId}`);
    const p = await this.phone(); if (!p.ok) return p;
    const opened = await this.tool("phone.open_app", { serial: p.value.serial, app: this.spec.appId }); if (!opened.ok) return opened;
    await this.o.sleep(this.o.settleMs);
    const s = await this.screen(p.value.serial); if (!s.ok) return s;
    const g = this.gate(s.value); if (!g.ok) return g;
    // The message field must be on screen: if the app's layout changed, stop now rather than mid-send.
    if (!findByLabel(g.value, new RegExp(`^${escapeRe(this.spec.composer)}$`))) return this.drift("composer");
    if (this.spec.newChat && findByLabel(g.value, this.spec.newChat)) {
      const label = findByLabel(g.value, this.spec.newChat)!;
      await this.tool("phone.find_and_tap", { serial: p.value.serial, element_id: label.label ?? label.id });
      await this.o.sleep(this.o.settleMs);
    }
    for (const c of this.ctxs.values()) c.closed = true; // one conversation on screen at a time
    const id = `ph-ctx-${++this.n}-${Math.random().toString(36).slice(2, 8)}`;
    const c: Ctx = { id, threadId: input.threadId ?? id, closed: false, serial: p.value.serial, events: [], seq: 0, msgN: 0, done: new Map(), lock: Promise.resolve() };
    this.ctxs.set(id, c);
    this.push(c, "agent.context.opened", { title: input.title ?? null, route: this.spec.route }, id);
    return ok({ contextId: id, isolation: this.spec.capabilities.context.isolation, guarantee: this.spec.capabilities.guarantee, resumed: false });
  }

  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    return c && !c.closed ? ok(c) : fail("CONTEXT_NOT_FOUND", `No such open ${this.spec.appName} conversation.`);
  }

  async contextMessage(input: MessageInput): Promise<AaiResult<MessageResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const prior = c.done.get(input.correlationId);
    if (prior) return prior;
    const run = c.lock.then(() => this.sendOne(c, input));
    c.lock = run.catch(() => undefined);
    c.done.set(input.correlationId, run);
    return run;
  }

  /**
   * Static text on screen that isn't app chrome: iOS StaticText, Android TextView. The message
   * field is excluded even though iOS calls it a TextView (it carries the composer label).
   */
  private contentTexts(nodes: UiNode[]): string[] {
    return nodes
      .filter((n) => /^(StaticText|TextView)$/.test(n.cls) && !n.flags.includes("editable") && n.label !== this.spec.composer)
      .map(nodeText)
      .filter((t): t is string => !!t && t.trim().length > 0 && !this.spec.chrome.test(t));
  }

  private async sendOne(c: Ctx, input: MessageInput): Promise<AaiResult<MessageResult>> {
    const before = await this.screen(c.serial); if (!before.ok) return before;
    const g = this.gate(before.value); if (!g.ok) return g;
    const seen = new Set(this.contentTexts(g.value));
    const tap = await this.tool("phone.find_and_tap", { serial: c.serial, element_id: this.spec.composer }); if (!tap.ok) return this.drift("composer");
    const typed = await this.tool("phone.type", { serial: c.serial, text: input.text }); if (!typed.ok) return typed;
    await this.o.sleep(300);
    const afterType = await this.screen(c.serial); if (!afterType.ok) return afterType;
    const send = findByLabel(afterType.value, this.spec.send);
    if (!send) return this.drift("send_button");
    const sent = await this.tool("phone.find_and_tap", { serial: c.serial, element_id: send.label ?? send.id }); if (!sent.ok) return sent;
    c.msgN += 1;
    this.push(c, "agent.activity.started", { state: "sent" }, input.correlationId);
    seen.add(input.text);
    const messageId = `ph-msg-${c.id}-${c.msgN}`;
    const deadline = this.o.now() + this.o.replyTimeoutMs;
    let last: string | undefined;
    let stableReads = 0;
    let sawStreaming = false;
    let reply: string | undefined;
    for (;;) {
      await this.o.sleep(this.o.pollMs);
      const s = await this.screen(c.serial); if (!s.ok) return s;
      const gg = this.gate(s.value); if (!gg.ok) return gg;
      const streaming = !!findByLabel(gg.value, this.spec.stop);
      if (streaming) sawStreaming = true;
      const fresh = this.contentTexts(gg.value).filter((t) => !seen.has(t) && t !== input.text);
      const text = fresh.join("\n").trim() || undefined;
      if (text && text !== last) {
        last = text;
        stableReads = 0;
        this.push(c, "agent.message.delta", { text }, input.correlationId);
      } else if (text) {
        stableReads += 1;
      }
      const backToIdle = !streaming && (!!findByLabel(gg.value, this.spec.send) || !!findByLabel(gg.value, new RegExp(`^${escapeRe(this.spec.composer)}$`)));
      if (text && backToIdle && (sawStreaming || stableReads >= 1)) { reply = text; break; }
      if (this.o.now() >= deadline) break;
    }
    if (reply) this.push(c, "agent.message.completed", { text: reply, messageId }, input.correlationId);
    return ok({ messageId, correlationId: input.correlationId, reply, guarantee: this.spec.capabilities.guarantee });
  }

  private drift(missing: string): AaiResult<never> {
    this.halted = { code: "ADAPTER_DRIFT", retryable: false, humanMessage: `${this.spec.appName}'s ${missing.replace("_", " ")} on your phone wasn't found. Allternit has stopped driving it until the adapter is updated.`, details: { missing: [missing] } };
    return { ok: false, error: this.halted };
  }

  async contextCancel(input: { contextId: string }): Promise<AaiResult<CancelResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const s = await this.screen(l.value.serial); if (!s.ok) return s;
    const stop = findByLabel(s.value, this.spec.stop);
    if (stop) await this.tool("phone.find_and_tap", { serial: l.value.serial, element_id: stop.label ?? stop.id });
    await this.o.sleep(this.o.settleMs);
    const after = await this.screen(l.value.serial); if (!after.ok) return after;
    return ok({ confirmed: !findByLabel(after.value, this.spec.stop) });
  }

  async contextClose(input: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const c = this.ctxs.get(input.contextId);
    if (!c) return fail("CONTEXT_NOT_FOUND", `No such ${this.spec.appName} conversation.`);
    c.closed = true; // the chat stays in the app; closing never deletes it
    return ok({ closed: true });
  }

  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const from = Number(input.cursor ?? 0) || 0;
    const evs = l.value.events.filter((e) => Number(e.cursor) > from).slice(0, input.limit ?? 500);
    return ok({ events: evs, nextCursor: evs.length ? evs[evs.length - 1].cursor : String(from) });
  }

  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const p = await this.phone();
    if (!p.ok) return ok({ status: "down", lane: "ui_bridge", detail: p.error.humanMessage });
    return ok({ status: p.value.health?.state === "DEVICE_LOCKED" ? "degraded" : "healthy", lane: "ui_bridge", detail: p.value.health?.state === "DEVICE_LOCKED" ? "phone locked" : undefined });
  }
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
