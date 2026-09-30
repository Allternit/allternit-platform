// Agent Gateway bindings: wire types + state machines.
// The transition tables below are mirrored by the Rust API (WP2); keep in sync.
import { z } from "zod";
import { laneSchema } from "./agent";

function machine<S extends string>(table: Record<S, readonly S[]>) {
  return (from: S, to: S): boolean => (table[from] ?? []).includes(to);
}

// ---- BotExecutionBinding ----
export const botExecutionStateSchema = z.enum([
  "UNBOUND", "BOUND", "READY", "DEGRADED", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED",
]);
export type BotExecutionState = z.infer<typeof botExecutionStateSchema>;

export const EXECUTION_TRANSITIONS: Record<BotExecutionState, readonly BotExecutionState[]> = {
  UNBOUND: ["BOUND"],
  BOUND: ["READY", "NEEDS_AUTH", "FAILED", "UNBOUND", "DISABLED"],
  READY: ["DEGRADED", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED", "BOUND"],
  DEGRADED: ["READY", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED"],
  NEEDS_AUTH: ["BOUND", "READY", "DISABLED", "FAILED"],
  PAUSED: ["READY", "DISABLED", "BOUND"],
  DISABLED: ["BOUND", "UNBOUND"],
  // terminal until repaired or rebound
  FAILED: ["BOUND", "UNBOUND"],
};
export const canTransitionExecution = machine(EXECUTION_TRANSITIONS);

export const botExecutionBindingSchema = z.object({
  id: z.string(),
  botId: z.string(),
  type: z.enum(["allternit", "vendor"]),
  mode: z.enum(["native", "hosted", "linked", "mirror"]),
  vendor: z.string().optional(),
  adapterId: z.string().optional(),
  accountBindingId: z.string().optional(),
  preferredLane: laneSchema.optional(),
  externalAgentId: z.string().optional(),
  capabilities: z.record(z.unknown()).optional(),
  health: z.record(z.unknown()).optional(),
  state: botExecutionStateSchema,
});
export type BotExecutionBinding = z.infer<typeof botExecutionBindingSchema>;

// ---- ProviderAccountBinding ----
export const authTypeSchema = z.enum([
  "oauth", "browser_session", "api_key", "desktop_session",
  "local_endpoint", "channel_oauth", "mcp_plugin",
]);
export type AuthType = z.infer<typeof authTypeSchema>;

export const connectionStateSchema = z.enum([
  "DISCONNECTED", "CONSENT_REQUIRED", "AUTHENTICATING", "VERIFYING",
  "CONNECTED", "DEGRADED", "AUTH_FAILED", "EXPIRED", "REVOKED", "BLOCKED",
]);
export type ConnectionState = z.infer<typeof connectionStateSchema>;

export const CONNECTION_TRANSITIONS: Record<ConnectionState, readonly ConnectionState[]> = {
  DISCONNECTED: ["CONSENT_REQUIRED"],
  CONSENT_REQUIRED: ["AUTHENTICATING", "DISCONNECTED"],
  AUTHENTICATING: ["VERIFYING", "AUTH_FAILED", "DISCONNECTED"],
  VERIFYING: ["CONNECTED", "DEGRADED", "AUTH_FAILED", "DISCONNECTED"],
  CONNECTED: ["EXPIRED", "REVOKED", "BLOCKED", "DEGRADED", "DISCONNECTED"],
  DEGRADED: ["CONNECTED", "EXPIRED", "REVOKED", "BLOCKED", "DISCONNECTED"],
  AUTH_FAILED: ["CONSENT_REQUIRED", "AUTHENTICATING", "DISCONNECTED"],
  EXPIRED: ["AUTHENTICATING", "DISCONNECTED"],
  REVOKED: ["CONSENT_REQUIRED", "DISCONNECTED"],
  BLOCKED: ["AUTHENTICATING", "DISCONNECTED"],
};
export const canTransitionConnection = machine(CONNECTION_TRANSITIONS);

export const providerAccountBindingSchema = z.object({
  id: z.string(),
  owner: z.string(),
  vendor: z.string(),
  authType: authTypeSchema,
  externalAccountId: z.string().optional(),
  displayName: z.string().optional(),
  workspace: z.string().optional(),
  // References only: raw credentials never appear on a binding.
  secretRef: z.string().optional(),
  sessionRef: z.string().optional(),
  scopes: z.array(z.string()),
  state: connectionStateSchema,
  verifiedAt: z.string().optional(),
  expiresAt: z.string().optional(),
});
export type ProviderAccountBinding = z.infer<typeof providerAccountBindingSchema>;

// ---- RemoteThreadBinding ----
export const remoteThreadStateSchema = z.enum([
  "UNBOUND", "OPENING", "ACTIVE", "HANDOFF_PENDING", "CLOSED",
]);
export type RemoteThreadState = z.infer<typeof remoteThreadStateSchema>;

export const REMOTE_THREAD_TRANSITIONS: Record<RemoteThreadState, readonly RemoteThreadState[]> = {
  UNBOUND: ["OPENING"],
  OPENING: ["ACTIVE", "CLOSED"],
  ACTIVE: ["HANDOFF_PENDING", "CLOSED"],
  HANDOFF_PENDING: ["ACTIVE", "CLOSED"],
  CLOSED: [],
};
export const canTransitionRemoteThread = machine(REMOTE_THREAD_TRANSITIONS);

export const remoteThreadBindingSchema = z.object({
  id: z.string(),
  threadId: z.string(),
  generation: z.number().int().nonnegative(),
  executionBindingId: z.string(),
  externalContextId: z.string().optional(),
  externalTaskId: z.string().optional(),
  continuationToken: z.string().optional(),
  syncCursor: z.string().optional(),
  lastRemoteEventId: z.string().optional(),
  capabilitySnapshot: z.record(z.unknown()).optional(),
  lane: laneSchema,
  state: remoteThreadStateSchema,
});
export type RemoteThreadBinding = z.infer<typeof remoteThreadBindingSchema>;

// ---- ChannelConversationBinding ----
export const channelSyncStateSchema = z.enum(["LIVE", "DELAYED", "RECONNECTING", "DEGRADED", "DISCONNECTED"]);
export type ChannelSyncState = z.infer<typeof channelSyncStateSchema>;

export const channelConversationBindingSchema = z.object({
  threadId: z.string(),
  provider: z.string(),
  accountBindingId: z.string(),
  externalWorkspaceId: z.string().optional(),
  externalChannelId: z.string().optional(),
  externalConversationId: z.string(),
  externalThreadId: z.string().optional(),
  canonicalUrl: z.string().optional(),
  bidirectional: z.boolean(),
  readOnly: z.boolean(),
  postingIdentityId: z.string().optional(),
  lastInboundCursor: z.string().optional(),
  lastOutboundCursor: z.string().optional(),
  syncState: channelSyncStateSchema,
});
export type ChannelConversationBinding = z.infer<typeof channelConversationBindingSchema>;
