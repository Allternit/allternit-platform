// Schemas and types for the Gizzi eval loop (CONTRACT.md sections 2-6).
// Trajectory and grader field names follow graders.rs; the Rust names win.
import z from "zod/v4"

// ── caps ────────────────────────────────────────────────────────────────────
export const CASE_LIMIT = 100
export const MAX_TURNS = 10
export const MAX_TITLE_CHARS = 80
export const MAX_NOTE_CHARS = 2000
export const MAX_WORKSPACE_BYTES = 50 * 1024 * 1024
export const MAX_GRADERS_PER_CASE = 32
export const MAX_CHECKS_PER_CASE = 32
export const MAX_K = 10
export const ESTIMATE_TTL_MS = 10 * 60 * 1000
export const MAX_RUNS_LISTED = 50

export const CASE_SCHEMA = "allternit.gizzi_eval_case.v1"
export const SUITE_SCHEMA = "allternit.eval_suite.v1"
export const REPORT_SCHEMA = "allternit.eval_report.v1"
export const BASELINE_SCHEMA = "allternit.eval_baseline.v1"

// ── trajectory (Rust field names) ───────────────────────────────────────────
export const Step = z.object({ node: z.string(), primitive: z.string(), ok: z.boolean() })
export const Effect = z.object({ node: z.string(), tool: z.string(), committed: z.boolean() })
export const Trajectory = z.object({
  status: z.string(),
  verifier: z.boolean().nullable().optional(),
  steps: z.array(Step),
  effects: z.array(Effect),
  texts: z.array(z.string()).optional(),
  stopped_at: z.string().nullable().optional(),
  missing: z.array(z.string()).optional(),
  cost_usd: z.number().min(0),
  tokens: z.number().min(0),
  latency_ms: z.number().min(0),
})
export type Trajectory = z.infer<typeof Trajectory>

// ── graders (graders.rs params) ─────────────────────────────────────────────
const StepIds = z.array(z.string().min(1).max(128)).min(1).max(64)
export const GraderSpec = z.discriminatedUnion("type", [
  z.object({ type: z.literal("step_order"), steps: StepIds }).strict(),
  z.object({ type: z.literal("required_steps"), steps: StepIds }).strict(),
  z.object({ type: z.literal("forbidden_step"), steps: StepIds }).strict(),
  z.object({ type: z.literal("effect_count"), min: z.number().min(0).optional(), max: z.number().min(0).optional(), tool: z.string().optional() }).strict(),
  z.object({ type: z.literal("must_cite"), min: z.number().min(0).optional(), marker: z.string().min(1).max(32).optional() }).strict(),
  z.object({ type: z.literal("verifier_pass") }).strict(),
  z.object({ type: z.literal("cost_ceiling"), max_usd: z.number().min(0).optional(), max_tokens: z.number().min(0).optional() }).strict(),
  z.object({ type: z.literal("latency_ceiling"), max_ms: z.number().min(0) }).strict(),
  z.object({ type: z.literal("status"), equals: z.string().min(1) }).strict(),
  z.object({ type: z.literal("stopped_at"), step: z.string().min(1) }).strict(),
  z.object({ type: z.literal("missing_evidence"), equals: z.array(z.string()) }).strict(),
])
export type GraderSpec = z.infer<typeof GraderSpec>

export const GradeResult = z.object({ type: z.string(), pass: z.boolean(), detail: z.string() })
export type GradeResult = z.infer<typeof GradeResult>

// ── checks (run against the workspace after a live attempt) ─────────────────
export const Check = z.discriminatedUnion("type", [
  z.object({ type: z.literal("tests"), command: z.string().min(1), files: z.record(z.string(), z.string()).optional() }),
  z.object({ type: z.literal("contains"), path: z.string().min(1), text: z.string(), any: z.array(z.string()).optional(), ignore_case: z.boolean().optional() }),
  z.object({ type: z.literal("absent"), path: z.string().min(1), text: z.string(), any: z.array(z.string()).optional(), ignore_case: z.boolean().optional() }),
  z.object({ type: z.literal("unchanged"), path: z.string().min(1) }),
  z.object({ type: z.literal("exists"), path: z.string().min(1) }),
])
export type Check = z.infer<typeof Check>

// ── suite file ──────────────────────────────────────────────────────────────
export const SuiteCaseRef = z.object({
  id: z.string(),
  source: z.object({ kind: z.enum(["session", "workspace_task"]), path: z.string() }),
})
export const Suite = z.object({
  schema: z.literal(SUITE_SCHEMA),
  id: z.string(),
  title: z.string(),
  runner: z.literal("gizzi"),
  cases: z.array(SuiteCaseRef),
})
export type Suite = z.infer<typeof Suite>

// ── case file ───────────────────────────────────────────────────────────────
export const Label = z.enum(["good", "bad"])
export const QuickCheck = z.enum(["wrong_answer", "broke_file", "ignored_instructions", "too_slow", "other"])
export const Surface = z.enum(["desktop", "web", "phone", "ios", "android", "cli", "tui"])
export const Split = z.enum(["dev", "holdout"])
export const PassRule = z.union([z.literal("all_of_k"), z.object({ n_of_k: z.number().int().min(1).max(MAX_K) })])

export const Workspace = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("archive"), path: z.string(), bytes: z.number().int().min(0), snapshot: z.string() }),
  z.object({ kind: z.literal("none"), reason: z.enum(["not_git", "snapshot_off", "pruned", "too_large", "no_project"]) }),
])
export type Workspace = z.infer<typeof Workspace>

export const Case = z.object({
  schema: z.literal(CASE_SCHEMA),
  id: z.string(),
  suite: z.string(),
  title: z.string().max(MAX_TITLE_CHARS),
  label: Label,
  note: z.string().max(MAX_NOTE_CHARS).optional(),
  quick_check: z.array(QuickCheck).optional(),
  split: Split.default("dev"),
  origin: z.object({
    session_id: z.string(),
    through_message_id: z.string().nullable(),
    saved_at: z.number(),
    gizzi_version: z.string(),
    surface: Surface,
  }),
  turns: z.array(z.object({ role: z.literal("user"), text: z.string() })).min(1).max(MAX_TURNS),
  workspace: Workspace,
  checks: z.array(Check).max(MAX_CHECKS_PER_CASE).default([]),
  effects: z.enum(["block", "allow"]).default("block"),
  graders: z.array(GraderSpec).max(MAX_GRADERS_PER_CASE),
  k: z.number().int().min(1).max(MAX_K).default(1),
  pass_rule: PassRule.default("all_of_k"),
  recorded: z.object({ trajectory: Trajectory, grades: z.array(GradeResult), passed: z.boolean() }).nullable().optional(),
  needs_check: z.boolean().default(false),
  redaction: z.object({ secrets: z.number().int(), paths: z.number().int(), entries_dropped: z.number().int() }),
})
export type Case = z.infer<typeof Case>

// ── report and baseline (what scripts/agency-eval-gate.mjs compareReports reads) ─
export const Ci95 = z.object({
  low: z.number(),
  high: z.number(),
  method: z.string().optional(),
  resamples: z.number().optional(),
  seed: z.number().optional(),
})
export const SuiteMetrics = z.object({
  cases: z.number().int(),
  graded: z.number().int(),
  skipped: z.number().int().optional(),
  passed: z.number().int(),
  pass_rate: z.number().nullable(),
  attempt_pass_rate: z.number().nullable().optional(),
  ci95: Ci95.nullable().optional(),
  high_stakes_failures: z.array(z.string()).default([]),
  failed_cases: z.array(z.string()).optional(),
})
export const PerCase = z.object({
  id: z.string(),
  passes: z.number().int(),
  attempts: z.number().int(),
  pass: z.boolean(),
  cost: z.number(),
  tokens: z.number(),
  ms: z.number(),
})
export type PerCase = z.infer<typeof PerCase>
export const Report = z.object({
  schema: z.literal(REPORT_SCHEMA),
  seed: z.number().optional(),
  generated_by: z.string().optional(),
  suites: z.record(z.string(), SuiteMetrics),
  extra: z
    .object({
      mode: z.enum(["replay", "live"]),
      brain: z.string().optional(),
      per_case: z.array(PerCase),
      ci95: Ci95.nullable().optional(),
    })
    .optional(),
})
export type Report = z.infer<typeof Report>
export const Baseline = z.object({
  schema: z.literal(BASELINE_SCHEMA),
  description: z.string().optional(),
  max_drop_pp: z.number().min(0).default(2),
  suites: z.record(z.string(), z.object({ pass_rate: z.number(), graded: z.number().int().optional() })),
})
export type Baseline = z.infer<typeof Baseline>

// ── route bodies and responses (CONTRACT.md section 6) ──────────────────────
export const CaseSummary = z.object({
  id: z.string(),
  suite: z.string(),
  title: z.string(),
  label: Label,
  saved_at: z.number(),
  needs_check: z.boolean(),
  last_result: z.enum(["pass", "fail"]).nullable(),
  last_run_at: z.number().optional(),
  bytes: z.number().int().min(0),
  workspace_kind: z.enum(["archive", "none"]),
  session_id: z.string(),
})
export type CaseSummary = z.infer<typeof CaseSummary>

export const CreateCaseBody = z.object({
  session_id: z.string().min(1),
  label: Label,
  note: z.string().max(MAX_NOTE_CHARS).optional(),
  through_message_id: z.string().optional(),
  quick_check: z.array(QuickCheck).optional(),
  suite: z.string().optional(),
})
export type CreateCaseBody = z.infer<typeof CreateCaseBody>
export const CreateCaseResponse = z.object({
  case: CaseSummary,
  self_test: z.object({ passed: z.boolean(), agrees_with_label: z.boolean() }),
  needs_check: z.boolean(),
})
export type CreateCaseResponse = z.infer<typeof CreateCaseResponse>

export const ListCasesQuery = z.object({ session_id: z.string().optional(), suite: z.string().optional() })
export const ListCasesResponse = z.object({ cases: z.array(CaseSummary) })
export const GetCaseResponse = z.object({ case: Case })

export const SuiteSummary = z.object({
  id: z.string(),
  title: z.string(),
  cases: z.number().int(),
  last_run: z.object({ run_id: z.string(), pass_rate: z.number().nullable(), at: z.number() }).optional(),
})
export const ListSuitesResponse = z.object({ suites: z.array(SuiteSummary) })

export const Billing = z.enum(["plan", "metered", "unknown"])
export const EstimateBody = z.object({
  suite: z.string(),
  case_ids: z.array(z.string()).optional(),
  k: z.number().int().min(1).max(MAX_K).optional(),
  model: z.string().optional(),
})
export type EstimateBody = z.infer<typeof EstimateBody>
export const EstimateResponse = z.object({
  estimate_id: z.string(),
  expires_at: z.number(),
  cases: z.number().int(),
  k: z.number().int(),
  attempts: z.number().int(),
  tokens_est: z.number(),
  minutes_est: z.number(),
  billing: Billing,
  usd_est: z.number().optional(),
  probe_required: z.boolean(),
})
export type EstimateResponse = z.infer<typeof EstimateResponse>

export const RunMode = z.enum(["replay", "live"])
export const StartRunBody = z.object({
  suite: z.string(),
  mode: RunMode,
  case_ids: z.array(z.string()).optional(),
  k: z.number().int().min(1).max(MAX_K).optional(),
  model: z.string().optional(),
  estimate_id: z.string().optional(),
  fraction: z.number().gt(0).max(1).optional(),
  seed: z.number().int().optional(),
})
export type StartRunBody = z.infer<typeof StartRunBody>
/** replay: 200 with the report. live: 202 with run_id only. */
export const StartRunResponse = z.object({ run_id: z.string(), report: Report.optional() })
export type StartRunResponse = z.infer<typeof StartRunResponse>

export const RunState = z.enum(["queued", "running", "done", "failed", "cancelled"])
export const GetRunResponse = z.object({
  run_id: z.string(),
  mode: RunMode,
  state: RunState,
  progress: z.object({ done: z.number().int(), total: z.number().int() }),
  report: Report.optional(),
  error: z.string().optional(),
  started_at: z.number(),
  finished_at: z.number().optional(),
})
export type GetRunResponse = z.infer<typeof GetRunResponse>

export const ListRunsQuery = z.object({ suite: z.string().optional() })
export const RunSummary = z.object({
  run_id: z.string(),
  mode: RunMode,
  state: RunState,
  pass_rate: z.number().nullable().optional(),
  started_at: z.number(),
})
export const ListRunsResponse = z.object({ runs: z.array(RunSummary).max(MAX_RUNS_LISTED) })

/** Error codes the routes return in `{error}` (CONTRACT.md section 6). */
export const EvalErrorCode = z.enum(["session_not_found", "case_limit", "empty_session", "estimate_required", "case_not_found", "run_not_found"])
export const EvalError = z.object({ error: EvalErrorCode, message: z.string().optional() })
