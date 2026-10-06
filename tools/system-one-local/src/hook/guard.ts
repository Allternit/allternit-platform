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
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { appendJsonl, BASE_DIR, sha256 } from "../log.ts";
import type { SystemOneRequest, SystemOneResponse } from "../types.ts";
import { evaluateHardRules, type HardRuleResult, type ToolCall, type Verdict } from "./hardrules.ts";
import { buildPack, shouldEscalate, thresholdsFromEnv, type Thresholds } from "./pack.ts";
import { incumbentGateAnswer, reportOutcome, shadowGate, tighten, type Friction } from "../decision/client.ts";

/** Shadow-ledger bank + primitive for the CLI guard's S1 permission GATE. */
export const GUARD_GATE = { bank: "bank.permission_gate", primitive: "permission.cli_guard", question: "may_proceed" } as const;
export const subjectRef = (toolUseId: unknown) => (typeof toolUseId === "string" && toolUseId ? `cc-tool:${toolUseId}` : undefined);

export type Mode = "log" | "advise" | "off";

/**
 * Q26 (#1148): what the harness itself does with a call the hook leaves alone. Under Q24
 * harnesses run in auto-approve mode (`permission_mode: "bypassPermissions"`), so the call
 * proceeds ("allow"). In any other mode the harness may ask the person, so the incumbent
 * is unknown here and no `x-incumbent` is sent. SYSTEM_ONE_HARNESS_AUTO_APPROVE=1 asserts
 * auto-approve for harnesses that don't report a permission_mode.
 */
export function harnessIncumbent(input: any, env = process.env): Friction | null {
  if (input?.permission_mode === "bypassPermissions") return "allow";
  if (input?.permission_mode == null && env.SYSTEM_ONE_HARNESS_AUTO_APPROVE === "1") return "allow";
  return null;
}

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
  /** Pending-permission store (deny labels); null disables it. Default ~/.allternit/system-one/pending. */
  pendingDir?: string | null;
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
  /** Shadow S1 GATE (Q27): logged to the shared ledger, never changes what the hook emits. */
  s1_gate: { decision_id: string | null; p_true: number | null; recommendation: Friction | null; tightened: Friction } | null;
  latency_ms: number;
}

export function modeFromEnv(env = process.env): Mode {
  const m = env.SYSTEM_ONE_HOOK_MODE;
  return m === "advise" || m === "off" ? m : "log";
}

function out(v: Verdict, reason: string): HookOutput {
  return { hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: v, permissionDecisionReason: reason } };
}

/**
 * Harnesses name the tool-call id differently (Claude Code `tool_use_id`;
 * Qwen/Gemini-family and others `tool_call_id`, `toolCallId`, `callId`).
 * Every hook entry reads `tool_use_id`, so copy the first one present.
 */
export function normalizeHookInput(input: any): any {
  if (!input || typeof input !== "object" || (typeof input.tool_use_id === "string" && input.tool_use_id)) return input;
  for (const k of ["tool_call_id", "toolCallId", "toolUseId", "callId", "call_id"]) {
    if (typeof input[k] === "string" && input[k]) return { ...input, tool_use_id: input[k] };
  }
  return input;
}

export async function runGuard(input: any, deps: GuardDeps = {}): Promise<{ output: HookOutput | null; record: GuardRecord | null }> {
  input = normalizeHookInput(input);
  const mode = deps.mode ?? modeFromEnv();
  if (mode === "off" || !input || typeof input.tool_name !== "string") return { output: null, record: null };
  const t0 = performance.now();
  rememberCall(input, deps.pendingDir === undefined && deps.logDir === null ? null : deps.pendingDir);
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
    s1_gate: null,
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
    const runGate = (incumbent: Friction | null) =>
      shadowGate(
        {
          producer: "system-one-hook", decision_bank_id: GUARD_GATE.bank, question_id: GUARD_GATE.question, motif: "GATE",
          primitive_id: GUARD_GATE.primitive, subject_ref: subjectRef(input.tool_use_id),
          instructions: "Should this tool call proceed without asking the person first? Answer true only if it is clearly safe and routine.",
          ...(incumbentGateAnswer(incumbent) ? { extensions: { "x-incumbent": incumbentGateAnswer(incumbent) } } : {}),
        },
        typeof pack.request.state === "string" ? pack.request.state : JSON.stringify(pack.request.state),
        { url: deps.serverUrl, fetchImpl: deps.fetchImpl, timeoutMs: deps.timeoutMs },
      );
    const harness = harnessIncumbent(input);
    // log mode never emits, so the incumbent is the harness's own flow and the GATE runs in
    // parallel. advise mode may emit "ask", so the GATE waits for the server's escalation.
    let res: Awaited<ReturnType<typeof callServer>>, gate: Awaited<ReturnType<typeof shadowGate>>;
    if (mode === "log") {
      [res, gate] = await Promise.all([callServer(pack.request, deps), runGate(harness)]);
    } else {
      res = await callServer(pack.request, deps);
      const escalates = res.body ? shouldEscalate(res.body, deps.thresholds ?? thresholdsFromEnv()).escalate : false;
      gate = await runGate(escalates ? "ask" : harness);
    }
    if (gate) record.s1_gate = { decision_id: gate.decision_id, p_true: gate.p_true, recommendation: gate.recommendation, tightened: "allow" };
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
  // What tighten-only S1 would have made of the hook's decision (shadow: recorded, not emitted).
  if (record.s1_gate) record.s1_gate.tightened = tighten((output?.hookSpecificOutput.permissionDecision ?? "allow") as Friction, record.s1_gate.recommendation);
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

// ---------------------------------------------------------------- outcome labels (WP-S1U-3)
//
// The shadow GATEs about one tool call (this guard's, the Factory Gate judge's, the judge first
// pass) all carry x-subject_ref = cc-tool:<tool_use_id>. Outcome labels, by subject_ref:
//   PostToolUse         the call ran → true
//   PostToolUseFailure  the call ran (and failed) → true: it was allowed
//   PermissionRequest   the person was asked; remembered in a small pending-file store
//                       (<pending>/<session>/ask-<key>) keyed by session + tool-call id
//   Stop / SessionEnd   every PermissionRequest with no PostToolUse/Failure since → false
//                       (the person denied it), then the session's pending files are dropped
// No question_id is sent: the ledger labels the latest decision of every primitive on that call.

export const OUTCOME_SOURCES = {
  ran: "cli_hook.post_tool_use",
  ranFailed: "cli_hook.post_tool_use_failure",
  denied: "cli_hook.permission_denied",
} as const;

type OutcomeDeps = Pick<GuardDeps, "serverUrl" | "fetchImpl" | "timeoutMs" | "pendingDir">;

const safeName = (s: string) => s.replace(/[^A-Za-z0-9_.-]/g, "_").slice(0, 128);
const callHash = (input: any) => sha256(`${input?.tool_name ?? ""}\n${JSON.stringify(input?.tool_input ?? {})}`).slice(0, 32);

function pendingRoot(dir: string | null | undefined): string | null {
  if (dir === null) return null;
  return dir ?? process.env.SYSTEM_ONE_PENDING_DIR ?? join(BASE_DIR, "pending");
}

function sessionDir(input: any, dir: string | null | undefined): string | null {
  const root = pendingRoot(dir);
  const sid = typeof input?.session_id === "string" && input.session_id ? input.session_id : null;
  return root && sid ? join(root, safeName(sid)) : null;
}

function write(path: string, body: string) {
  mkdirSync(join(path, ".."), { recursive: true, mode: 0o700 });
  writeFileSync(path, body, { mode: 0o600 });
}

const rm = (path: string) => {
  try { unlinkSync(path); } catch { /* not there */ }
};

/** PreToolUse: remember tool input → tool_use_id, since PermissionRequest inputs may lack the id. */
export function rememberCall(input: any, dir?: string | null) {
  try {
    const sd = sessionDir(input, dir);
    if (!sd || typeof input?.tool_use_id !== "string" || !input.tool_use_id) return;
    write(join(sd, `seen-${callHash(input)}`), input.tool_use_id);
  } catch { /* best-effort */ }
}

/** PermissionRequest: the person is being asked about this call. */
export function recordPermissionRequest(input: any, dir?: string | null): boolean {
  try {
    const sd = sessionDir(input, dir);
    if (!sd) return false;
    const h = callHash(input);
    let id: string | undefined = typeof input?.tool_use_id === "string" && input.tool_use_id ? input.tool_use_id : undefined;
    const seen = join(sd, `seen-${h}`);
    if (!id && existsSync(seen)) id = readFileSync(seen, "utf8").trim() || undefined;
    const subject_ref = subjectRef(id);
    if (!subject_ref) return false;
    write(join(sd, `ask-${safeName(id!)}`), JSON.stringify({ subject_ref, tool: input.tool_name, hash: h, ts: new Date().toISOString() }));
    return true;
  } catch {
    return false;
  }
}

function clearPending(input: any, dir?: string | null) {
  const sd = sessionDir(input, dir);
  if (!sd) return;
  if (typeof input?.tool_use_id === "string" && input.tool_use_id) rm(join(sd, `ask-${safeName(input.tool_use_id)}`));
  if (input?.tool_name) rm(join(sd, `seen-${callHash(input)}`));
}

/**
 * PostToolUse / PostToolUseFailure: the call ran, so the person (or their settings) let it
 * proceed. A failed run was still allowed, so it reports true too.
 */
export async function reportToolRan(input: any, deps: OutcomeDeps = {}): Promise<boolean> {
  const subject_ref = subjectRef(input?.tool_use_id);
  if (!subject_ref || modeFromEnv() === "off") return false;
  try { clearPending(input, deps.pendingDir); } catch { /* best-effort */ }
  const source = input?.hook_event_name === "PostToolUseFailure" ? OUTCOME_SOURCES.ranFailed : OUTCOME_SOURCES.ran;
  return reportOutcome(
    { subject_ref, truth: "true", source },
    { url: deps.serverUrl, fetchImpl: deps.fetchImpl, timeoutMs: deps.timeoutMs },
  );
}

/** Stop / SessionEnd: every asked call that never ran was denied. Returns how many were labelled. */
export async function reportDenied(input: any, deps: OutcomeDeps = {}): Promise<number> {
  const sd = sessionDir(input, deps.pendingDir);
  if (!sd || !existsSync(sd)) return 0;
  let n = 0;
  try {
    if (modeFromEnv() !== "off") {
      for (const f of readdirSync(sd).filter((x) => x.startsWith("ask-"))) {
        let subject_ref: string | undefined;
        try { subject_ref = JSON.parse(readFileSync(join(sd, f), "utf8")).subject_ref; } catch { /* corrupt */ }
        if (!subject_ref) continue;
        const ok = await reportOutcome(
          { subject_ref, truth: "false", source: OUTCOME_SOURCES.denied },
          { url: deps.serverUrl, fetchImpl: deps.fetchImpl, timeoutMs: deps.timeoutMs },
        );
        if (ok) n++;
      }
    }
  } finally {
    try { rmSync(sd, { recursive: true, force: true }); } catch { /* best-effort */ }
  }
  return n;
}

/** One entry for every outcome hook event (hooks/s1-outcome). Never prints a decision. */
export async function handleOutcomeHook(input: any, deps: OutcomeDeps = {}): Promise<void> {
  input = normalizeHookInput(input);
  switch (input?.hook_event_name) {
    case "PermissionRequest":
      if (modeFromEnv() !== "off") recordPermissionRequest(input, deps.pendingDir);
      return;
    case "Stop":
    case "SessionEnd":
      await reportDenied(input, deps);
      return;
    default: // PostToolUse, PostToolUseFailure (and the legacy posttooluse-outcome entry)
      await reportToolRan(input, deps);
  }
}
