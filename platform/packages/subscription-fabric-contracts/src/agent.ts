// Allternit Agent Interface (AAI) v0.1 — canonical contracts.
// Wire types are camelCase (unlike the legacy snake_case subsfab schemas).
import { z } from "zod";

export const aaiOperationSchema = z.enum([
  "agent.list",
  "agent.get",
  "agent.capabilities",
  "agent.identity",
  "agent.context.open",
  "agent.context.message",
  "agent.context.steer",
  "agent.context.cancel",
  "agent.context.close",
  "agent.events",
  "agent.tasks",
  "agent.memory",
  "agent.computer",
  "agent.artifacts",
  "agent.approvals",
  "agent.snapshot",
  "agent.sync",
  "agent.health",
]);
export type AAIOperation = z.infer<typeof aaiOperationSchema>;

export const laneSchema = z.enum(["official", "channel", "ui_bridge", "local"]);
export type Lane = z.infer<typeof laneSchema>;

export const guaranteeSchema = z.enum(["exact", "best_effort", "read_only"]);
export type Guarantee = z.infer<typeof guaranteeSchema>;

export const eventGuaranteeSchema = z.enum(["exact", "best_effort", "inferred"]);
export type EventGuarantee = z.infer<typeof eventGuaranteeSchema>;

export const contextIsolationSchema = z.enum(["isolated", "shared", "stateless"]);
export type ContextIsolation = z.infer<typeof contextIsolationSchema>;

export const agentCapabilityManifestSchema = z.object({
  vendor: z.string(),
  adapterId: z.string(),
  lane: laneSchema,
  guarantee: guaranteeSchema,
  context: z.object({
    supported: z.boolean(),
    resume: z.boolean(),
    parallel: z.boolean(),
    maxParallel: z.number().int().nonnegative(),
    isolation: contextIsolationSchema,
  }),
  messaging: z.object({
    send: z.boolean(),
    stream: z.boolean(),
    steer: z.boolean(),
    interrupt: z.boolean(),
    cancel: z.boolean(),
  }),
  memory: z.object({
    read: z.boolean(),
    write: z.boolean(),
    snapshot: z.boolean(),
    opaque: z.boolean(),
  }),
  tools: z.object({
    tools: z.boolean(),
    mcp: z.boolean(),
    plugins: z.boolean(),
    connectors: z.boolean(),
  }),
  tasks: z.object({
    list: z.boolean(),
    schedule: z.boolean(),
    cancel: z.boolean(),
    background: z.boolean(),
  }),
  approvals: z.object({
    read: z.boolean(),
    respond: z.boolean(),
    exact: z.boolean(),
  }),
  computer: z.object({
    view: z.boolean(),
    control: z.boolean(),
    takeover: z.boolean(),
  }),
  artifacts: z.object({
    read: z.boolean(),
    write: z.boolean(),
    export: z.boolean(),
  }),
  events: z.object({
    native: z.boolean(),
    polling: z.boolean(),
    transcriptDerived: z.boolean(),
    replay: z.boolean(),
  }),
  runtime: z.object({
    alwaysOn: z.boolean(),
    localRequired: z.boolean(),
    cloud: z.boolean(),
  }),
});
export type AgentCapabilityManifest = z.infer<typeof agentCapabilityManifestSchema>;

export const aaiErrorCodeSchema = z.enum([
  "UNSUPPORTED",
  "AUTH_REQUIRED",
  "AUTH_REVOKED",
  "RATE_LIMITED",
  "VENDOR_UNAVAILABLE",
  "LANE_BLOCKED",
  "CONTEXT_NOT_FOUND",
  "CONTEXT_BUSY",
  "APPROVAL_REQUIRED",
  "POLICY_DENIED",
  "ADAPTER_DRIFT",
  "SYNC_CONFLICT",
  "UNKNOWN",
]);
export type AAIErrorCode = z.infer<typeof aaiErrorCodeSchema>;

export const aaiErrorSchema = z.object({
  code: aaiErrorCodeSchema,
  retryable: z.boolean(),
  retryAfterMs: z.number().int().nonnegative().optional(),
  vendorCode: z.string().optional(),
  humanMessage: z.string(),
  details: z.record(z.unknown()).optional(),
});
export type AAIError = z.infer<typeof aaiErrorSchema>;

export const gatewayEventTypeSchema = z.enum([
  "agent.context.opened",
  "agent.activity.started",
  "agent.message.delta",
  "agent.message.completed",
  "agent.tool.called",
  "agent.approval.requested",
  "agent.approval.resolved",
  "agent.artifact.created",
  "agent.computer.frame",
  "agent.task.updated",
  "agent.health.changed",
  "channel.message.received",
  "channel.message.sent",
  "channel.reaction.updated",
  "channel.message.edited",
  "channel.message.deleted",
]);
export type GatewayEventType = z.infer<typeof gatewayEventTypeSchema>;

export const eventSourceSchema = z.enum(["allternit", "vendor"]);
export type EventSource = z.infer<typeof eventSourceSchema>;

export const gatewayEventSchema = z.object({
  type: gatewayEventTypeSchema,
  botId: z.string(),
  threadId: z.string(),
  generationId: z.string(),
  source: eventSourceSchema,
  vendor: z.string().optional(),
  adapter: z.string().optional(),
  lane: laneSchema.optional(),
  remoteEventId: z.string().optional(),
  remoteContextId: z.string().optional(),
  causationId: z.string(),
  correlationId: z.string(),
  guarantee: eventGuaranteeSchema,
  at: z.string().optional(),
  payload: z.record(z.unknown()).optional(),
});
export type GatewayEvent = z.infer<typeof gatewayEventSchema>;

// Two authorities: an Allternit approval never auto-approves a vendor one.
export const approvalAuthoritySchema = z.enum(["allternit", "vendor"]);
export type ApprovalAuthority = z.infer<typeof approvalAuthoritySchema>;

export const approvalStateSchema = z.enum(["pending", "approved", "denied", "expired", "cancelled"]);
export type ApprovalState = z.infer<typeof approvalStateSchema>;

export const approvalSchema = z.object({
  authority: approvalAuthoritySchema,
  actor: z.string(),
  action: z.string(),
  threadId: z.string(),
  remoteRef: z.string().optional(),
  state: approvalStateSchema,
});
export type Approval = z.infer<typeof approvalSchema>;

export const threadOriginSchema = z.object({
  type: z.enum(["allternit", "channel", "gateway", "mention", "routine"]),
  provider: z.string().optional(),
  externalConversationId: z.string().optional(),
  externalMessageId: z.string().optional(),
});
export type ThreadOrigin = z.infer<typeof threadOriginSchema>;

export const memoryRecordSchema = z.object({
  // Record id and body for a readable vendor memory (allternit-api's vendor-memory view needs
  // both to list and promote a record; `remoteRef` is used as the id when `id` is absent).
  id: z.string().optional(),
  text: z.string().optional(),
  scope: z.string(),
  source: z.enum(["native", "vendor"]),
  vendor: z.string().optional(),
  remoteRef: z.string().optional(),
  authority: z.enum(["allternit", "vendor", "shared"]),
  confidence: z.number().min(0).max(1).optional(),
  promotable: z.boolean(),
});
export type MemoryRecord = z.infer<typeof memoryRecordSchema>;

// Field-level Mirror sync state (agent.sync). Spec "Mirror rules": the vendor is the authority for
// mirrored fields; a field the adapter cannot read is `unobservable`, never claimed `synced`.
// `partial` = in sync on the part the adapter can observe; `stale` = one side moved (see `direction`).
export const mirrorFieldStateSchema = z.object({
  field: z.string(),
  authority: z.literal("vendor").default("vendor"),
  observability: z.enum(["exact", "partial", "none"]),
  status: z.enum(["synced", "partial", "stale", "conflict", "unobservable"]),
  /** Which side moved, for `stale`. */
  direction: z.enum(["remote_ahead", "local_ahead"]).optional(),
  remoteVersion: z.string().optional(),
  localVersion: z.string().optional(),
  checkedAt: z.string().optional(),
});
export type MirrorFieldState = z.infer<typeof mirrorFieldStateSchema>;
