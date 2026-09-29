// @allternit/subscription-fabric-contracts — public surface.
// Named re-exports grouped by module (no `export *` so intent stays explicit).

// §S1 — capability taxonomy + shared primitives
export {
  artifactTypeSchema,
  capabilityDefSchema,
  capabilityIdSchema,
  jsonSchemaSchema,
  modelClassSchema,
  providerIdSchema,
  sensitivitySchema,
  sideEffectsSchema,
} from "./capability";
export type {
  ArtifactType,
  CapabilityDef,
  CapabilityId,
  JSONSchema,
  ModelClass,
  ProviderId,
  Sensitivity,
  SideEffects,
} from "./capability";

// §S2 — adapter manifest + pacing (§A5)
export {
  adapterManifestSchema,
  manifestCapabilitySchema,
  pacingProfileSchema,
  planDefSchema,
} from "./manifest";
export type {
  AdapterManifest,
  ManifestCapability,
  PacingProfile,
  PlanDef,
} from "./manifest";

// §S3 — accounts and session health
export { accountSchema, sessionHealthSchema } from "./account";
export type { Account, SessionHealth } from "./account";

// §S3 — quota pools, signals, entitlements
export { entitlementSchema, quotaPoolSchema, quotaSignalSchema } from "./quota";
export type { Entitlement, QuotaPool, QuotaSignal } from "./quota";

// §S5 — artifacts
export {
  artifactFileSchema,
  artifactSchema,
  providerArtifactRefSchema,
} from "./artifact";
export type { Artifact, ArtifactFile, ProviderArtifactRef } from "./artifact";

// §S6 — thread mappings
export { threadMappingSchema, threadSnapshotSchema } from "./thread";
export type { ThreadMapping, ThreadSnapshot } from "./thread";

// §S4 + §S7 — tasks, attempts, inputs, errors
export {
  attemptOutcomeSchema,
  failureClassSchema,
  initiatedBySchema,
  requesterSchema,
  submissionStateSchema,
  taskAttemptSchema,
  taskConstraintsSchema,
  taskErrorSchema,
  taskInputSchema,
  taskResultSchema,
  taskRoutingSchema,
  taskSchema,
  taskStatusSchema,
} from "./task";
export type {
  AttemptOutcome,
  FailureClass,
  InitiatedBy,
  Requester,
  SubmissionState,
  Task,
  TaskAttempt,
  TaskConstraints,
  TaskError,
  TaskInput,
  TaskResult,
  TaskRouting,
  TaskStatus,
} from "./task";

// §A2 — pure router + snapshot
export {
  fabricSnapshotSchema,
  rejectReasonSchema,
  rejectedRouteSchema,
  routeCandidateSchema,
  routeDecisionSchema,
  routeLaneSchema,
} from "./routing";
export type {
  CapabilityRouter,
  FabricSnapshot,
  RejectReason,
  RejectedRoute,
  RouteCandidate,
  RouteDecision,
  RouteLane,
} from "./routing";

// §A1 — adapter event stream + runtime boundary contracts
export {
  adapterEventSchema,
  probeCheckSchema,
  probeResultSchema,
  reconcileOutcomeSchema,
  reconcileResultSchema,
  resumeTokenSchema,
} from "./events";
export type {
  AdapterEvent,
  AdapterRuntime,
  ArtifactSink,
  ExecutionContext,
  Pacer,
  PageLease,
  ProbeCheck,
  ProbeResult,
  ReconcileOutcome,
  ReconcileResult,
  RedactingLogger,
  ResumeToken,
  SelectorResolver,
  SubscriptionAdapter,
} from "./events";
