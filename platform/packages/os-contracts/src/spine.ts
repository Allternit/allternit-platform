/**
 * AllternitOS contract spine — DRAFT.
 *
 * These are the minimal `Workload`/`Artifact`/`Capability`/`Lease`/`Receipt`/
 * `Approval`/`Event` contracts the Documents and Office program package is
 * built against. They are PROVISIONAL: the platform's contract-spine ADRs
 * (ADR-002..012) are not ratified (see docs/learnings/GENOFFICE_PHASE5_DECISION.md and
 * docs/Core_System/00-Strategy/ALLTERNIT_OS_LIVING_ROADMAP.md), so shapes
 * here may change when the ratified schemas land. Everything in this package
 * is versioned `0.1.0-draft` and must not be treated as a stable platform
 * contract until ratification.
 */

import { z } from 'zod'

/** Contract spine version — bumped on any shape change. */
export const CONTRACT_SPINE_VERSION = '0.1.0-draft.0'

/** Lifecycle marker: everything here is provisional until ADR ratification. */
export const CONTRACT_SPINE_STATUS = 'DRAFT-pending-ADR-ratification' as const

const isoDateTime = z.string().datetime()
const id = z.string().min(1)

// ── Event ────────────────────────────────────────────────────────────────────

export const eventSchema = z.object({
  id,
  /** Dot-namespaced event type, e.g. `artifact.created`. */
  type: z.string().min(1),
  at: isoDateTime,
  /** Emitting program or service id. */
  source: z.string().min(1),
  /** Subject resource id (artifact, workload, …) when applicable. */
  subject: z.string().optional(),
  data: z.record(z.string(), z.unknown()).optional(),
})
export type SpineEvent = z.infer<typeof eventSchema>

// ── Capability ───────────────────────────────────────────────────────────────

export const capabilityKindSchema = z.enum(['read', 'write', 'execute', 'compute', 'observe', 'stream'])
export const capabilitySchema = z.object({
  id,
  /** Dot-namespaced capability name, e.g. `office.docx.edit`. */
  name: z.string().min(1),
  version: z.string().min(1),
  kind: capabilityKindSchema,
  /** Resource scope the capability governs, e.g. `artifact:office-document`. */
  resource: z.string().min(1),
  description: z.string().optional(),
})
export type Capability = z.infer<typeof capabilitySchema>

// ── Fabric node identity ─────────────────────────────────────────────────────

/** Transport families a Fabric endpoint may use. */
export const fabricTransportSchema = z.enum(['tailscale', 'tunnel', 'mdns', 'local', 'relay'])
export type FabricTransport = z.infer<typeof fabricTransportSchema>

export const nodeEndpointSchema = z.object({
  transport: fabricTransportSchema,
  url: z.string().url(),
  /** Lower is preferred. Direct local/mesh endpoints win over relay. */
  priority: z.number().int().default(0),
  /** Transport-specific metadata (e.g. tailnet IP, tunnel hostname). */
  metadata: z.record(z.string(), z.unknown()).optional(),
})
export type NodeEndpoint = z.infer<typeof nodeEndpointSchema>

export const nodeResourceSchema = z.object({
  kind: z.string().min(1),
  name: z.string().min(1),
  value: z.union([z.string(), z.number(), z.boolean()]).optional(),
  unit: z.string().optional(),
})
export type NodeResource = z.infer<typeof nodeResourceSchema>

export const nodeIdentitySchema = z.object({
  nodeId: id,
  name: z.string().min(1),
  /** Runtime type: desktop, vps, worker, phone, etc. */
  runtimeType: z.string().min(1),
  /** Platform identifier, e.g. darwin-arm64. */
  platform: z.string().min(1),
  version: z.string().min(1),
  endpoints: z.array(nodeEndpointSchema),
  capabilities: z.array(capabilitySchema),
  resources: z.array(nodeResourceSchema).optional(),
})
export type NodeIdentity = z.infer<typeof nodeIdentitySchema>

export const capabilityQuerySchema = z.object({
  name: z.string().optional(),
  kind: capabilityKindSchema.optional(),
  resource: z.string().optional(),
  nodeId: z.string().optional(),
})
export type CapabilityQuery = z.infer<typeof capabilityQuerySchema>

export const fabricEventSchema = z.object({
  id,
  /** Dot-namespaced event type, e.g. `fabric.node.joined`. */
  type: z.string().min(1),
  at: isoDateTime,
  source: z.string().min(1),
  subject: z.string().optional(),
  data: z.record(z.string(), z.unknown()).optional(),
})
export type FabricEvent = z.infer<typeof fabricEventSchema>

// ── Lease ────────────────────────────────────────────────────────────────────

export const leasePolicySchema = z.object({
  workloadId: id.optional(),
  principalId: z.string().optional(),
  budgetId: z.string().optional(),
  maxInvocations: z.number().int().min(1).optional(),
  extra: z.record(z.string(), z.unknown()).optional(),
})
export type LeasePolicy = z.infer<typeof leasePolicySchema>

export const leaseSchema = z.object({
  id,
  capabilityId: id,
  /** Program the capability is granted to. */
  grantee: z.string().min(1),
  issuedAt: isoDateTime,
  expiresAt: isoDateTime.optional(),
  status: z.enum(['active', 'revoked', 'expired']),
  constraints: z.record(z.string(), z.unknown()).optional(),
  policy: leasePolicySchema.optional(),
  /** Signature proving the lease was issued by the authority (format TBD). */
  signature: z.string().optional(),
})
export type Lease = z.infer<typeof leaseSchema>

// ── Workload ─────────────────────────────────────────────────────────────────

export const workloadSchema = z.object({
  id,
  programId: z.string().min(1),
  /** Dot-namespaced workload kind, e.g. `office.document.parse`. */
  kind: z.string().min(1),
  status: z.enum(['queued', 'running', 'succeeded', 'failed', 'cancelled']),
  /** Leases this workload executes under. */
  leaseIds: z.array(id).default([]),
  inputs: z.record(z.string(), z.unknown()).optional(),
  createdAt: isoDateTime,
  updatedAt: isoDateTime,
})
export type Workload = z.infer<typeof workloadSchema>

// ── Artifact ─────────────────────────────────────────────────────────────────

export const artifactProvenanceSchema = z.object({
  workloadId: id.optional(),
  programId: z.string().optional(),
  /** Free-form lineage note (e.g. engine name + version). */
  engine: z.string().optional(),
})

export const artifactSchema = z.object({
  id,
  workspaceId: z.string().min(1),
  type: z.string().min(1),
  title: z.string(),
  status: z.enum(['draft', 'published', 'archived']),
  /** Monotonic content version; 1-based. */
  version: z.number().int().positive(),
  /** Content fingerprint (format recorded in `checksumAlgo`). */
  checksum: z.string().optional(),
  checksumAlgo: z.enum(['fnv1a64', 'sha256']).optional(),
  provenance: artifactProvenanceSchema.optional(),
  createdAt: isoDateTime,
  updatedAt: isoDateTime,
})
export type Artifact = z.infer<typeof artifactSchema>

// ── Receipt ──────────────────────────────────────────────────────────────────

export const receiptSchema = z.object({
  id,
  workloadId: id,
  status: z.enum(['succeeded', 'failed', 'cancelled']),
  /** Artifacts produced or mutated by the workload. */
  artifactIds: z.array(id).default([]),
  /** Short human/agent-readable summary of what happened. */
  summary: z.string().optional(),
  issuedAt: isoDateTime,
})
export type Receipt = z.infer<typeof receiptSchema>

// ── Fabric invocation receipt ────────────────────────────────────────────────

export const fabricInvocationReceiptSchema = z.object({
  id,
  at: isoDateTime,
  capability: z.string().min(1),
  nodeId: z.string().min(1),
  requestId: z.string().min(1),
  leaseId: id.optional(),
  ok: z.boolean(),
  /** Structured result summary (size-bounded by the emitter). */
  result: z.unknown().optional(),
  error: z.string().optional(),
  /** Input keys only — values are intentionally omitted for audit privacy. */
  inputKeys: z.array(z.string()),
  resource: z.string().optional(),
})
export type FabricInvocationReceipt = z.infer<typeof fabricInvocationReceiptSchema>

// ── Approval ─────────────────────────────────────────────────────────────────

export const approvalSchema = z.object({
  id,
  workloadId: id,
  prompt: z.string().min(1),
  status: z.enum(['pending', 'approved', 'rejected']),
  decidedAt: isoDateTime.optional(),
  decidedBy: z.string().optional(),
})
export type Approval = z.infer<typeof approvalSchema>

// ── Program manifest ─────────────────────────────────────────────────────────

export const programSurfaceSchema = z.object({
  /** Surface kind: web route, desktop window, or native mobile view. */
  kind: z.enum(['web-route', 'desktop-window', 'ios-view']),
  /** Route path (`/docs`), window target (`docs`), or iOS feature name. */
  ref: z.string().min(1),
})

export const programSidecarSchema = z.object({
  /** Managed child process the program needs (e.g. office-engine). */
  name: z.string().min(1),
  /** How the host lifecycle-manages it. */
  lifecycle: z.enum(['managed', 'external']),
  healthPath: z.string().optional(),
})

export const programManifestSchema = z.object({
  programId: z.string().min(1),
  name: z.string().min(1),
  version: z.string().min(1),
  /** `provisional` until the contract spine ADRs are ratified. */
  status: z.enum(['provisional', 'ratified']),
  contractSpineVersion: z.string().min(1),
  capabilities: z.array(capabilitySchema),
  surfaces: z.array(programSurfaceSchema),
  sidecars: z.array(programSidecarSchema).default([]),
})
export type ProgramManifest = z.infer<typeof programManifestSchema>
