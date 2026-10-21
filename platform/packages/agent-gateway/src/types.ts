// AAI v0.1 runtime types. Wire contracts (manifest, errors, events, approvals)
// come from @allternit/subscription-fabric-contracts and are not redefined here.
import type {
  AAIError,
  AAIErrorCode,
  AAIOperation,
  AgentCapabilityManifest,
  Approval,
  ContextIsolation,
  GatewayEvent,
  Guarantee,
  MemoryRecord,
  MirrorFieldState,
} from "@allternit/subscription-fabric-contracts";

export type { AAIError, AAIErrorCode, AAIOperation, AgentCapabilityManifest, Approval, GatewayEvent, MemoryRecord, MirrorFieldState };

export type AaiResult<T> = { ok: true; value: T } | { ok: false; error: AAIError };

export const ok = <T>(value: T): AaiResult<T> => ({ ok: true, value });

const RETRYABLE: ReadonlySet<AAIErrorCode> = new Set(["RATE_LIMITED", "VENDOR_UNAVAILABLE"]);

export function fail(
  code: AAIErrorCode,
  humanMessage: string,
  extra: Partial<Omit<AAIError, "code" | "humanMessage">> = {},
): AaiResult<never> {
  return { ok: false, error: { code, retryable: RETRYABLE.has(code), humanMessage, ...extra } };
}

export const unsupported = (op: AAIOperation | string): AaiResult<never> =>
  fail("UNSUPPORTED", `${op} is not supported by this provider`, { retryable: false });

// ---- identity ----
export interface AgentSummary {
  agentId: string; displayName: string; vendor: string; state: string;
  /** The agent's own avatar as the vendor shows it: an https URL or a png/jpeg/webp/gif data URI (never svg). */
  avatarUrl?: string;
  /** Vendor account entry category and human-readable label. */
  kind?: string; kindLabel?: string;
}
export interface AgentDetail extends AgentSummary { remoteIds: Record<string, string>; capabilities: AgentCapabilityManifest }
export interface AgentIdentity { agentId: string; displayName: string; vendor: string; lookPack?: string | null }

// ---- execution ----
export interface OpenContextInput {
  agentId: string;
  /** Allternit Thread this context serves (adopted if it already exists remotely). */
  threadId?: string;
  title?: string;
  /** Resume/adopt an existing remote context. Only valid when manifest.context.resume. */
  adoptContextId?: string;
}
export interface OpenContextResult { contextId: string; isolation: ContextIsolation; guarantee: Guarantee; resumed: boolean }
export interface MessageInput { contextId: string; correlationId: string; text: string }
export interface MessageResult { messageId: string; correlationId: string; reply?: string; guarantee: Guarantee }
export interface SteerInput { contextId: string; text: string }
export interface CancelResult { confirmed: boolean }
export interface CursoredEvent { cursor: string; event: GatewayEvent }
export interface EventsInput { contextId: string; cursor?: string; limit?: number }
export interface EventsResult { events: CursoredEvent[]; nextCursor: string }

// ---- resources / governance ----
export interface TaskInfo { taskId: string; title: string; state: string }
export type MemoryInput =
  | { contextId?: string; op: "read"; key: string }
  | { contextId?: string; op: "write"; key: string; value: string }
  | { contextId?: string; op: "snapshot" };
export interface MemoryResult { value?: string; records?: MemoryRecord[] }
export type ComputerInput = { contextId: string; op: "frame" | "control" | "takeover"; args?: Record<string, unknown> };
export interface ComputerResult { frame?: string; state?: string }
export interface ArtifactInfo { artifactId: string; name: string }
export type ApprovalsInput =
  | { contextId?: string; op: "list" }
  | { contextId?: string; op: "respond"; approvalId: string; decision: "approve" | "deny"; actor: { type: "human" | "system"; id: string } };
export interface ApprovalsResult { approvals?: Approval[]; resolved?: Approval }
export interface SnapshotResult { agentId: string; fields: Record<string, unknown> }
export interface SyncResult { fields: MirrorFieldState[] }
export interface HealthResult { status: "healthy" | "degraded" | "down"; lane?: string; detail?: string }

/** One method per AAI v0.1 operation. Implementations return AaiResult, never throw. */
export interface AaiProvider {
  readonly adapterId: string;
  list(): Promise<AaiResult<AgentSummary[]>>;
  get(agentId: string): Promise<AaiResult<AgentDetail>>;
  capabilities(agentId: string): Promise<AaiResult<AgentCapabilityManifest>>;
  identity(agentId: string): Promise<AaiResult<AgentIdentity>>;
  contextOpen(input: OpenContextInput): Promise<AaiResult<OpenContextResult>>;
  contextMessage(input: MessageInput): Promise<AaiResult<MessageResult>>;
  contextSteer(input: SteerInput): Promise<AaiResult<{ accepted: boolean }>>;
  contextCancel(input: { contextId: string }): Promise<AaiResult<CancelResult>>;
  contextClose(input: { contextId: string }): Promise<AaiResult<{ closed: boolean }>>;
  events(input: EventsInput): Promise<AaiResult<EventsResult>>;
  tasks(input: { agentId: string }): Promise<AaiResult<TaskInfo[]>>;
  memory(input: MemoryInput): Promise<AaiResult<MemoryResult>>;
  computer(input: ComputerInput): Promise<AaiResult<ComputerResult>>;
  artifacts(input: { agentId: string; contextId?: string }): Promise<AaiResult<ArtifactInfo[]>>;
  approvals(input: ApprovalsInput): Promise<AaiResult<ApprovalsResult>>;
  snapshot(input: { agentId: string }): Promise<AaiResult<SnapshotResult>>;
  sync(input: { agentId: string }): Promise<AaiResult<SyncResult>>;
  health(input: { agentId?: string }): Promise<AaiResult<HealthResult>>;
}
