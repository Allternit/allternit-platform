// The one TypeScript client for the S1 decision runtime (`POST /v1/decision`,
// `POST /v1/decision/outcome`). Used by the CLI guard hook and by gizzi-code's
// permission layer (imported by relative path, so this file has no runtime
// imports). Every call is fail-soft: an unreachable or erroring runtime returns
// null/false and never throws into the caller.
//
// Permission law (Q26): S1 may only TIGHTEN a permission decision. The policy
// floor / incumbent decides first; `tighten()` is the only way an S1
// recommendation may be combined with it, and it can never produce a looser
// action than the incumbent's.

export type Friction = "allow" | "ask" | "deny";
export type GateMotif = "GATE" | "CONFIDENCE_GATE";

export interface DecisionClientOptions {
  url?: string;
  token?: string | null;
  timeoutMs?: number;
  /** Backend routed by the runtime: "auto" (Laya while healthy, else local), "laya_bundled", "jev_api", or omitted. */
  backend?: string;
  fetchImpl?: (url: string, init?: RequestInit) => Promise<Response>;
  enabled?: boolean;
}

export function clientFromEnv(env: Record<string, string | undefined> = process.env): Required<Omit<DecisionClientOptions, "fetchImpl">> {
  const url = env.ALLTERNIT_S1_URL ?? env.SYSTEM_ONE_URL ?? "http://127.0.0.1:7717";
  return {
    url: url.replace(/\/$/, ""),
    token: env.SYSTEM_ONE_TOKEN || null,
    timeoutMs: Number(env.ALLTERNIT_S1_TIMEOUT_MS ?? 1500),
    backend: env.ALLTERNIT_S1_BACKEND ?? "auto",
    enabled: env.ALLTERNIT_S1_SHADOW_GATES !== "0",
  };
}

function resolve(opts: DecisionClientOptions) {
  const base = clientFromEnv();
  return {
    url: (opts.url ?? base.url).replace(/\/$/, ""),
    token: opts.token === undefined ? base.token : opts.token,
    timeoutMs: opts.timeoutMs ?? base.timeoutMs,
    backend: opts.backend ?? base.backend,
    enabled: opts.enabled ?? base.enabled,
    fetchImpl: opts.fetchImpl ?? fetch,
  };
}

function headers(token: string | null) {
  return { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) };
}

export function envelope(producer: string, ids: { run_id?: string; session_id?: string; task_id?: string } = {}) {
  return {
    abi_version: "1.0.0", schema_id: "allternit.kernel.DecisionRequestV1", schema_version: "1.0.0",
    run_id: ids.run_id ?? producer, session_id: ids.session_id ?? producer, task_id: ids.task_id ?? producer,
    state_version: 0, created_at: new Date().toISOString(), producer, trace_id: ids.run_id ?? producer, provenance: [],
  };
}

export interface GateSpec {
  producer: string;
  decision_bank_id: string;
  question_id: string;
  instructions: string;
  /** Motif id recorded as `x-motif`; GATE = "may this proceed", CONFIDENCE_GATE = "is the upstream answer trustworthy". */
  motif?: GateMotif;
  /** Shadow-ledger primitive id (defaults to the bank id at the runtime). */
  primitive_id?: string;
  /** Join key for outcome labels reported later by subject_ref. */
  subject_ref?: string;
  ids?: { run_id?: string; session_id?: string; task_id?: string };
  extensions?: Record<string, unknown>;
}

export function gateRequest(spec: GateSpec) {
  return {
    envelope: envelope(spec.producer, spec.ids),
    operation: "GATE",
    state_projection_ref: `state.${spec.producer}`,
    decision_bank_id: spec.decision_bank_id,
    question_id: spec.question_id,
    instructions: spec.instructions,
    latency_class: "INTERACTIVE",
    extensions: {
      "x-motif": spec.motif ?? "GATE",
      ...(spec.primitive_id ? { "x-primitive_id": spec.primitive_id } : {}),
      ...(spec.subject_ref ? { "x-subject_ref": spec.subject_ref } : {}),
      ...(spec.extensions ?? {}),
    },
  };
}

export interface DecisionResult {
  answer: unknown;
  probabilities?: Record<string, number> | null;
  confidence: number;
  threshold_action: string;
  backend_id?: string | null;
  extensions?: Record<string, unknown>;
}

/** POST /v1/decision. Returns null on any failure. */
export async function decide(request: unknown, state: string, opts: DecisionClientOptions & { reversible?: boolean } = {}): Promise<DecisionResult | null> {
  const o = resolve(opts);
  if (!o.enabled) return null;
  try {
    const res = await o.fetchImpl(`${o.url}/v1/decision`, {
      method: "POST", headers: headers(o.token),
      body: JSON.stringify({ request, state, ...(o.backend ? { backend: o.backend } : {}), ...(opts.reversible ? { reversible: true } : {}) }),
      signal: AbortSignal.timeout(o.timeoutMs),
    });
    if (!res.ok) return null;
    return (await res.json()) as DecisionResult;
  } catch {
    return null;
  }
}

export interface OutcomeLabel { decision_id?: string | null; subject_ref?: string | null; question_id?: string | null; truth: string; source: string }

/** POST /v1/decision/outcome. Returns whether the runtime accepted it; never throws. */
export async function reportOutcome(label: OutcomeLabel, opts: DecisionClientOptions = {}): Promise<boolean> {
  const o = resolve(opts);
  if (!o.enabled || (!label.decision_id && !label.subject_ref)) return false;
  try {
    const res = await o.fetchImpl(`${o.url}/v1/decision/outcome`, {
      method: "POST", headers: headers(o.token), body: JSON.stringify(label), signal: AbortSignal.timeout(o.timeoutMs),
    });
    return res.ok;
  } catch {
    return false;
  }
}

export const decisionId = (r: DecisionResult | null): string | null =>
  typeof r?.extensions?.["x-decision_id"] === "string" ? (r.extensions["x-decision_id"] as string) : null;

/** P(true) of a GATE/BELIEF/VERIFY result, or null. */
export const pTrue = (r: DecisionResult | null): number | null =>
  typeof r?.probabilities?.true === "number" ? r.probabilities.true : null;

/** Map P(may proceed) to a permission recommendation. Uncalibrated numbers are advisory only. */
export function gateRecommendation(p: number | null, t: { allowAt?: number; denyBelow?: number } = {}): Friction | null {
  if (p === null || !Number.isFinite(p)) return null;
  if (p >= (t.allowAt ?? 0.8)) return "allow";
  if (p <= (t.denyBelow ?? 0.2)) return "deny";
  return "ask";
}

const RANK: Record<Friction, number> = { allow: 0, ask: 1, deny: 2 };

/**
 * Combine the incumbent decision with an S1 recommendation. Tighten-only: the
 * result is never looser than the incumbent, so an S1 "allow" can never turn
 * ask/deny into allow. A missing recommendation leaves the incumbent unchanged.
 */
export function tighten(incumbent: Friction, s1: Friction | null | undefined): Friction {
  if (!s1 || !(s1 in RANK)) return incumbent;
  return RANK[s1] > RANK[incumbent] ? s1 : incumbent;
}

/**
 * Q26 (#1148): the incumbent's answer to a permission GATE (`may_proceed`), for
 * `x-incumbent`. allow = "true", deny = "false"; ask defers to the person, so it has
 * no answer of its own (undefined: send nothing).
 */
export function incumbentGateAnswer(incumbent: Friction | string | null | undefined): "true" | "false" | undefined {
  return incumbent === "allow" ? "true" : incumbent === "deny" ? "false" : undefined;
}

/** One shadow GATE: returns the decision id, P(true) and the recommendation, or null. */
export async function shadowGate(spec: GateSpec, state: string, opts: DecisionClientOptions = {}) {
  const r = await decide(gateRequest(spec), state, opts);
  if (!r) return null;
  const p = pTrue(r);
  return { decision_id: decisionId(r), p_true: p, recommendation: gateRecommendation(p), result: r };
}
