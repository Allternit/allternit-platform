// Route `desktop-app-ax`: one AAI provider for any signed-in desktop chat app read and operated through the macOS
// Accessibility tree. A vendor adapter (chatgpt-desktop, kimi-desktop) supplies only data: bundle ids, a selector pack,
// a manifest. The provider reads the AX tree directly (no DOM re-rendering) and covers the spec §3 contract items it can:
//   start/continue a chat, pick the model, read the reply while it streams and know it finished (the app's own Stop
//   control leaves the tree — never "the screen stopped changing"), stop mid-stream, text from the tree, list + reopen
//   past threads, typed usage-limit / sign-out / verification-wall errors.
// It never sends attachments and never fetches generated files: those answer UNSUPPORTED with the reason (see
// AX_CHAT_CONTRACT), so the route router marks the route ineligible instead of it quietly doing less.
// Events are best_effort / inferred, never exact (lane ui_bridge).
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type AgentDetail, type AgentIdentity, type AgentSummary, type AAIError,
  type CancelResult, type CursoredEvent, type EventsInput, type EventsResult, type GatewayEvent, type HealthResult,
  type MessageInput, type MessageResult, type OpenContextInput, type OpenContextResult,
} from "@allternit/agent-gateway";
import type { AxDriver } from "./bridge.js";
import { buttonLabel, writeComposer } from "./page.js";
import { assertPackCompatible, missingCritical, resolveKey, resolveKeys, type AxSelectorPack } from "./selector.js";
import { AX_NOT_TRUSTED_MESSAGE, AxError, type AxNode } from "./types.js";

export const DESKTOP_APP_AX_ROUTE = "desktop-app-ax" as const;
export const DESKTOP_APP_CDP_ROUTE = "desktop-app-cdp" as const;
export type DesktopRouteId = typeof DESKTOP_APP_AX_ROUTE | typeof DESKTOP_APP_CDP_ROUTE;

// ---------------------------------------------------------------- contract (spec §3)
export type ContractItem =
  | "chat.start_continue" | "reply.stream_read" | "reply.stop_midstream" | "attachments.send"
  | "outputs.exact_and_files" | "threads.list_reopen" | "errors.typed_states";
export interface ContractEntry { item: ContractItem; status: "implemented" | "partial" | "blocked"; how: string }
/** What every AxChatAppProvider does for each §3 item. Adapters copy this into their README table. */
export const AX_CHAT_CONTRACT: ContractEntry[] = [
  { item: "chat.start_continue", status: "implemented", how: "contextOpen presses New chat (and picks a model when `model` is given); contextMessage writes the composer, reads it back, presses Send. models()/selectModel() drive the app's model picker." },
  { item: "reply.stream_read", status: "implemented", how: "agent.message.delta while the app shows its Stop control; agent.message.completed when that control leaves the tree and the reply has text (a real app signal)." },
  { item: "reply.stop_midstream", status: "implemented", how: "contextCancel presses Stop; confirmed=true only when the next read shows streaming ended." },
  { item: "attachments.send", status: "blocked", how: "Attaching opens the macOS open panel (a separate process) or needs pasteboard injection; neither is inside the ax-bridge contract, so sendAttachment() answers UNSUPPORTED." },
  { item: "outputs.exact_and_files", status: "partial", how: "Text: read verbatim from the AX static-text values (no OCR), paragraph breaks preserved. Generated images/files: blocked — fetching them means driving a vendor save dialog; outputFiles() answers UNSUPPORTED." },
  { item: "threads.list_reopen", status: "implemented", how: "threads() lists the sidebar rows; contextOpen({ adoptContextId: \"thread:<id>\" }) presses that row and continues it." },
  { item: "errors.typed_states", status: "implemented", how: "usage limit -> RATE_LIMITED (+retryAfterMs from the banner); signed out -> AUTH_REQUIRED/AUTH_REVOKED; verification wall -> LANE_BLOCKED (latched); missing helper permission -> AUTH_REQUIRED; selectors gone -> ADAPTER_DRIFT (latched)." },
];
export const AX_ROUTE_ELIGIBLE = AX_CHAT_CONTRACT.every((c) => c.status === "implemented");

// ---------------------------------------------------------------- spec
/**
 * Selector pack keys an app pack provides (critical marks drift evidence):
 *  composer* · send · stop · newChat · userTurn · assistantTurn · alert · signIn · modelPicker · modelOption · threadRow · attach
 */
export interface AxChatAppSpec {
  adapterId: string;
  agentId: string;
  appName: string;
  vendor: string;
  lookPack: string;
  /** Tried in order: the first one running is driven. */
  bundleIds: string[];
  pack: AxSelectorPack;
  capabilities: AgentCapabilityManifest;
  pacing: { min_action_gap_ms: [number, number]; min_task_gap_s: number; max_tasks_per_hour: number; max_tasks_per_day: number };
  patterns?: Partial<typeof DEFAULT_PATTERNS>;
}
export const DEFAULT_PATTERNS = {
  rateLimited: /usage limit|limit reached|reached (your|the) .*limit|too many (requests|messages)|out of (free )?messages|rate limit|try again later/i,
  blocked: /unusual activity|suspicious activity|verify (that )?(you are|you're) (a )?human|captcha|security check|account (has been )?(suspended|disabled)/i,
};

// ---------------------------------------------------------------- reading the tree
export interface AxChatTurn { role: "user" | "assistant"; text: string }
export type AxChatState =
  | { kind: "ok" | "rate_limited"; turns: AxChatTurn[]; streaming: boolean; composer: string; banner?: string; model?: string; retryAfterMs?: number }
  | { kind: "logged_out" | "blocked"; banner?: string }
  | { kind: "drift"; missing: string[]; reason: string };

/** Verbatim text under a node: static-text values joined as-is inside one parent, "\n" between parents (paragraphs). */
export function exactText(n: AxNode): string {
  const out: string[] = []; let lastParent = "";
  const visit = (node: AxNode, parent: string) => {
    if (node.role === "AXStaticText" || node.role === "AXHeading") {
      const t = node.value ?? node.title ?? node.description ?? "";
      if (t) { if (out.length && parent !== lastParent) out.push("\n"); out.push(t); lastParent = parent; }
      return;
    }
    for (const c of node.children ?? []) visit(c, node.path.join("."));
  };
  visit(n, "");
  return out.join("").replace(/\n{3,}/g, "\n\n").trim();
}

/** "resets in 2 hours" / "try again in 15 minutes" / "resets at 14:00" -> ms (bounded 1 min..24 h). */
export function retryAfterFromBanner(text: string, now: number): number | undefined {
  const a = /\bin\s+(\d+)\s*(second|sec|minute|min|hour|hr|day)s?\b/i.exec(text);
  if (a) {
    const unit = a[2].toLowerCase();
    const mult = unit.startsWith("s") ? 1000 : unit.startsWith("m") ? 60_000 : unit.startsWith("h") ? 3_600_000 : 86_400_000;
    return Math.min(86_400_000, Math.max(60_000, Number(a[1]) * mult));
  }
  const b = /\bat\s+(\d{1,2}):(\d{2})\s*(am|pm)?/i.exec(text);
  if (b) {
    let h = Number(b[1]); const ap = b[3]?.toLowerCase();
    if (ap === "pm" && h < 12) h += 12; if (ap === "am" && h === 12) h = 0;
    const d = new Date(now); d.setHours(h, Number(b[2]), 0, 0);
    let ms = d.getTime() - now; if (ms <= 0) ms += 86_400_000;
    return Math.min(86_400_000, Math.max(60_000, ms));
  }
  return undefined;
}

const labelText = (n: AxNode) => (buttonLabel(n) || exactText(n)).trim();

export function readChatState(root: AxNode, pack: AxSelectorPack, now: number, patterns = DEFAULT_PATTERNS): AxChatState {
  try { assertPackCompatible(pack); } catch (e) { return { kind: "drift", missing: [], reason: (e as Error).message }; }
  const banner = resolveKey(root, pack, "alert").map(exactText).filter(Boolean).join(" | ") || undefined;
  if (banner && patterns.blocked.test(banner)) return { kind: "blocked", banner };
  const composer = resolveKey(root, pack, "composer")[0];
  if (!composer) {
    if (pack.keys.signIn && resolveKey(root, pack, "signIn").length) return { kind: "logged_out", banner };
    return { kind: "drift", missing: missingCritical(root, pack), reason: `composer not found by any AX selector (${pack.packVersion})` };
  }
  const turns = resolveKeys(root, pack, ["userTurn", "assistantTurn"]).map(({ key, node }) => ({ role: key === "userTurn" ? "user" as const : "assistant" as const, text: exactText(node) }));
  const streaming = !!pack.keys.stop && resolveKey(root, pack, "stop").length > 0;
  const picker = pack.keys.modelPicker ? resolveKey(root, pack, "modelPicker")[0] : undefined;
  const model = picker ? (picker.value && !/^\d+$/.test(picker.value) ? picker.value : labelText(picker)) || undefined : undefined;
  const base = { turns, streaming, composer: composer.value ?? "", banner, model };
  if (banner && patterns.rateLimited.test(banner)) return { kind: "rate_limited", ...base, retryAfterMs: retryAfterFromBanner(banner, now) ?? (/usage limit|out of/i.test(banner) ? 3_600_000 : 60_000) };
  return { kind: "ok", ...base };
}

// ---------------------------------------------------------------- provider
export interface AxThread { id: string; title: string }
export interface AxChatAppProviderOptions {
  ax: AxDriver;
  /** Explicit per-app OK from the user to drive this app (default false: every call answers LANE_BLOCKED, the bridge is untouched). */
  consented?: boolean;
  pacing?: boolean;
  pollMs?: number;
  replyTimeoutMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
  random?: () => number;
}
interface Ctx {
  id: string; threadId: string; closed: boolean; events: CursoredEvent[]; seq: number; baseline: number;
  seen: Map<number, { text: string; completed: boolean }>; currentCorr?: string; msgN: number; activity: boolean;
  done: Map<string, Promise<AaiResult<MessageResult>>>; lock: Promise<unknown>; sending: number;
}
type Ok = Extract<AxChatState, { kind: "ok" | "rate_limited" }>;

const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 80);

export class AxChatAppProvider extends BaseAaiProvider {
  readonly adapterId: string;
  private ctxs = new Map<string, Ctx>();
  private n = 0;
  private halted: AAIError | undefined;
  private cooldownUntil = 0;
  private everLoggedIn = false;
  private sendTimes: number[] = [];
  private attached: string | undefined;
  /** Why the last read was drift (selector evidence for humans/logs). */
  lastDrift: { missing: string[]; packVersion: string; reason: string } | undefined;
  private o: Required<Omit<AxChatAppProviderOptions, "ax">> & { ax: AxDriver };
  private patterns: typeof DEFAULT_PATTERNS;

  constructor(readonly spec: AxChatAppSpec, opts: AxChatAppProviderOptions) {
    super();
    this.adapterId = spec.adapterId;
    this.patterns = { ...DEFAULT_PATTERNS, ...spec.patterns };
    this.o = { consented: false, pacing: true, pollMs: 300, replyTimeoutMs: 120_000, now: Date.now, sleep: (ms) => new Promise((r) => setTimeout(r, ms)), random: Math.random, ...opts };
  }
  clearHalt() { this.halted = undefined; }
  /** The bundle id actually attached (one of spec.bundleIds), once attached. */
  get attachedBundleId() { return this.attached; }

  // ---------- identity ----------
  private summary(): AgentSummary { return { agentId: this.spec.agentId, displayName: this.spec.appName, vendor: this.spec.vendor, state: this.halted ? "blocked" : "linked" }; }
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
    return ok({ agentId: this.spec.agentId, displayName: this.spec.appName, vendor: this.spec.vendor, lookPack: this.spec.lookPack });
  }

  // ---------- AX access ----------
  private async attach(): Promise<void> {
    if (this.attached) return;
    if (!(await this.o.ax.trust())) throw new AxError("not_trusted", "Accessibility permission not granted");
    let last: unknown;
    for (const b of this.spec.bundleIds) {
      try { await this.o.ax.attach(b); this.attached = b; return; }
      catch (e) { last = e; if (!(e instanceof AxError && e.fault === "not_running")) throw e; }
    }
    throw last ?? new AxError("not_running", `${this.spec.appName} is not running`);
  }
  private async root(): Promise<AxNode> { await this.attach(); return (await this.o.ax.snapshot()).root; }
  private axFail(e: unknown): AaiResult<never> {
    if (e instanceof AxError) {
      if (e.fault === "not_trusted") return fail("AUTH_REQUIRED", AX_NOT_TRUSTED_MESSAGE);
      if (e.fault === "not_running") return fail("VENDOR_UNAVAILABLE", `${this.spec.appName} is not running. Open it and sign in, then try again.`);
      if (e.fault === "selector_version") return this.drift([], e.message);
    }
    return fail("VENDOR_UNAVAILABLE", `Could not read ${this.spec.appName}: ${(e as Error).message}`);
  }
  private async read(allowRateLimited = false): Promise<AaiResult<Ok & { root: AxNode }>> {
    if (this.halted) return { ok: false, error: this.halted };
    if (!this.o.consented) return fail("LANE_BLOCKED", `Driving ${this.spec.appName} through Accessibility needs your explicit OK first. Enable it for this account, then try again.`, { retryable: false });
    let root: AxNode;
    try { root = await this.root(); } catch (e) { return this.axFail(e); }
    const st = readChatState(root, this.spec.pack, this.o.now(), this.patterns);
    switch (st.kind) {
      case "ok": this.everLoggedIn = true; this.lastDrift = undefined; return ok({ ...st, root });
      case "rate_limited": {
        this.everLoggedIn = true;
        this.cooldownUntil = this.o.now() + (st.retryAfterMs ?? 60_000);
        if (allowRateLimited) return ok({ ...st, root });
        return fail("RATE_LIMITED", `${this.spec.appName} reports a usage or rate limit. Allternit will wait before sending again.`, { retryAfterMs: st.retryAfterMs, details: { banner: st.banner } });
      }
      case "logged_out":
        return fail(this.everLoggedIn ? "AUTH_REVOKED" : "AUTH_REQUIRED", `${this.spec.appName} is signed out. Sign in inside the app, then reconnect.`, { retryable: false });
      case "blocked":
        this.halted = { code: "LANE_BLOCKED", retryable: false, humanMessage: `${this.spec.appName} is asking for verification or reports unusual activity. Allternit has stopped; please resolve it in the app yourself.`, details: { banner: st.banner } };
        return { ok: false, error: this.halted };
      case "drift":
        return this.drift(st.missing, st.reason);
    }
  }
  private drift(missing: string[], reason: string): AaiResult<never> {
    this.lastDrift = { missing, packVersion: this.spec.pack.packVersion, reason };
    this.halted = { code: "ADAPTER_DRIFT", retryable: false, humanMessage: `${this.spec.appName}'s screen no longer matches what this adapter expects (selectors ${this.spec.pack.packVersion}). Allternit has stopped driving it until the adapter is updated.`, details: { missing, selectorsVersion: this.spec.pack.packVersion, detail: reason } };
    return { ok: false, error: this.halted };
  }
  private async press(key: string, root?: AxNode, match?: (n: AxNode) => boolean): Promise<boolean> {
    if (!this.spec.pack.keys[key]) return false;
    const nodes = resolveKey(root ?? (await this.root()), this.spec.pack, key).filter((n) => !match || match(n));
    return nodes[0] ? this.o.ax.press(nodes[0].path) : false;
  }
  private cooldown(): AaiResult<never> | undefined {
    const left = this.cooldownUntil - this.o.now();
    return left > 0 ? fail("RATE_LIMITED", `${this.spec.appName} usage-limit cooldown in effect.`, { retryAfterMs: left }) : undefined;
  }
  private async pace() {
    if (!this.o.pacing) return;
    const [lo, hi] = this.spec.pacing.min_action_gap_ms;
    await this.o.sleep(lo + this.o.random() * (hi - lo));
  }

  // ---------- models (contract: pick the model/mode the app offers) ----------
  /** Current model + the options the app's picker lists (opens the picker, reads, closes it with a second press). */
  async models(): Promise<AaiResult<{ current?: string; options: string[] }>> {
    const g = await this.read(true); if (!g.ok) return g;
    if (!this.spec.pack.keys.modelPicker || !resolveKey(g.value.root, this.spec.pack, "modelPicker").length) return fail("UNSUPPORTED", `${this.spec.appName} shows no model picker in this window.`, { retryable: false });
    try {
      await this.press("modelPicker", g.value.root);
      const opts = resolveKey(await this.root(), this.spec.pack, "modelOption").map(labelText).filter(Boolean);
      await this.press("modelPicker");
      return ok({ current: g.value.model, options: [...new Set(opts)] });
    } catch (e) { return this.axFail(e); }
  }
  /** Pick a model by its visible name (case-insensitive exact, else unique prefix). */
  async selectModel(name: string): Promise<AaiResult<{ model: string }>> {
    const g = await this.read(true); if (!g.ok) return g;
    if (g.value.streaming) return this.busy();
    if (g.value.model && g.value.model.toLowerCase() === name.toLowerCase()) return ok({ model: g.value.model });
    try {
      if (!(await this.press("modelPicker", g.value.root))) return fail("UNSUPPORTED", `${this.spec.appName} shows no model picker in this window.`, { retryable: false });
      await this.pace();
      const options = resolveKey(await this.root(), this.spec.pack, "modelOption");
      const lc = name.toLowerCase();
      const pick = options.find((o) => labelText(o).toLowerCase() === lc) ?? (() => { const p = options.filter((o) => labelText(o).toLowerCase().startsWith(lc)); return p.length === 1 ? p[0] : undefined; })();
      if (!pick) { await this.press("modelPicker"); return fail("UNKNOWN", `${this.spec.appName} does not offer a model called "${name}". Options: ${options.map(labelText).join(", ") || "none"}.`, { retryable: false }); }
      await this.o.ax.press(pick.path);
      const after = await this.read(true); if (!after.ok) return after;
      return ok({ model: after.value.model ?? labelText(pick) });
    } catch (e) { return this.axFail(e); }
  }

  // ---------- threads (contract: list and reopen past threads) ----------
  private threadRows(root: AxNode): Array<{ node: AxNode; thread: AxThread }> {
    if (!this.spec.pack.keys.threadRow) return [];
    const seen = new Map<string, number>();
    return resolveKey(root, this.spec.pack, "threadRow").map((node) => {
      const title = labelText(node);
      const base = slug(title) || "untitled"; const k = (seen.get(base) ?? 0) + 1; seen.set(base, k);
      return { node, thread: { id: k > 1 ? `${base}~${k}` : base, title } };
    }).filter((r) => r.thread.title);
  }
  async threads(): Promise<AaiResult<AxThread[]>> {
    const g = await this.read(true); if (!g.ok) return g;
    return ok(this.threadRows(g.value.root).map((r) => r.thread));
  }

  // ---------- outputs (contract: blocked items answer, never silently do less) ----------
  async sendAttachment(_i: { contextId: string; path: string }): Promise<AaiResult<never>> {
    return fail("UNSUPPORTED", AX_CHAT_CONTRACT.find((c) => c.item === "attachments.send")!.how, { retryable: false, details: { contractItem: "attachments.send", route: DESKTOP_APP_AX_ROUTE } });
  }
  async outputFiles(_i: { contextId: string }): Promise<AaiResult<never>> {
    return fail("UNSUPPORTED", "Generated images/files are not fetched on the accessibility route: that needs a vendor save dialog. Text replies are returned.", { retryable: false, details: { contractItem: "outputs.exact_and_files", route: DESKTOP_APP_AX_ROUTE } });
  }
  /** Full visible transcript of an open context's window, verbatim from the tree. */
  async transcript(contextId: string): Promise<AaiResult<AxChatTurn[]>> {
    const l = this.live(contextId); if (!l.ok) return l;
    const g = await this.read(true); if (!g.ok) return g;
    return ok(g.value.turns);
  }

  // ---------- events ----------
  private push(c: Ctx, type: GatewayEvent["type"], guarantee: GatewayEvent["guarantee"], payload: Record<string, unknown>, source: GatewayEvent["source"] = "vendor", corr?: string) {
    c.seq += 1;
    const correlationId = corr ?? c.currentCorr ?? `obs-${c.id}`;
    c.events.push({ cursor: String(c.seq), event: {
      type, botId: this.spec.agentId, threadId: c.threadId, generationId: String(c.msgN), source, vendor: this.spec.vendor, adapter: this.spec.adapterId, lane: "ui_bridge",
      remoteEventId: `${c.id}:${c.seq}`, remoteContextId: c.id, causationId: correlationId, correlationId, guarantee, at: new Date(this.o.now()).toISOString(), payload,
    } });
  }
  private observe(c: Ctx, st: Ok) {
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
        this.push(c, "agent.message.completed", "inferred", { text: t.text, turnIndex: i, finishedBy: "stop_control_gone" });
        c.activity = false;
      }
      c.seen.set(i, prev);
    });
  }

  // ---------- context ----------
  private busy(): AaiResult<never> {
    return fail("CONTEXT_BUSY", `${this.spec.appName} is still answering. Try again when that reply finishes.`, { retryAfterMs: 30_000 });
  }
  private live(id: string): AaiResult<Ctx> {
    const c = this.ctxs.get(id);
    return c && !c.closed ? ok(c) : fail("CONTEXT_NOT_FOUND", `No such open ${this.spec.appName} conversation.`);
  }
  async contextOpen(input: OpenContextInput & { model?: string }): Promise<AaiResult<OpenContextResult>> {
    if (input.agentId !== this.spec.agentId) return fail("CONTEXT_NOT_FOUND", `No such agent ${input.agentId}`);
    const caps = this.spec.capabilities;
    const adopt = input.adoptContextId;
    if (adopt && !adopt.startsWith("thread:")) {
      const c = this.ctxs.get(adopt);
      return c && !c.closed ? ok({ contextId: c.id, isolation: caps.context.isolation, guarantee: caps.guarantee, resumed: true }) : fail("CONTEXT_NOT_FOUND", `No such ${this.spec.appName} conversation to resume.`);
    }
    const open = [...this.ctxs.values()].filter((c) => !c.closed);
    if (open.some((c) => c.sending > 0)) return this.busy();
    const cd = this.cooldown(); if (cd) return cd;
    const g = await this.read(); if (!g.ok) return g;
    if (g.value.streaming) return this.busy();
    // The app shows one conversation at a time: a new one replaces the open one (which stays in the app, undriven).
    for (const o of open) o.closed = true;
    await this.pace();
    let reopened: AxThread | undefined;
    try {
      if (adopt) {
        const want = adopt.slice("thread:".length);
        const row = this.threadRows(g.value.root).find((r) => r.thread.id === want || r.thread.title === want);
        if (!row) return fail("CONTEXT_NOT_FOUND", `${this.spec.appName} has no past thread "${want}" in its sidebar.`);
        if (!(await this.o.ax.press(row.node.path))) return this.drift(["threadRow"], "thread row did not accept a press");
        reopened = row.thread;
      } else if (!(await this.press("newChat", g.value.root))) {
        return this.drift(["newChat"], "New chat control not found");
      }
    } catch (e) { return this.axFail(e); }
    if (input.model) { await this.pace(); const m = await this.selectModel(input.model); if (!m.ok) return m; }
    const after = await this.read(); if (!after.ok) return after;
    const id = `${this.spec.adapterId}-ctx-${++this.n}-${Math.random().toString(36).slice(2, 8)}`;
    const c: Ctx = { id, threadId: input.threadId ?? id, closed: false, events: [], seq: 0, baseline: after.value.turns.length, seen: new Map(), msgN: 0, activity: false, done: new Map(), lock: Promise.resolve(), sending: 0 };
    this.ctxs.set(id, c);
    this.push(c, "agent.context.opened", "best_effort", { title: input.title ?? null, model: after.value.model ?? null, ...(reopened ? { reopened: reopened.id, turns: after.value.turns.length } : {}) }, "allternit", id);
    return ok({ contextId: id, isolation: caps.context.isolation, guarantee: caps.guarantee, resumed: !!reopened });
  }

  async contextMessage(input: MessageInput): Promise<AaiResult<MessageResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const prior = c.done.get(input.correlationId);
    if (prior) return prior; // idempotent replays return the first result
    c.sending += 1;
    const run = c.lock.then(() => this.sendOne(c, input)).finally(() => { c.sending -= 1; });
    c.lock = run.catch(() => undefined);
    c.done.set(input.correlationId, run);
    return run;
  }
  private async sendOne(c: Ctx, input: MessageInput): Promise<AaiResult<MessageResult>> {
    const cd = this.cooldown(); if (cd) return cd;
    const now = this.o.now(); const P = this.spec.pacing;
    this.sendTimes = this.sendTimes.filter((t) => now - t < 86_400_000);
    if (this.sendTimes.filter((t) => now - t < 3_600_000).length >= P.max_tasks_per_hour) return fail("RATE_LIMITED", `Allternit's own hourly pacing limit for ${this.spec.appName} was reached.`, { retryAfterMs: 600_000 });
    if (this.sendTimes.length >= P.max_tasks_per_day) return fail("RATE_LIMITED", `Allternit's own daily pacing limit for ${this.spec.appName} was reached.`, { retryAfterMs: 3_600_000 });
    const g = await this.read(); if (!g.ok) return g;
    if (g.value.streaming) return this.busy();
    const sendIndex = g.value.turns.length;
    if (this.o.pacing && this.sendTimes.length) await this.o.sleep(Math.max(0, P.min_task_gap_s * 1000 - (now - this.sendTimes[this.sendTimes.length - 1])));
    await this.pace();
    try {
      const find = async () => resolveKey(await this.root(), this.spec.pack, "composer")[0];
      if (!(await writeComposer(this.o.ax, await find(), input.text, find))) return this.drift(["composer"], "composer did not accept or read back the text");
      await this.pace();
      if (!(await this.press("send"))) return this.drift(["send"], "Send control not found");
    } catch (e) { return this.axFail(e); }
    this.sendTimes.push(this.o.now());
    c.msgN += 1; c.currentCorr = input.correlationId;
    this.push(c, "agent.activity.started", "best_effort", { state: "sent" }, "allternit", input.correlationId);
    const messageId = `${this.spec.adapterId}-msg-${c.id}-${c.msgN}`;
    const deadline = this.o.now() + this.o.replyTimeoutMs;
    let reply: string | undefined;
    for (;;) {
      const st = await this.read(true); if (!st.ok) return st;
      this.observe(c, st.value);
      const a = st.value.turns.slice(sendIndex + 1).find((t) => t.role === "assistant");
      if (a && !st.value.streaming && a.text) { reply = a.text; break; }
      if (this.o.now() >= deadline) break; // still streaming: the caller keeps reading events()
      await this.o.sleep(this.o.pollMs);
    }
    return ok({ messageId, correlationId: input.correlationId, reply, guarantee: this.spec.capabilities.guarantee });
  }

  async contextCancel(input: { contextId: string }): Promise<AaiResult<CancelResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const g = await this.read(true); if (!g.ok) return g;
    try { if (g.value.streaming) await this.press("stop", g.value.root); } catch (e) { return this.axFail(e); }
    const after = await this.read(true); if (!after.ok) return after;
    this.observe(l.value, after.value);
    return ok({ confirmed: !after.value.streaming });
  }
  async contextClose(input: { contextId: string }): Promise<AaiResult<{ closed: boolean }>> {
    const c = this.ctxs.get(input.contextId);
    if (!c) return fail("CONTEXT_NOT_FOUND", `No such ${this.spec.appName} conversation.`);
    c.closed = true; // the chat stays in the app: closing never deletes the user's thread
    return ok({ closed: true });
  }
  async events(input: EventsInput): Promise<AaiResult<EventsResult>> {
    const l = this.live(input.contextId); if (!l.ok) return l;
    const c = l.value;
    const g = await this.read(true); if (!g.ok) return g;
    this.observe(c, g.value);
    const from = Number(input.cursor ?? 0) || 0;
    const evs = c.events.filter((e) => Number(e.cursor) > from).slice(0, input.limit ?? 500);
    return ok({ events: evs, nextCursor: evs.length ? evs[evs.length - 1].cursor : String(from) });
  }
  async health(_i: { agentId?: string }): Promise<AaiResult<HealthResult>> {
    const g = await this.read(true);
    if (!g.ok) return ok({ status: "down", lane: "ui_bridge", detail: `${g.error.code}: ${g.error.humanMessage}` });
    const cool = this.cooldownUntil > this.o.now();
    return ok({ status: cool ? "degraded" : "healthy", lane: "ui_bridge", detail: cool ? "usage-limit cooldown" : undefined });
  }
}


/** An AxDriver for when no ax-bridge binary is configured: every call fails as bridge_unavailable (-> VENDOR_UNAVAILABLE). */
export function unavailableAxDriver(reason: string): AxDriver {
  const no = async (): Promise<never> => { throw new AxError("bridge_unavailable", reason); };
  return { trust: no, attach: no, snapshot: no, find: no, press: no, setValue: no, focus: no, observe: no, dispose: async () => {} };
}
