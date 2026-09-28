/**
 * A:// types, mirrored from the Rust sources of truth:
 *   - `infrastructure/executor/cowork/cowork/allternit-cowork-runtime/src/transport.rs`
 *     (IntentEnvelope, IntentAction, LeaseGrant)
 *   - `cmd/allternit-api/src/rails/fabric_transport_routes.rs` (request bodies)
 *   - `cmd/allternit-api/src/cowork_routes.rs` (memory)
 * Fields the server builds ad hoc are typed where they're stable and left
 * open otherwise (`[key: string]: unknown`).
 */

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

/** An A:// principal id, e.g. `a://workspace/ws/bot/ledger`. */
export type PrincipalId = string

export interface Principal {
  id: PrincipalId
  workspace: string
  capabilities: string[]
  roles?: string[]
  status?: string
  [key: string]: unknown
}

export interface CreatePrincipalRequest {
  id: PrincipalId
  workspace: string
  capabilities: string[]
  roles?: string[]
}

/** Returned once: the token is not stored in plain text server-side. */
export interface CreatePrincipalResponse {
  id: PrincipalId
  workspace: string
  capabilities: string[]
  token: string
}

export interface EnsureWorkerPrincipalResponse {
  principal_id: PrincipalId
  workspace: string
  token: string
}

export interface IntentAction {
  action_type: string
  description: string
  payload?: Json
}

/** Compute placement: "auto" | "local" | "cloud" | "vm" | "byo" (→ required capability). */
export type ComputePolicy = "auto" | "local" | "cloud" | "vm" | "byo" | { policy: string; [key: string]: Json }

export interface IntentEnvelope {
  version: string
  intent_id: string
  workspace: string
  initiator: PrincipalId
  delegator?: PrincipalId | null
  target?: PrincipalId | null
  action: IntentAction
  permissions?: string[]
  compute?: ComputePolicy | null
  model?: Json
  approval?: Json
  return_channel?: Json
  causation_chain?: string[]
}

export interface IntentView {
  intent_id: string
  status?: string
  run_id?: string
  job_ids?: string[]
  [key: string]: unknown
}

export interface LeaseGrant {
  job_id: string
  run_id: string
  lease_id: string
  lease_generation: number
  lease_expires_at: string
  payload: Json
  required_capabilities: string[]
  current_checkpoint_id: string | null
  initiator: PrincipalId | null
  delegator: PrincipalId | null
}

export interface Lease {
  lease_id: string
  lease_generation: number
}

export interface ClaimRequest {
  job_id?: string
  wait_secs?: number
  lease_ttl_secs?: number
}

export interface CompleteRequest extends Lease {
  success: boolean
  summary?: string
  outputs?: Json
}

export interface JobView {
  id: string
  run_id: string
  state: string
  [key: string]: unknown
}

export interface ApprovalScopeRequest extends Lease {
  capability: string
  target: string
  approval_ttl_secs?: number
}

export interface Approval {
  id: string
  status: string
  capability?: string
  target?: string
  job_id?: string
  [key: string]: unknown
}

export interface DelegationRule {
  workspace: string
  action_type: string
  target_principal: PrincipalId
  priority?: number
}

export interface MemoryEntry {
  id: string
  content: string
  type: string
  tags: string | null
  source: string | null
  owner_principal: PrincipalId | null
  grants: string
  created_at: string
}

export interface StoreMemoryRequest {
  content: string
  type?: string
  tags?: string
  source?: string
  project_id?: string
  session_id?: string
  /** Owning principal; or `bot` to own it by a bot's principal. */
  principal?: PrincipalId
  /** Principals explicitly granted access (default-deny otherwise). */
  grants?: PrincipalId[]
  bot?: string
}

export interface Dag {
  id: string
  [key: string]: unknown
}

export interface DagNode {
  id: string
  [key: string]: unknown
}
