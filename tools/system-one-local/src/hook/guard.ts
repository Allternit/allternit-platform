// PreToolUse guard logic (testable core; the executable is hooks/pretooluse-guard).
//
// Order is fixed:
//   1. Hard rules (deterministic, final). deny/ask here cannot be changed by the model.
//   2. Only if no hard rule fired: the hazard pack goes to the LOCAL System One server.
//      Its result can only RAISE friction (no decision → ask). It never emits "allow".
//   3. Server down / timeout / error → emit nothing (normal Claude Code permission flow).
//
// Modes (SYSTEM_ONE_HOOK_MODE):
//   log    (default) dry run. Emits NO decision at all, even for hard rules; appends one
//          JSONL record per call to ~/.allternit/system-one/dryrun/<date>.jsonl.
//   advise hard rules emit their deny/ask; the pack may escalate to ask. Also logs.
//   off    do nothing.
import { join } from "node:path";
import { appendJsonl, BASE_DIR, sha256 } from "../log.ts";
import type { SystemOneRequest, SystemOneResponse } from "../types.ts";
import { evaluateHardRules, type HardRuleResult, type ToolCall, type Verdict } from "./hardrules.ts";
import { buildPack, shouldEscalate, thresholdsFromEnv, type Thresholds } from "./pack.ts";

export type Mode = "log" | "advise" | "off";

export interface HookOutput {
  hookSpecificOutput: {
    hookEventName: "PreToolUse";
    permissionDecision: Verdict; // never "allow"
    permissionDecisionReason: string;
  };
}

export interface GuardDeps {
  mode?: Mode;
  serverUrl?: string;
  timeoutMs?: number;
  thresholds?: Thresholds;
  fetchImpl?: (url: string, init?: RequestInit) => Promise<Response>;
  logDir?: string | null; // null disables logging (tests)
  now?: () => Date;
}

export interface GuardRecord {
  ts: string;
  mode: Mode;
  tool: string;
  hard_rule: { verdict: Verdict; rule: string } | null;
  settled_by: "hard_rule" | "jev" | "none";
  pack: { state_sha256: string; state_chars: number; token_estimate: number; question_ids: string[] } | null;
  redactions: { secret: number; email: number; client: number; possible_name: number } | null;
  server: "ok" | "down" | "timeout" | "error" | "skipped";
  usage: { input_tokens: number; output_tokens: number } | null;
  answers: Record<string, number> | null;
  methods: Record<string, string> | null;
  would_escalate: boolean;
  escalate_reasons: string[];
  decision_emitted: Verdict | null;
  latency_ms: number;
}

export function modeFromEnv(env = process.env): Mode {
  const m = env.SYSTEM_ONE_HOOK_MODE;
  return m === "advise" || m === "off" ? m : "log";
}

function out(v: Verdict, reason: string): HookOutput {
  return { hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: v, permissionDecisionReason: reason } };
}

export async function runGuard(input: any, deps: GuardDeps = {}): Promise<{ output: HookOutput | null; record: GuardRecord | null }> {
  const mode = deps.mode ?? modeFromEnv();
  if (mode === "off" || !input || typeof input.tool_name !== "string") return { output: null, record: null };
  const t0 = performance.now();
  const call: ToolCall = { tool_name: input.tool_name, tool_input: input.tool_input ?? {}, cwd: input.cwd };
  const record: GuardRecord = {
    ts: (deps.now ?? (() => new Date()))().toISOString(),
    mode,
    tool: call.tool_name,
    hard_rule: null,
    settled_by: "none",
    pack: null,
    redactions: null,
    server: "skipped",
    usage: null,
    answers: null,
    methods: null,
    would_escalate: false,
    escalate_reasons: [],
    decision_emitted: null,
    latency_ms: 0,
  };

  let hard: HardRuleResult | null = null;
  try {
    hard = evaluateHardRules(call);
  } catch {
    hard = null;
  }

  let output: HookOutput | null = null;
  if (hard) {
    record.hard_rule = { verdict: hard.verdict, rule: hard.rule };
    record.settled_by = "hard_rule";
    if (mode === "advise") output = out(hard.verdict, `[hard rule ${hard.rule}] ${hard.reason}`);
  } else {
    const pack = buildPack(call);
    record.pack = {
      state_sha256: sha256(JSON.stringify(pack.request.state)),
      state_chars: pack.stateChars,
      token_estimate: pack.tokenEstimate,
      question_ids: Object.keys(pack.request.questions),
    };
    record.redactions = pack.flags;
    const res = await callServer(pack.request, deps);
    record.server = res.status;
    if (res.body) {
      record.usage = res.body.usage;
      record.answers = Object.fromEntries(
        Object.entries(res.body.answers).map(([k, a]) => [k, a.type === "noul" ? a.noul : a.type === "score" ? a.score : NaN]),
      );
      record.methods = res.body.x_allternit?.methods ?? null;
      const esc = shouldEscalate(res.body, deps.thresholds ?? thresholdsFromEnv());
      record.would_escalate = esc.escalate;
      record.escalate_reasons = esc.reasons;
      if (esc.escalate) {
        record.settled_by = "jev";
        if (mode === "advise") {
          output = out("ask", `[system-one advisory] hazard signals ${esc.reasons.join(", ")} — confirm before running`);
        }
      }
    }
  }
  // Invariant: this hook never emits allow, and in log mode never emits anything.
  if (mode === "log") output = null;
  record.decision_emitted = output?.hookSpecificOutput.permissionDecision ?? null;
  record.latency_ms = Math.round(performance.now() - t0);
  if (deps.logDir !== null) {
    try {
      appendJsonl(deps.logDir ?? process.env.SYSTEM_ONE_DRYRUN_DIR ?? join(BASE_DIR, "dryrun"), record);
    } catch {
      // never block on logging
    }
  }
  return { output, record };
}

/**
 * Talks to the canonical /v1/decision route (ABI DecisionRequestV1/DecisionResultV1): one BELIEF
 * (noul) or SCORE request per pack question, then folds the results back into the legacy
 * SystemOneResponse shape the escalation logic reads. /v1/systemone remains served for SDK
 * clients but the hook no longer uses it, so there is a single decision path.
 */
async function callServer(
  body: SystemOneRequest, deps: GuardDeps,
): Promise<{ status: GuardRecord["server"]; body?: SystemOneResponse }> {
  const url = `${(deps.serverUrl ?? process.env.SYSTEM_ONE_URL ?? "http://127.0.0.1:7717").replace(/\/$/, "")}/v1/decision`;
  const f = deps.fetchImpl ?? fetch;
  const state = typeof body.state === "string" ? body.state : JSON.stringify(body.state);
  const signal = AbortSignal.timeout(deps.timeoutMs ?? Number(process.env.SYSTEM_ONE_HOOK_TIMEOUT_MS ?? 4000));
  const str = (v: unknown) => (typeof v === "string" ? v : JSON.stringify(v));
  const now = (deps.now ?? (() => new Date()))().toISOString();
  const envelope = {
    abi_version: "1.0.0", schema_id: "allternit.kernel.DecisionRequestV1", schema_version: "1.0.0",
    run_id: "hook", session_id: "hook", task_id: "pretooluse", state_version: 0, created_at: now, producer: "system-one-hook", trace_id: "hook", provenance: [],
  };
  try {
    const entries = Object.entries(body.questions);
    const results = await Promise.all(entries.map(async ([qid, q]) => {
      const request = {
        envelope, operation: q.type === "score" ? "SCORE" : "BELIEF", state_projection_ref: "state.hook", decision_bank_id: "bank.pretooluse_guard",
        question_id: qid, instructions: str(q.instructions), latency_class: "REALTIME",
        ...(q.type === "score" ? { scale: q.criteria.map(str) } : {}),
        ...(q.type === "noul" && q.criteria ? { extensions: { "x-criteria": q.criteria } } : {}),
      };
      const res = await f(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ request, state }), signal });
      if (!res.ok) throw Object.assign(new Error(`status ${res.status}`), { name: "HttpError" });
      return [qid, q, (await res.json()) as any] as const;
    }));
    const answers: SystemOneResponse["answers"] = {};
    const methods: Record<string, "logprobs" | "sampled" | "remote"> = {};
    const usage = { input_tokens: 0, output_tokens: 0 };
    for (const [qid, q, r] of results) {
      const p: Record<string, number> | null = r?.probabilities ?? null;
      if (!p) return { status: "error" };
      if (q.type === "score") {
        const probabilities = p;
        const score = Object.entries(p).reduce((a, [k, v]) => a + Number(k) * v, 0);
        answers[qid] = { type: "score", score, legend: {}, probabilities, confidence: r.confidence ?? 0 };
      } else {
        answers[qid] = { type: "noul", noul: p.true ?? 0 };
      }
      methods[qid] = r.extensions?.["x-readout_method"] ?? "sampled";
      usage.input_tokens += r.extensions?.["x-usage"]?.input_tokens ?? 0;
      usage.output_tokens += r.extensions?.["x-usage"]?.output_tokens ?? 0;
    }
    return { status: "ok", body: { model: "decision", answers, usage, x_allternit: { backend: "decision", methods, latency_ms: 0 } } };
  } catch (e) {
    const name = (e as Error)?.name;
    if (name === "HttpError") return { status: "error" };
    return { status: name === "TimeoutError" || name === "AbortError" ? "timeout" : "down" };
  }
}
