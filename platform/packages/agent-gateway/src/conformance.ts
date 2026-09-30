// Adapter conformance harness (spec: "Conformance suite"). Checks respect the provider's own declared
// manifest: `exact` providers get strict ordering/dedup/replay checks, `best_effort` get them relaxed,
// and anything the manifest does not declare must answer UNSUPPORTED (never ok, never a raw throw).
import { agentCapabilityManifestSchema, gatewayEventSchema, mirrorFieldStateSchema } from "@allternit/subscription-fabric-contracts";
import type { AaiProvider, AaiResult, AAIError, AAIErrorCode, AgentCapabilityManifest, CursoredEvent } from "./types.js";

export type FaultKind = "vendor_down" | "rate_limited" | "auth_revoked" | "account_banned" | "ui_changed";

export interface ConformanceFixtures {
  agentId: string;
  /** Distinct strings used to detect cross-context contamination. */
  tokens?: { a: string; b: string };
  /** A pending approval id known to the recorded session (for approvals respond tests). */
  approvalId?: string;
  /** Builds a provider whose vendor is failing in the given way (transport-level fault injection). */
  faulty?: Partial<Record<FaultKind, () => AaiProvider>>;
  /** Mutates the remote so agent.sync should report drift. */
  remoteMutation?: () => Promise<void>;
  /** ms to wait when checking that cancelled work really stopped. Default 40. */
  settleMs?: number;
}

export type CheckStatus = "pass" | "fail" | "unsupported-ok" | "skipped";
export interface CheckResult { name: string; status: CheckStatus; reason?: string }
export type AreaStatus = "pass" | "fail" | "skipped-unsupported";
export interface AreaResult { area: string; status: AreaStatus; reasons: string[]; checks: CheckResult[] }
export interface ConformanceReport {
  adapterId: string; guarantee: string; lane: string; ok: boolean;
  areas: AreaResult[]; summary: { pass: number; fail: number; skippedUnsupported: number };
}

class Area {
  checks: CheckResult[] = [];
  constructor(readonly name: string) {}
  pass(name: string) { this.checks.push({ name, status: "pass" }); }
  fail(name: string, reason: string) { this.checks.push({ name, status: "fail", reason }); }
  unsup(name: string) { this.checks.push({ name, status: "unsupported-ok" }); }
  skip(name: string, reason: string) { this.checks.push({ name, status: "skipped", reason }); }
  expect(name: string, cond: boolean, reason: string) { cond ? this.pass(name) : this.fail(name, reason); }
  result(): AreaResult {
    const fails = this.checks.filter((c) => c.status === "fail");
    const anyPass = this.checks.some((c) => c.status === "pass");
    const status: AreaStatus = fails.length ? "fail" : anyPass ? "pass" : "skipped-unsupported";
    const reasons = this.checks.filter((c) => c.reason).map((c) => `${c.name}: ${c.reason}`);
    return { area: this.name, status, reasons, checks: this.checks };
  }
}

const errOf = <T>(r: AaiResult<T>): AAIError | undefined => (r.ok ? undefined : r.error);
const desc = <T>(r: AaiResult<T>) => (r.ok ? "ok" : `${r.error.code} (${r.error.humanMessage})`);

async function safe<T>(a: Area, label: string, fn: () => Promise<AaiResult<T>>): Promise<AaiResult<T>> {
  try { return await fn(); } catch (e) {
    a.fail(`${label} never throws`, `threw raw: ${(e as Error)?.message ?? String(e)}`);
    return { ok: false, error: { code: "UNKNOWN", retryable: false, humanMessage: "threw", details: { threw: true } } };
  }
}

/** Declared-unsupported (or undeclared) ops must return UNSUPPORTED. */
async function expectUnsupported<T>(a: Area, name: string, fn: () => Promise<AaiResult<T>>) {
  const r = await safe(a, name, fn);
  if (!r.ok && r.error.code === "UNSUPPORTED") a.unsup(name);
  else a.fail(name, `not declared, expected UNSUPPORTED but got ${desc(r)}`);
}

async function expectOk<T>(a: Area, name: string, fn: () => Promise<AaiResult<T>>, extra?: (v: T) => string | undefined): Promise<T | undefined> {
  const r = await safe(a, name, fn);
  if (!r.ok) { a.fail(name, `expected ok, got ${desc(r)}`); return undefined; }
  const bad = extra?.(r.value);
  bad ? a.fail(name, bad) : a.pass(name);
  return r.value;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let corrN = 0;
const corr = (p: string) => `conf-${p}-${++corrN}`;

interface Env { p: AaiProvider; cap: AgentCapabilityManifest; fx: ConformanceFixtures; tokens: { a: string; b: string } }

async function openCtx(a: Area, e: Env, label: string, title = "conformance") {
  const r = await safe(a, label, () => e.p.contextOpen({ agentId: e.fx.agentId, title }));
  if (!r.ok) { a.fail(label, `open failed: ${desc(r)}`); return undefined; }
  return r.value.contextId;
}
async function closeAll(e: Env, ids: Array<string | undefined>) {
  for (const id of ids) if (id) { try { await e.p.contextClose({ contextId: id }); } catch { /* ignore */ } }
}
async function allEvents(a: Area, e: Env, contextId: string, cursor?: string): Promise<CursoredEvent[] | undefined> {
  const r = await safe(a, "events", () => e.p.events({ contextId, cursor }));
  return r.ok ? r.value.events : undefined;
}

// ---------------- areas ----------------
async function identity(a: Area, e: Env) {
  const { p, fx, cap } = e;
  const l = await expectOk(a, "discover: agent.list", () => p.list());
  if (l) a.expect("discover: agent appears in list", l.some((x) => x.agentId === fx.agentId), `agent ${fx.agentId} not listed`);
  await expectOk(a, "agent.get", () => p.get(fx.agentId), (v) => (v.agentId !== fx.agentId ? "agentId mismatch" : undefined));
  a.expect("manifest adapterId matches provider", cap.adapterId === p.adapterId, `${cap.adapterId} != ${p.adapterId}`);
  await expectOk(a, "agent.identity", () => p.identity(fx.agentId));
  const again = await safe(a, "agent.capabilities", () => p.capabilities(fx.agentId));
  a.expect("restore binding: manifest stable across calls", again.ok && JSON.stringify(again.value) === JSON.stringify(cap), "manifest changed between calls");
  const unknown = await safe(a, "agent.get unknown", () => p.get("no-such-agent-" + Date.now()));
  a.expect("unknown agent is an AAIError, not ok", !unknown.ok, "unknown agent returned ok");
  await expectOk(a, "agent.health", () => p.health({ agentId: fx.agentId }));
}

async function context(a: Area, e: Env) {
  const { p, fx, cap } = e;
  if (!cap.context.supported || !cap.messaging.send) {
    await expectUnsupported(a, "context.open", () => p.contextOpen({ agentId: fx.agentId }));
    await expectUnsupported(a, "context.message", () => p.contextMessage({ contextId: "x", correlationId: corr("u"), text: "hi" }));
    return;
  }
  const opened = await safe(a, "context.open", () => p.contextOpen({ agentId: fx.agentId, title: "ctx" }));
  if (!opened.ok) return a.fail("open isolated context", desc(opened));
  const id = opened.value.contextId;
  a.expect("open returns declared isolation + guarantee", opened.value.isolation === cap.context.isolation && opened.value.guarantee === cap.guarantee,
    `open said ${opened.value.isolation}/${opened.value.guarantee}, manifest says ${cap.context.isolation}/${cap.guarantee}`);
  if (cap.context.resume) {
    const re = await expectOk(a, "resume: adopt existing context", () => p.contextOpen({ agentId: fx.agentId, adoptContextId: id }),
      (v) => (v.contextId !== id ? `resume produced a different context ${v.contextId}` : undefined));
    void re;
  } else {
    await expectUnsupported(a, "resume (undeclared)", () => p.contextOpen({ agentId: fx.agentId, adoptContextId: id }));
  }
  const c1 = corr("send");
  await expectOk(a, "send", () => p.contextMessage({ contextId: id, correlationId: c1, text: "hello" }),
    (v) => (v.correlationId !== c1 ? "correlationId not echoed" : !v.messageId ? "no messageId" : undefined));
  if (cap.messaging.steer) await expectOk(a, "steer", () => p.contextSteer({ contextId: id, text: "focus" }));
  else await expectUnsupported(a, "steer (undeclared)", () => p.contextSteer({ contextId: id, text: "focus" }));
  await expectOk(a, "terminate: close", () => p.contextClose({ contextId: id }));
  const after = await safe(a, "send after close", () => p.contextMessage({ contextId: id, correlationId: corr("closed"), text: "x" }));
  a.expect("closed context rejects messages (CONTEXT_NOT_FOUND)", errOf(after)?.code === "CONTEXT_NOT_FOUND", `got ${desc(after)}`);
}

async function isolation(a: Area, e: Env) {
  const { p, cap, tokens } = e;
  if (!cap.context.supported || !cap.messaging.send) return a.skip("isolation", "no execution surface declared");
  if (cap.context.isolation !== "isolated") return a.skip("isolation", `declared ${cap.context.isolation}: isolation not promised`);
  const A = await openCtx(a, e, "open A"), B = await openCtx(a, e, "open B");
  try {
    if (!A || !B) return;
    a.expect("distinct context ids", A !== B, "two opens returned the same context");
    const ra = await safe(a, "send A", () => p.contextMessage({ contextId: A, correlationId: corr("iso-a"), text: `remember ${tokens.a}` }));
    const rb = await safe(a, "send B", () => p.contextMessage({ contextId: B, correlationId: corr("iso-b"), text: `remember ${tokens.b}` }));
    const ea = (await allEvents(a, e, A)) ?? [], eb = (await allEvents(a, e, B)) ?? [];
    const blobA = JSON.stringify([ea, ra.ok ? ra.value : null]), blobB = JSON.stringify([eb, rb.ok ? rb.value : null]);
    a.expect("B never sees A's content", !blobB.includes(tokens.a), "context B contains A's token");
    a.expect("A never sees B's content", !blobA.includes(tokens.b), "context A contains B's token");
    a.expect("events carry their own context id", ea.every((x) => !x.event.remoteContextId || x.event.remoteContextId === A) && eb.every((x) => !x.event.remoteContextId || x.event.remoteContextId === B),
      "event tagged with another context's id");
  } finally { await closeAll(e, [A, B]); }
}

async function parallelism(a: Area, e: Env) {
  const { p, fx, cap } = e;
  if (!cap.context.supported) return a.skip("parallelism", "no contexts declared");
  const limit = cap.context.parallel ? cap.context.maxParallel : 1;
  const opened: string[] = [];
  try {
    const n = limit === 0 ? 3 : limit;
    for (let i = 0; i < n; i++) {
      const r = await safe(a, `open #${i + 1}`, () => p.contextOpen({ agentId: fx.agentId, title: `par${i}` }));
      if (r.ok) opened.push(r.value.contextId); else { a.fail(`open #${i + 1} within declared max (${limit || "unbounded"})`, desc(r)); break; }
    }
    if (limit === 0) return a.expect("unbounded declared: several contexts open", opened.length === n, "could not open 3");
    a.expect(`declared max ${limit} contexts open`, opened.length === limit, `only ${opened.length} opened`);
    if (opened.length === limit) {
      const over = await safe(a, "open over limit", () => p.contextOpen({ agentId: fx.agentId, title: "over" }));
      if (over.ok) opened.push(over.value.contextId);
      a.expect("context beyond max is CONTEXT_BUSY", errOf(over)?.code === "CONTEXT_BUSY", `got ${desc(over)}`);
      await p.contextClose({ contextId: opened[0] });
      const again = await safe(a, "reopen after close", () => p.contextOpen({ agentId: fx.agentId, title: "again" }));
      if (again.ok) opened.push(again.value.contextId);
      a.expect("slot frees after close", again.ok, `got ${desc(again)}`);
    }
  } finally { await closeAll(e, opened); }
}

async function memory(a: Area, e: Env) {
  const { p, cap, fx } = e;
  const m = cap.memory;
  const ctxId = m.read || m.write ? await openCtx(a, e, "open ctx") : undefined;
  const ids: Array<string | undefined> = [ctxId];
  try {
    const key = "conf-key", val = `v-${Date.now()}`;
    if (m.write) await expectOk(a, "memory.write", () => p.memory({ contextId: ctxId, op: "write", key, value: val }));
    else await expectUnsupported(a, "memory.write (undeclared)", () => p.memory({ contextId: ctxId, op: "write", key, value: val }));
    if (m.read) {
      if (m.write) await expectOk(a, "memory.read returns what was written", () => p.memory({ contextId: ctxId, op: "read", key }), (v) => (v.value !== val ? `read ${JSON.stringify(v.value)}` : undefined));
      else await expectOk(a, "memory.read", () => p.memory({ contextId: ctxId, op: "read", key }));
      if (m.write && cap.context.supported) {
        const c2 = await openCtx(a, e, "open second ctx"); ids.push(c2);
        const r = await safe(a, "memory.read in new context", () => p.memory({ contextId: c2, op: "read", key }));
        const saw = r.ok && r.value.value === val;
        if (cap.context.isolation === "stateless") a.expect("ephemeral when declared stateless", !saw, "stateless provider leaked memory across contexts");
        else a.expect("survives across contexts when promised", saw, `memory lost: ${desc(r)}`);
      }
    } else await expectUnsupported(a, "memory.read (undeclared)", () => p.memory({ contextId: ctxId, op: "read", key }));
    if (m.snapshot) await expectOk(a, "memory.snapshot", () => p.memory({ contextId: ctxId, op: "snapshot" }));
    else await expectUnsupported(a, "memory.snapshot (undeclared)", () => p.memory({ contextId: ctxId, op: "snapshot" }));
  } finally { await closeAll(e, ids); }
  void fx;
}

async function events(a: Area, e: Env) {
  const { p, cap } = e;
  if (!cap.context.supported) return a.skip("events", "no contexts declared");
  const exact = cap.guarantee === "exact";
  const id = await openCtx(a, e, "open ctx");
  try {
    if (!id) return;
    const c1 = corr("ev1");
    if (cap.messaging.send) {
      await safe(a, "send 1", () => p.contextMessage({ contextId: id, correlationId: c1, text: "one" }));
      await safe(a, "send 2", () => p.contextMessage({ contextId: id, correlationId: corr("ev2"), text: "two" }));
    }
    const all = await allEvents(a, e, id);
    if (!all) return a.fail("events readable", "events returned an error");
    a.expect("events schema-valid", all.every((x) => gatewayEventSchema.safeParse(x.event).success), "event failed gatewayEventSchema");
    if (cap.messaging.send) a.expect("events reflect sent message (correlationId)", all.some((x) => x.event.correlationId === c1), "no event with the message's correlationId");
    a.expect("events have cursors", all.every((x) => typeof x.cursor === "string" && x.cursor !== ""), "missing cursor");
    const honest = cap.lane === "ui_bridge" ? all.every((x) => x.event.guarantee !== "exact") : true;
    a.expect("event guarantee honest for lane", honest, "ui_bridge events must not claim exact");
    if (exact) a.expect("exact provider: every event guarantee is exact", all.every((x) => x.event.guarantee === "exact"), "non-exact event from exact provider");
    const nums = all.map((x) => Number(x.cursor));
    const rids = all.map((x) => x.event.remoteEventId).filter(Boolean);
    if (exact) {
      a.expect("ordering: cursors strictly increasing", nums.every((n, i) => Number.isFinite(n) && (i === 0 || n > nums[i - 1])), "cursors not strictly increasing");
      a.expect("dedup: no duplicate cursors or remote ids", new Set(all.map((x) => x.cursor)).size === all.length && new Set(rids).size === rids.length, "duplicate events");
    } else {
      a.expect("dedup (relaxed): duplicates must be identifiable by cursor or remoteEventId", all.every((x) => x.cursor || x.event.remoteEventId), "events not dedupable");
      a.skip("ordering", "best_effort: strict ordering relaxed");
    }
    if (cap.events.replay) {
      if (all.length >= 2) {
        const mid = all[Math.floor(all.length / 2) - 1] ?? all[0];
        const suffix = all.slice(all.indexOf(mid) + 1).map((x) => x.cursor);
        const replay = (await allEvents(a, e, id, mid.cursor))?.map((x) => x.cursor) ?? [];
        if (exact) a.expect("cursor replay returns exactly the suffix", JSON.stringify(replay) === JSON.stringify(suffix), `expected ${suffix} got ${replay}`);
        else a.expect("cursor replay (relaxed): includes every later event", suffix.every((c) => replay.includes(c)), "replay missed events");
      } else a.skip("cursor replay", "fewer than 2 events");
      const last = all[all.length - 1]?.cursor;
      if (cap.messaging.send && last) {
        const c3 = corr("ev3");
        await safe(a, "send 3", () => p.contextMessage({ contextId: id, correlationId: c3, text: "three" }));
        const more = (await allEvents(a, e, id, last)) ?? [];
        a.expect("reconnect from last cursor yields only new events", more.some((x) => x.event.correlationId === c3) && (!exact || more.every((x) => Number(x.cursor) > Number(last))), "reconnect missed or repeated events");
      }
    } else {
      await expectUnsupported(a, "cursor replay (undeclared)", () => p.events({ contextId: id, cursor: "1" }));
    }
  } finally { await closeAll(e, [id]); }
}

async function approvals(a: Area, e: Env) {
  const { p, cap, fx } = e;
  const ap = cap.approvals;
  const human = { type: "human" as const, id: "conformance-human" }, robot = { type: "system" as const, id: "conformance-bot" };
  if (ap.read) {
    const l = await expectOk(a, "approvals surfaced (list)", () => p.approvals({ op: "list" }), (v) => (Array.isArray(v.approvals) ? undefined : "no approvals array"));
    void l;
  } else await expectUnsupported(a, "approvals list (undeclared)", () => p.approvals({ op: "list" }));
  const id = fx.approvalId;
  if (!id) return a.skip("respond", "no approvalId fixture");
  const pending = async () => { const r = await p.approvals({ op: "list" }); return r.ok ? r.value.approvals?.find((x) => x.remoteRef === id) : undefined; };
  const robotTry = await safe(a, "system respond", () => p.approvals({ op: "respond", approvalId: id, decision: "approve", actor: robot }));
  a.expect("cannot auto-approve (non-human actor rejected)", !robotTry.ok && ["APPROVAL_REQUIRED", "POLICY_DENIED", "UNSUPPORTED"].includes(robotTry.error.code), `non-human respond returned ${desc(robotTry)}`);
  if (ap.read) {
    const st = await pending();
    a.expect("approval still pending after non-human attempt", !st || st.state === "pending", `state is ${st?.state}`);
  }
  if (ap.respond) {
    await expectOk(a, "human response propagates", () => p.approvals({ op: "respond", approvalId: id, decision: "approve", actor: human }),
      (v) => (v.resolved?.state === "approved" ? undefined : `resolved.state=${v.resolved?.state}`));
    if (ap.read) { const st = await pending(); a.expect("response visible in list", st?.state === "approved", `state is ${st?.state}`); }
  } else await expectUnsupported(a, "human respond (undeclared)", () => p.approvals({ op: "respond", approvalId: id, decision: "approve", actor: human }));
}

async function computer(a: Area, e: Env) {
  const { p, cap } = e;
  const c = cap.computer;
  const id = c.view || c.control || c.takeover ? await openCtx(a, e, "open ctx") : "none";
  try {
    if (!id) return;
    const op = (o: "frame" | "control" | "takeover") => () => p.computer({ contextId: id, op: o });
    if (c.view) {
      await expectOk(a, "computer frames", op("frame"), (v) => (v.frame ? undefined : "no frame"));
      await expectOk(a, "computer frames after disconnect recovery (second call)", op("frame"), (v) => (v.frame ? undefined : "no frame"));
    } else await expectUnsupported(a, "frames (undeclared)", op("frame"));
    if (c.control) await expectOk(a, "computer control", op("control")); else await expectUnsupported(a, "control (undeclared)", op("control"));
    if (c.takeover) await expectOk(a, "computer takeover", op("takeover")); else await expectUnsupported(a, "takeover (undeclared)", op("takeover"));
  } finally { if (id !== "none") await closeAll(e, [id]); }
}

const FAULT_EXPECT: Record<FaultKind, { codes: AAIErrorCode[]; retryable?: boolean }> = {
  vendor_down: { codes: ["VENDOR_UNAVAILABLE"], retryable: true },
  rate_limited: { codes: ["RATE_LIMITED"], retryable: true },
  auth_revoked: { codes: ["AUTH_REVOKED", "AUTH_REQUIRED"], retryable: false },
  account_banned: { codes: ["LANE_BLOCKED", "POLICY_DENIED"], retryable: false },
  ui_changed: { codes: ["ADAPTER_DRIFT"], retryable: false },
};
async function failure(a: Area, e: Env) {
  for (const kind of Object.keys(FAULT_EXPECT) as FaultKind[]) {
    const build = e.fx.faulty?.[kind];
    if (!build) { a.skip(kind, "no fault fixture (not applicable to this adapter)"); continue; }
    const fp = build();
    const r = await safe(a, kind, () => fp.contextOpen({ agentId: e.fx.agentId, title: "fault" }));
    const ex = FAULT_EXPECT[kind];
    if (r.ok) { a.fail(kind, "operation succeeded while vendor was failing"); continue; }
    a.expect(`${kind}: maps to ${ex.codes.join("|")}`, ex.codes.includes(r.error.code), `got ${r.error.code}`);
    if (ex.retryable !== undefined) a.expect(`${kind}: retryable=${ex.retryable}`, r.error.retryable === ex.retryable, `retryable=${r.error.retryable}`);
    if (kind === "rate_limited") a.expect("rate_limited: retryAfterMs surfaced", (r.error.retryAfterMs ?? 0) > 0, "no retryAfterMs");
  }
}

async function idempotency(a: Area, e: Env) {
  const { p, cap } = e;
  if (!cap.context.supported || !cap.messaging.send) return void (await expectUnsupported(a, "context.message (undeclared)", () => p.contextMessage({ contextId: "x", correlationId: corr("u"), text: "x" })));
  const id = await openCtx(a, e, "open ctx");
  try {
    if (!id) return;
    const c = corr("idem");
    const first = await safe(a, "first send", () => p.contextMessage({ contextId: id, correlationId: c, text: "do the thing once" }));
    const second = await safe(a, "replayed send", () => p.contextMessage({ contextId: id, correlationId: c, text: "do the thing once" }));
    a.expect("replay returns the first result", first.ok && second.ok && first.value.messageId === second.value.messageId && first.value.reply === second.value.reply, "replay produced a different result");
    const cc = corr("idem-par");
    const par = await Promise.all([1, 2, 3].map(() => safe(a, "concurrent send", () => p.contextMessage({ contextId: id, correlationId: cc, text: "parallel" }))));
    a.expect("concurrent replays collapse to one result", par.every((r) => r.ok && r.value.messageId === (par[0] as { value: { messageId: string } }).value?.messageId), "concurrent replays diverged");
    const evs = (await allEvents(a, e, id)) ?? [];
    for (const k of [c, cc]) {
      const n = evs.filter((x) => x.event.correlationId === k && x.event.type === "agent.message.completed").length;
      a.expect(`no duplicate work for ${k === c ? "sequential" : "concurrent"} replay`, n <= 1, `${n} completions for one correlationId`);
    }
    const other = await safe(a, "new correlationId", () => p.contextMessage({ contextId: id, correlationId: corr("idem-new"), text: "again" }));
    a.expect("new correlationId is real new work", other.ok && first.ok && other.value.messageId !== first.value.messageId, "distinct correlation reused a messageId");
  } finally { await closeAll(e, [id]); }
}

async function cancellation(a: Area, e: Env) {
  const { p, cap, fx } = e;
  if (!cap.messaging.cancel || !cap.context.supported) return void (await expectUnsupported(a, "cancel (undeclared)", () => p.contextCancel({ contextId: "x" })));
  const id = await openCtx(a, e, "open ctx");
  try {
    if (!id) return;
    await safe(a, "send", () => p.contextMessage({ contextId: id, correlationId: corr("cx"), text: "long task" }));
    const cancelled = await expectOk(a, "cancel", () => p.contextCancel({ contextId: id }), (v) => (typeof v.confirmed === "boolean" ? undefined : "confirmed must be boolean"));
    if (cancelled?.confirmed) {
      const before = ((await allEvents(a, e, id)) ?? []).filter((x) => x.event.type === "agent.message.delta").length;
      await sleep(fx.settleMs ?? 40);
      const after = ((await allEvents(a, e, id)) ?? []).filter((x) => x.event.type === "agent.message.delta").length;
      a.expect("confirmed cancel actually stops work", after === before, `${after - before} message.delta events after confirmed cancel`);
    } else a.skip("stop actually stops", "cancel reported best-effort (confirmed=false)");
    const ghost = await safe(a, "cancel unknown", () => p.contextCancel({ contextId: "no-such-ctx" }));
    a.expect("cancel on unknown context is CONTEXT_NOT_FOUND", errOf(ghost)?.code === "CONTEXT_NOT_FOUND", `got ${desc(ghost)}`);
  } finally { await closeAll(e, [id]); }
}

async function sync(a: Area, e: Env) {
  const { p, fx } = e;
  const r = await safe(a, "agent.sync", () => p.sync({ agentId: fx.agentId }));
  if (!r.ok) return r.error.code === "UNSUPPORTED" ? a.unsup("agent.sync") : a.fail("agent.sync", desc(r));
  a.expect("sync fields are valid MirrorFieldState", r.value.fields.every((f) => mirrorFieldStateSchema.safeParse(f).success), "invalid MirrorFieldState");
  if (fx.remoteMutation) {
    await fx.remoteMutation();
    const after = await safe(a, "sync after remote change", () => p.sync({ agentId: fx.agentId }));
    a.expect("remote change detected (drift shown honestly)", after.ok && after.value.fields.some((f) => f.status !== "synced"), "remote change not reflected");
  } else a.skip("remote change detection", "no remoteMutation fixture");
}

async function resources(a: Area, e: Env) {
  const { p, cap, fx } = e;
  if (cap.tasks.list) await expectOk(a, "agent.tasks", () => p.tasks({ agentId: fx.agentId })); else await expectUnsupported(a, "agent.tasks (undeclared)", () => p.tasks({ agentId: fx.agentId }));
  if (cap.artifacts.read) await expectOk(a, "agent.artifacts", () => p.artifacts({ agentId: fx.agentId })); else await expectUnsupported(a, "agent.artifacts (undeclared)", () => p.artifacts({ agentId: fx.agentId }));
  const s = await safe(a, "agent.snapshot", () => p.snapshot({ agentId: fx.agentId }));
  if (s.ok) a.pass("agent.snapshot"); else s.error.code === "UNSUPPORTED" ? a.unsup("agent.snapshot") : a.fail("agent.snapshot", desc(s));
}

// ---------------- entry ----------------
export async function runConformance(provider: AaiProvider, fixtures: ConformanceFixtures): Promise<ConformanceReport> {
  const areas: Area[] = [];
  const mk = (n: string) => { const a = new Area(n); areas.push(a); return a; };
  const names = ["identity", "context", "isolation", "parallelism", "memory", "events", "approvals", "computer", "failure", "idempotency", "cancellation", "sync", "resources"];
  const fns: Record<string, (a: Area, e: Env) => Promise<unknown>> = { identity, context, isolation, parallelism, memory, events, approvals, computer, failure, idempotency, cancellation, sync, resources };

  const first = mk("identity");
  let cap: AgentCapabilityManifest | undefined;
  try {
    const r = await provider.capabilities(fixtures.agentId);
    const parsed = r.ok ? agentCapabilityManifestSchema.safeParse(r.value) : undefined;
    if (parsed?.success) { cap = parsed.data; first.pass("capabilities manifest valid"); }
    else first.fail("capabilities manifest valid", r.ok ? "manifest failed schema" : desc(r));
  } catch (e) { first.fail("capabilities never throws", (e as Error).message); }

  const tokens = fixtures.tokens ?? { a: "TOKEN_ALPHA_7f3a", b: "TOKEN_BRAVO_91c2" };
  for (const n of names) {
    const a = n === "identity" ? first : mk(n);
    if (!cap) { if (n !== "identity") a.fail("manifest", "no valid capability manifest; area not runnable"); continue; }
    try { await fns[n](a, { p: provider, cap, fx: fixtures, tokens }); }
    catch (err) { a.fail("harness", `unexpected error: ${(err as Error).message}`); }
  }
  const results = areas.map((x) => x.result());
  const summary = {
    pass: results.filter((r) => r.status === "pass").length,
    fail: results.filter((r) => r.status === "fail").length,
    skippedUnsupported: results.filter((r) => r.status === "skipped-unsupported").length,
  };
  return { adapterId: provider.adapterId, guarantee: cap?.guarantee ?? "unknown", lane: cap?.lane ?? "unknown", ok: summary.fail === 0, areas: results, summary };
}

/** Fault-injecting wrapper: named ops return an error (or throw) instead of reaching the provider. */
export function withFaults(provider: AaiProvider, faults: Partial<Record<keyof AaiProvider, AAIError | "throw">>): AaiProvider {
  return new Proxy(provider, {
    get(t, prop, recv) {
      const f = (faults as Record<string, AAIError | "throw" | undefined>)[String(prop)];
      if (f === undefined) { const v = Reflect.get(t, prop, recv); return typeof v === "function" ? v.bind(t) : v; }
      return async () => { if (f === "throw") throw new Error(`injected throw in ${String(prop)}`); return { ok: false, error: f } as AaiResult<never>; };
    },
  });
}
