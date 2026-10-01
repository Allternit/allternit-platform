// §A1 — Adapter interface, event stream, and the SDK-owned boundary contracts.
// Data types are zod schemas (round-trip testable); the adapter/runtime
// interfaces are TS-only, because the SDK implements them later.
import { z } from "zod";
import type { ReplyEvent } from "@allternit/replies-contract";
import type { AccountUsage } from "./account";
import type { AdapterManifest } from "./manifest";
import type { QuotaSignal } from "./quota";
import type { ThreadSnapshot } from "./thread";
import type { ArtifactFile, ProviderArtifactRef } from "./artifact";
import type { Task, TaskAttempt, TaskError } from "./task";
import { artifactSchema, providerArtifactRefSchema } from "./artifact";
import { quotaSignalSchema } from "./quota";
import { taskErrorSchema } from "./task";

// ---------------------------------------------------------------------------
// Small data contracts
// ---------------------------------------------------------------------------

export const probeCheckSchema = z.object({
  key: z.string(), // named locator key from the selector registry (§A3)
  critical: z.boolean(),
  ok: z.boolean(),
  detail: z.string().optional(),
});
export type ProbeCheck = z.infer<typeof probeCheckSchema>;

// Non-spending canary result: auth state + every critical locator.
export const probeResultSchema = z.object({
  ok: z.boolean(),
  checks: z.array(probeCheckSchema),
  observed_at: z.string(),
});
export type ProbeResult = z.infer<typeof probeResultSchema>;

// Opaque resume handle for a detached task (Critical #3). The worker schedules
// `resume(token)` on a backoff; nothing else interprets `token`.
export const resumeTokenSchema = z.object({
  token: z.string(),
  adapter_id: z.string(),
  attempt_no: z.number().int(),
  issued_at: z.string(),
  poll_after_s: z.number(),
});
export type ResumeToken = z.infer<typeof resumeTokenSchema>;

// §A1 `reconcile()` outcome — resolving a `sent_unconfirmed` attempt (Critical
// #2). Adapters never resubmit automatically.
export const reconcileOutcomeSchema = z.enum([
  "acknowledged",
  "duplicate",
  "not_found",
  "ambiguous",
]);
export type ReconcileOutcome = z.infer<typeof reconcileOutcomeSchema>;

export const reconcileResultSchema = z.object({
  outcome: reconcileOutcomeSchema,
  provider_thread_id: z.string().optional(),
  detail: z.string().optional(),
});
export type ReconcileResult = z.infer<typeof reconcileResultSchema>;

// `reply` events carry the canonical `ReplyEvent` from
// `@allternit/replies-contract` verbatim, so Thread UIs reuse
// `@allternit/replies-reducer` instead of adopting a third event dialect.
const replyEventSchema: z.ZodType<ReplyEvent> = z.custom<ReplyEvent>(
  (v) =>
    typeof v === "object" &&
    v !== null &&
    typeof (v as { type?: unknown }).type === "string",
  { message: "expected a @allternit/replies-contract ReplyEvent" }
);

// ---------------------------------------------------------------------------
// AdapterEvent — the 11-variant streaming union (§A1), verbatim.
// ---------------------------------------------------------------------------

export const adapterEventSchema = z.discriminatedUnion("t", [
  z.object({
    t: z.literal("submitted"),
    provider_thread_id: z.string().nullable(),
    provider_url: z.string().nullable(),
  }),
  z.object({
    t: z.literal("reply"),
    event: replyEventSchema, // reuse @allternit/replies-contract
  }),
  z.object({
    t: z.literal("progress"),
    label: z.string(),
    fraction: z.number().optional(),
  }),
  // D11 — gateway/worker heartbeat: the UI must never render silence.
  z.object({
    t: z.literal("progress.heartbeat"),
    elapsed_s: z.number(),
    last_change_at: z.string(),
  }),
  z.object({
    t: z.literal("artifact.partial"),
    ref: providerArtifactRefSchema,
    preview: z.instanceof(Uint8Array).optional(),
  }),
  z.object({
    t: z.literal("artifact.ready"),
    ref: providerArtifactRefSchema,
    meta: artifactSchema.partial(),
  }),
  z.object({ t: z.literal("model.observed"), model: z.string() }),
  z.object({
    t: z.literal("quota.signal"),
    pool_id: z.string(),
    signal: quotaSignalSchema,
  }),
  z.object({
    t: z.literal("needs_user"),
    reason: z.enum(["auth", "challenge", "confirm_dialog"]),
    message: z.string(),
  }),
  z.object({
    t: z.literal("detached"),
    resume_token: resumeTokenSchema,
    poll_after_s: z.number(),
  }),
  z.object({
    t: z.literal("done"),
    outcome: z.enum(["success", "partial"]),
    text: z.string().optional(),
  }),
  z.object({ t: z.literal("error"), error: taskErrorSchema }),
]);
export type AdapterEvent = z.infer<typeof adapterEventSchema>;

// ---------------------------------------------------------------------------
// SDK-owned collaborators — boundary contracts implemented by the adapter SDK.
// They are declared here (opaque/minimal) so the adapter interface can be typed
// without depending on the SDK package.
// ---------------------------------------------------------------------------

// implemented by @allternit/subscription-adapter-sdk (P2)
export interface PageLease {
  readonly lease_id: string;
  url(): string;
  release(): Promise<void>;
}

// implemented by @allternit/subscription-adapter-sdk (P2)
export interface ArtifactSink {
  begin(
    ref: ProviderArtifactRef,
    meta?: { mime_type?: string; format?: string; title?: string }
  ): Promise<string>; // returns a new artifact_id
  write(artifact_id: string, chunk: Uint8Array): Promise<void>;
  commit(artifact_id: string, file: ArtifactFile): Promise<void>;
  fail(artifact_id: string, reason: string): Promise<void>;
}

// implemented by @allternit/subscription-adapter-sdk (P2)
export interface Pacer {
  beforeAction(): Promise<void>; // A5 min_action_gap_ms
  beforeTask(): Promise<void>; // A5 min_task_gap_s / hourly + daily caps
}

// implemented by @allternit/subscription-adapter-sdk (P2)
export interface SelectorResolver {
  // Opaque, session-scoped locator for a named locator key; the SDK owns the
  // concrete Playwright locator type.
  resolve(locator_key: string): Promise<unknown>;
  // Which ordered strategy matched last (early-drift telemetry, §A3).
  lastMatchedStrategy(locator_key: string): string | null;
}

// implemented by @allternit/subscription-adapter-sdk (P2)
export interface RedactingLogger {
  debug(message: string, fields?: Record<string, unknown>): void;
  info(message: string, fields?: Record<string, unknown>): void;
  warn(message: string, fields?: Record<string, unknown>): void;
  error(message: string, fields?: Record<string, unknown>): void;
}

// implemented by @allternit/subscription-adapter-sdk (P2)
export interface AdapterRuntime {
  readonly adapter_id: string;
  readonly origins: string[]; // navigation allowlist (§A6)
  navigate(url: string): Promise<void>;
}

// ---------------------------------------------------------------------------
// §A1 — ExecutionContext and SubscriptionAdapter
// ---------------------------------------------------------------------------

export interface ExecutionContext {
  signal: AbortSignal; // cancellation; abort() → adapter clicks provider Stop
  page: PageLease; // scoped, worker-owned; adapters must not keep references
  artifacts: ArtifactSink; // streams bytes into the content-addressed store
  pacing: Pacer; // A5 — adapters call pacing.beforeAction()
  selectors: SelectorResolver; // A3
  log: RedactingLogger;
  attempt: TaskAttempt; // adapter updates submission_state via markSubmitted()
  // Durable write BEFORE continuing: `sent_unconfirmed` before Send is
  // clicked, `acknowledged` after the provider acknowledges.
  markSubmitted(provider_thread_id: string | null): Promise<void>;
}

export interface AccountObservation {
  identity: string | null;
  usage: AccountUsage | null;
  /** The account's own agents on the provider (Claude Projects…); undefined = not read. */
  agents?: { id: string; name: string; kind?: string }[];
}

export interface SubscriptionAdapter {
  readonly manifest: AdapterManifest;

  // Lifecycle — the worker injects a browser context; adapters never launch.
  attach(ctx: AdapterRuntime): Promise<void>;
  detach(): Promise<void>;

  // Health — non-spending.
  probe(signal: AbortSignal): Promise<ProbeResult>; // canary: auth + critical locators
  readPlan?(signal: AbortSignal): Promise<string | null>; // observed plan tier
  // Who is signed in and what the page shows about usage — non-spending,
  // read after a ready probe. Never returns tokens or cookies.
  readAccount?(signal: AbortSignal): Promise<AccountObservation>;
  readQuota?(pool_id: string, signal: AbortSignal): Promise<QuotaSignal | null>;

  // Execution — streaming, cancellable.
  execute(task: Task, ctx: ExecutionContext): AsyncIterable<AdapterEvent>;
  resume?(
    token: ResumeToken,
    ctx: ExecutionContext
  ): AsyncIterable<AdapterEvent>; // detached tasks
  reconcile?(
    attempt: TaskAttempt,
    ctx: ExecutionContext
  ): Promise<ReconcileResult>; // Critical #2

  // Threads/artifacts
  readThread?(
    provider_thread_id: string,
    ctx: ExecutionContext
  ): Promise<ThreadSnapshot>; // divergence check
  fetchArtifact?(
    ref: ProviderArtifactRef,
    ctx: ExecutionContext
  ): Promise<ArtifactFile>;
  exportArtifact?(
    ref: ProviderArtifactRef,
    format: string,
    ctx: ExecutionContext
  ): Promise<ArtifactFile>;
}
