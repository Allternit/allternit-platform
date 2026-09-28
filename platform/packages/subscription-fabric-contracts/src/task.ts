// §S4 — Task, TaskStatus, TaskAttempt, TaskInput; §S7 — TaskError, FailureClass
import { z } from "zod";
import {
  capabilityIdSchema,
  providerIdSchema,
  sensitivitySchema,
} from "./capability";
import { routeDecisionSchema } from "./routing";

// §S7 base list (SPEC §31) **minus** `selector_not_found`, which §A9 folds into
// `provider_ui_changed` with a `detail.locator_key` — callers must not have to
// tell the two apart, and both trip the §A4 circuit breaker.
export const failureClassSchema = z.enum([
  // SPEC §31 base
  "auth_required",
  "quota_exhausted",
  "rate_limited",
  "provider_ui_changed",
  "provider_error",
  "artifact_generation_failed",
  "download_failed",
  "network_error",
  "user_intervention_required",
  "unsupported_capability",
  // §A9 additions
  "challenge_presented",
  "account_restricted",
  "submission_ambiguous",
  "model_downgraded",
  "content_refused",
  "stalled",
  "timeout",
  "output_truncated",
  "profile_locked",
  "worker_crashed",
  "artifact_expired",
  "approval_required",
  "policy_denied",
]);
export type FailureClass = z.infer<typeof failureClassSchema>;

export const taskErrorSchema = z.object({
  class: failureClassSchema,
  // what the failure poisons
  scope: z.enum(["task", "pool", "account", "adapter", "gateway"]),
  retryable: z.boolean(),
  fallback_eligible: z.boolean(),
  cooldown_s: z.number().nullable(),
  user_action: z.string().nullable(), // e.g. "Solve verification in window"
  detail: z.string(), // redacted
  evidence_ref: z.string().nullable(), // redacted screenshot/DOM snapshot id
});
export type TaskError = z.infer<typeof taskErrorSchema>;

// §S4 exact union. `provider_running` = provider is working server-side while
// the worker is detached (Critical #3). `partial` = text succeeded but an
// artifact failed (download/export). `needs_user` pauses the task (auth,
// challenge, approval) — it does not fail it. `artifact_ready` is an *event*,
// not a status: one task can emit several.
export const taskStatusSchema = z.enum([
  "queued",
  "routing",
  "waiting_worker",
  "running",
  "streaming",
  "provider_running",
  "needs_user",
  "completed",
  "partial",
  "failed",
  "cancelled",
]);
export type TaskStatus = z.infer<typeof taskStatusSchema>;

export const submissionStateSchema = z.enum([
  "not_sent",
  "sent_unconfirmed",
  "acknowledged",
]);
export type SubmissionState = z.infer<typeof submissionStateSchema>;

export const attemptOutcomeSchema = z.enum([
  "success",
  "partial",
  "failed",
  "cancelled",
  "ambiguous",
]);
export type AttemptOutcome = z.infer<typeof attemptOutcomeSchema>;

// §S4 — at-most-once submit bookkeeping (Critical #2). `prompt_fingerprint` is
// sha256(normalized prompt + input hashes); reconcile matches a provider thread
// against it and never resubmits automatically.
export const taskAttemptSchema = z.object({
  attempt_no: z.number().int(),
  adapter_id: z.string(),
  adapter_version: z.string(),
  account_id: z.string(),
  pool_key: z.string(),
  submission_state: submissionStateSchema,
  prompt_fingerprint: z.string(),
  provider_thread_id: z.string().nullable(),
  requested_model_class: z.string().nullable(),
  observed_model: z.string().nullable(), // Critical #4 — silent downgrade detection
  started_at: z.string(),
  ended_at: z.string().nullable(),
  outcome: attemptOutcomeSchema,
  error: taskErrorSchema.nullable(),
});
export type TaskAttempt = z.infer<typeof taskAttemptSchema>;

// §S4 four-variant union. `file.path` must resolve (after realpath) under an
// allowlisted root per §A6 — that is a gateway runtime rule, not a schema rule.
export const taskInputSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("artifact"), artifact_id: z.string() }),
  z.object({
    type: z.literal("file"),
    path: z.string(),
    sha256: z.string(),
    size_bytes: z.number().int(),
  }),
  z.object({
    type: z.literal("text"),
    name: z.string(),
    content: z.string(),
  }),
  z.object({ type: z.literal("url"), url: z.string() }),
]);
export type TaskInput = z.infer<typeof taskInputSchema>;

export const requesterSchema = z.object({
  kind: z.enum(["bot", "user", "cli", "system"]),
  id: z.string(),
  bot_id: z.string().optional(),
});
export type Requester = z.infer<typeof requesterSchema>;

// D16 — every fabric task is a human act: the surface that took the human
// action (send, approval-card confirm) stamps who and which action. The
// gateway rejects tasks without it; allternit-api's forwarder fills it from
// the authenticated user + a single-use human-action id.
export const initiatedBySchema = z.object({
  kind: z.literal("human"),
  user_id: z.string().min(1),
  action_id: z.string().min(1),
});
export type InitiatedBy = z.infer<typeof initiatedBySchema>;

export const taskRoutingSchema = z.object({
  mode: z.enum(["auto", "prefer", "force"]),
  provider: providerIdSchema.optional(),
  account_id: z.string().optional(),
  allow_fallback: z.boolean(), // default true within subscriptions, false across a thread boundary
  allow_metered: z.boolean(), // default false (§32)
  allow_thread_migration: z.boolean(), // default false (Critical #7)
  // Set when the caller pinned `provider` (D16: disclosure is per provider).
  // Routing — first pick and failure re-routes — stays inside that provider.
  provider_pinned: z.boolean().optional(),
});
export type TaskRouting = z.infer<typeof taskRoutingSchema>;

export const taskConstraintsSchema = z.object({
  sensitivity: sensitivitySchema,
  deadline_at: z.string().nullable(),
  max_metered_usd: z.number().nullable(),
  required_export_format: z.string().nullable(),
});
export type TaskConstraints = z.infer<typeof taskConstraintsSchema>;

export const taskResultSchema = z.object({
  artifact_ids: z.array(z.string()),
  text: z.string().optional(),
});
export type TaskResult = z.infer<typeof taskResultSchema>;

export const taskSchema = z.object({
  task_id: z.string(),
  idempotency_key: z.string().nullable(), // caller-supplied; unique per requester
  capability: capabilityIdSchema,
  capability_version: z.number().int(),
  requester: requesterSchema,
  // null only on tasks persisted before D16 enforcement (0005 migration).
  initiated_by: initiatedBySchema.nullable(),
  thread_id: z.string().nullable(),
  project_id: z.string().nullable(),
  parent_task_id: z.string().nullable(), // research.continue, *.edit chains
  prompt: z.string(),
  inputs: z.array(taskInputSchema),
  options: z.record(z.unknown()), // validated against CapabilityDef.input_schema
  routing: taskRoutingSchema,
  constraints: taskConstraintsSchema,
  // required when the capability's side_effects = external_publish (§A6/§9).
  // Presence is an approval-time runtime rule, not a schema `.refine()`.
  approval_id: z.string().nullable(),
  priority: z.enum(["interactive", "normal", "background"]),
  status: taskStatusSchema,
  status_detail: z.string().nullable(), // human-readable ("waiting for login", "provider researching")
  route_decision: routeDecisionSchema.nullable(),
  attempts: z.array(taskAttemptSchema),
  result: taskResultSchema.nullable(),
  error: taskErrorSchema.nullable(),
  created_at: z.string(),
  updated_at: z.string(),
  completed_at: z.string().nullable(),
});
export type Task = z.infer<typeof taskSchema>;
