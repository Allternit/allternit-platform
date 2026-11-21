/**
 * Managed Agents console — types and API helpers.
 *
 * Every shape here was confirmed against the backend handlers:
 * - Agents CRUD + toolset + subagents: `cmd/allternit-api/src/agent_routes.rs`
 *   (AgentRow @154-199, CreateAgentBody @388-439, toolset @1653-1926,
 *   subagents @1210-1370, TOOL_PERMISSION_VALUES @1721).
 * - Cloud session facade: `cmd/allternit-api/src/cloud_agents_routes.rs`
 *   (router @46-59, public_session @221-254, CreateCloudSessionBody @114-142,
 *   SendEventsBody @158-161, turns/threads/outputs @1282-1383).
 * - Deployments: `cmd/allternit-api/src/beta_deployment_routes.rs`
 *   (router @30-56, bodies @58-86, rows @88-111).
 * - Computers: `cmd/allternit-api/src/computer_routes.rs`
 *   (ComputerResponse @66-93, ListComputersQuery @125-133, router @153-186).
 * - Vaults: `cmd/allternit-api/src/allternit_vault.rs`
 *   (router @103-145, vault_json @210-214, credential_json @478-493).
 * - Memory stores: `cmd/allternit-api/src/beta_memory_store_routes.rs`
 *   (router @60-76, rows @87-120, entries @278-514).
 */

import { api } from "@/lib/api-client";

// ─── Agents ──────────────────────────────────────────────────────────────────

/** Mirrors agent_routes.rs AgentRow (the canonical agents column list). */
export interface AgentRecord {
  id: string;
  user_id: string;
  name: string;
  description?: string | null;
  type?: string;
  parent_agent_id?: string | null;
  /** The brain (model id). */
  model: string;
  provider: string;
  /** Runtime registry id (Harness & Bots 2.0); absent on old records = "allternit". */
  runtime?: string | null;
  /** How the brain is paid for. */
  access?: "plan" | "key" | "local" | "credits" | null;
  agentView?: { view?: string; look?: string } | null;
  capabilities?: unknown;
  system_prompt?: string | null;
  tools?: unknown;
  max_iterations?: number;
  temperature?: number;
  config?: Record<string, unknown> | null;
  status: string;
  workspace_id?: string | null;
  trust_tier?: string;
  allowed_skills?: unknown;
  allowed_tools?: unknown;
  created_at: string;
  updated_at: string;
  last_run_at?: string | null;
  mode?: string;
  version?: number;
  /** First-class bot flag (V143 column); bot rows are the "Hub" agents. */
  is_bot: boolean;
  tool_permissions?: Record<string, string> | null;
  mcp_connector_ids?: unknown;
}

/** Mirror of agent_routes.rs SubagentRow. */
export interface SubagentRecord {
  id: string;
  name: string;
  type?: string;
  mode?: string;
  parent_agent_id?: string | null;
  model: string;
  provider: string;
  status: string;
  created_at: string;
}

export interface AgentToolset {
  tools: unknown;
  allowed_tools: unknown;
  allowed_skills: unknown;
  mcp: Array<{ id: string; name: string; url?: string | null; enabled: boolean }>;
  mcp_connector_ids: string[];
  tool_permissions: Record<string, string>;
  tool_search: boolean;
  programmatic: boolean;
}

/** Per-tool permission values accepted by PUT /agents/:id/toolset. */
export const TOOL_PERMISSIONS = ["auto", "always_allow", "always_ask"] as const;
export type ToolPermission = (typeof TOOL_PERMISSIONS)[number];

export function toolNames(tools: unknown): string[] {
  if (!Array.isArray(tools)) return [];
  const names: string[] = [];
  for (const item of tools) {
    if (typeof item === "string") names.push(item);
    else if (item && typeof item === "object" && typeof (item as { name?: unknown }).name === "string") {
      names.push((item as { name: string }).name);
    }
  }
  return names;
}

export async function listAgents(): Promise<AgentRecord[]> {
  const data = await api.get<{ agents?: AgentRecord[] }>("/api/v1/agents");
  return data.agents ?? [];
}

export async function getAgent(id: string): Promise<AgentRecord> {
  const data = await api.get<{ agent: AgentRecord }>(
    `/api/v1/agents/${encodeURIComponent(id)}`
  );
  return data.agent;
}

export async function createAgent(body: Record<string, unknown>): Promise<AgentRecord> {
  const data = await api.post<{ agent: AgentRecord }>("/api/v1/agents", body);
  return data.agent;
}

export async function patchAgent(id: string, body: Record<string, unknown>): Promise<AgentRecord> {
  const data = await api.patch<{ agent: AgentRecord }>(
    `/api/v1/agents/${encodeURIComponent(id)}`,
    body
  );
  return data.agent;
}

export async function archiveAgent(id: string): Promise<void> {
  await api.post(`/api/v1/agents/${encodeURIComponent(id)}/archive`, {});
}

export async function getAgentToolset(id: string): Promise<AgentToolset> {
  const data = await api.get<{ toolset: AgentToolset }>(
    `/api/v1/agents/${encodeURIComponent(id)}/toolset`
  );
  return data.toolset;
}

export async function putAgentToolset(
  id: string,
  body: {
    tools?: unknown;
    allowed_tools?: unknown;
    allowed_skills?: unknown;
    mcp_connector_ids?: string[];
    tool_permissions?: Record<string, string>;
  }
): Promise<AgentToolset> {
  const data = await api.put<{ toolset: AgentToolset }>(
    `/api/v1/agents/${encodeURIComponent(id)}/toolset`,
    body
  );
  return data.toolset;
}

export async function listSubagents(id: string): Promise<SubagentRecord[]> {
  const data = await api.get<{ subagents?: SubagentRecord[] }>(
    `/api/v1/agents/${encodeURIComponent(id)}/subagents`
  );
  return data.subagents ?? [];
}

export async function createSubagent(
  parentId: string,
  body: Record<string, unknown>
): Promise<void> {
  await api.post(`/api/v1/agents/${encodeURIComponent(parentId)}/subagents`, body);
}

/** Cloud skills (see pages/console/SkillsPage.tsx). */
export interface CloudSkillRef {
  id: string;
  name: string;
  description?: string | null;
}

export async function listSkillRefs(): Promise<CloudSkillRef[]> {
  const data = await api.get<{ skills?: CloudSkillRef[] }>("/api/v1/skills");
  return data.skills ?? [];
}

// ─── Sessions (cloud session facade) ─────────────────────────────────────────

export interface SessionBudget {
  tokens_used?: number;
  estimated_cost_usd?: number;
  estimated_cost_cents?: number;
  charged?: boolean;
  max_cost_usd?: number | null;
  over_budget?: boolean;
  [key: string]: unknown;
}

/** Mirror of cloud_agents_routes.rs public_session. */
export interface SessionRecord {
  id: string;
  agent_id?: string | null;
  name?: string | null;
  status?: string;
  metadata?: Record<string, unknown>;
  budget?: SessionBudget;
  computer?: { kind?: string; id?: string | null };
  brain_id?: unknown;
  vault_ids?: unknown;
  memory_store_ids?: unknown;
  bot_id?: unknown;
  parent_thread_id?: string | null;
  permission?: unknown;
  effective_permissions?: unknown;
  created_at: string;
  updated_at: string;
  archived_at?: string | null;
}

/** Mirror of cloud_agents_routes.rs turns_from_events. */
export interface SessionTurn {
  id?: string;
  status?: string;
  started_at?: string | null;
  completed_at?: string | null;
}

/** Mirror of cloud_agents_routes.rs list_cloud_outputs. */
export interface SessionOutput {
  id: string;
  filename: string;
  mime_type?: string | null;
  size_bytes?: number | null;
  created_at: string;
}

/** SSE event on GET /sessions/:id/events/stream — translate_event base shape. */
export interface SessionStreamEvent {
  id?: string;
  sequence?: number;
  type?: string;
  session_id?: string;
  created_at?: string;
  data?: Record<string, unknown>;
}

export const SESSION_COMPUTER_KINDS = ["none", "local", "sandbox", "desktop", "fabric"] as const;
export type SessionComputerKind = (typeof SESSION_COMPUTER_KINDS)[number];

export async function listSessions(): Promise<SessionRecord[]> {
  const data = await api.get<{ sessions?: SessionRecord[] }>("/api/v1/sessions");
  return data.sessions ?? [];
}

export async function getSession(id: string): Promise<SessionRecord> {
  const data = await api.get<{ session?: SessionRecord }>(
    `/api/v1/sessions/${encodeURIComponent(id)}`
  );
  if (!data.session) throw new Error("Session not found");
  return data.session;
}

export async function createSession(body: Record<string, unknown>): Promise<SessionRecord> {
  const response = await api.raw("/api/v1/sessions", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const json = (await response.json().catch(() => ({}))) as {
    session?: SessionRecord;
    error?: string;
    message?: string;
    code?: string;
  };
  if (response.status === 503 || json.code === "computer_unavailable") {
    const unavailable = new Error(
      json.error || json.message || "Computer Cloud has no VM driver on this host"
    ) as Error & { unavailable?: boolean };
    unavailable.unavailable = true;
    throw unavailable;
  }
  if (!response.ok || !json.session) {
    throw new Error(json.error || json.message || `HTTP ${response.status}`);
  }
  return json.session;
}

export async function archiveSession(id: string): Promise<void> {
  await api.post(`/api/v1/sessions/${encodeURIComponent(id)}/archive`, {});
}

export type SessionEventInput =
  | { type: "user.message"; content: string }
  | { type: "user.interrupt" }
  | { type: "user.tool_result"; data?: Record<string, unknown> };

export async function sendSessionEvents(id: string, events: SessionEventInput[]): Promise<void> {
  await api.post(`/api/v1/sessions/${encodeURIComponent(id)}/events`, { events });
}

export async function listSessionTurns(id: string): Promise<SessionTurn[]> {
  const data = await api.get<{ turns?: SessionTurn[] }>(
    `/api/v1/sessions/${encodeURIComponent(id)}/turns`
  );
  return data.turns ?? [];
}

export async function listSessionThreads(id: string): Promise<SessionRecord[]> {
  const data = await api.get<{ threads?: SessionRecord[] }>(
    `/api/v1/sessions/${encodeURIComponent(id)}/threads`
  );
  return data.threads ?? [];
}

export async function listSessionOutputs(id: string): Promise<SessionOutput[]> {
  const data = await api.get<{ outputs?: SessionOutput[] }>(
    `/api/v1/sessions/${encodeURIComponent(id)}/outputs`
  );
  return data.outputs ?? [];
}

// ─── Deployments ─────────────────────────────────────────────────────────────

/** Mirror of beta_deployment_routes.rs DeploymentRow. */
export interface DeploymentRecord {
  id: string;
  agent_id?: string | null;
  cron: string;
  next_run_at?: string | null;
  last_run_at?: string | null;
  status: string;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

/** Mirror of beta_deployment_routes.rs RunRow. */
export interface DeploymentRun {
  id: string;
  deployment_id: string;
  status: string;
  result?: unknown;
  error?: string | null;
  started_at: string;
  finished_at?: string | null;
  triggered_by?: string | null;
}

export const DEPLOYMENT_STATUSES = ["active", "paused", "archived"] as const;

export async function listDeployments(status?: string): Promise<DeploymentRecord[]> {
  const query = status ? `?status=${encodeURIComponent(status)}` : "";
  const data = await api.get<{ deployments?: DeploymentRecord[] }>(
    `/api/v1/beta/deployments${query}`
  );
  return data.deployments ?? [];
}

export async function createDeployment(body: {
  agent_id?: string;
  cron: string;
  metadata: Record<string, unknown>;
}): Promise<DeploymentRecord> {
  const data = await api.post<{ deployment: DeploymentRecord }>(
    "/api/v1/beta/deployments",
    body
  );
  return data.deployment;
}

export async function patchDeployment(
  id: string,
  body: {
    agent_id?: string;
    cron?: string;
    status?: string;
    metadata?: Record<string, unknown>;
  }
): Promise<DeploymentRecord> {
  const data = await api.patch<{ deployment: DeploymentRecord }>(
    `/api/v1/beta/deployments/${encodeURIComponent(id)}`,
    body
  );
  return data.deployment;
}

export async function deleteDeployment(id: string): Promise<void> {
  await api.delete(`/api/v1/beta/deployments/${encodeURIComponent(id)}`);
}

export async function listDeploymentRuns(id: string): Promise<DeploymentRun[]> {
  const data = await api.get<{ runs?: DeploymentRun[] }>(
    `/api/v1/beta/deployments/${encodeURIComponent(id)}/runs`
  );
  return data.runs ?? [];
}

/** Insert a `running` run row + the work task a worker leases. */
export async function triggerDeployment(id: string): Promise<{ run_id?: string }> {
  return api.post(`/api/v1/beta/deployments/${encodeURIComponent(id)}/runs`, {});
}

export function deploymentName(d: DeploymentRecord): string {
  const name = d.metadata?.name;
  return typeof name === "string" && name.trim() ? name : d.id.slice(0, 8);
}

// ─── Computers ───────────────────────────────────────────────────────────────

/** Mirror of computer_routes.rs ComputerResponse. */
export interface ComputerRecord {
  id: string;
  kind: string;
  provider: string;
  status: string;
  owner_type: string;
  owner_id: string;
  bot_id?: string | null;
  session_id?: string | null;
  name: string;
  os?: string | null;
  cpu_cores?: number | null;
  memory_mb?: number | null;
  disk_mb?: number | null;
  region?: string | null;
  host?: string | null;
  template_id?: string | null;
  billing_source: string;
  created_at: string;
  updated_at: string;
  idle_timeout_secs?: number | null;
  last_activity_at?: string | null;
  group_id?: string | null;
  role?: string;
}

/** The `kind` query filter values the backend accepts (ComputerKind enum). */
export const COMPUTER_KINDS = ["local", "byo_vps", "managed", "byoc", "cloud_desktop"] as const;
export type ComputerKindFilter = (typeof COMPUTER_KINDS)[number];

export async function listComputers(kind?: string): Promise<ComputerRecord[]> {
  const query = kind ? `?kind=${encodeURIComponent(kind)}` : "";
  const data = await api.get<{ computers?: ComputerRecord[] }>(
    `/api/v1/computers${query}`
  );
  return data.computers ?? [];
}

// ─── Vaults ──────────────────────────────────────────────────────────────────

/** Mirror of allternit_vault.rs vault_json. Vaults have no status field.
 *  Create returns only {id, name, description} — the audit fields are
 *  populated from list/get/patch responses. */
export interface VaultRecord {
  id: string;
  name: string;
  description?: string | null;
  created_by?: string;
  created_at?: string;
  updated_at?: string;
}

/** Mirror of allternit_vault.rs credential_json. Secrets never round-trip. */
export interface VaultCredential {
  id: string;
  credential_type: string;
  provider: string;
  username?: string | null;
  origin_pattern?: string | null;
  agent_id?: string | null;
  session_id?: string | null;
  expires_at?: string | null;
  last_used_at?: string | null;
  use_count: number;
  created_at: string;
  updated_at: string;
}

export async function listVaults(): Promise<VaultRecord[]> {
  const data = await api.get<{ vaults?: VaultRecord[] }>("/api/v1/vaults");
  return data.vaults ?? [];
}

export async function createVault(body: { name: string; description?: string }): Promise<VaultRecord> {
  return api.post<VaultRecord>("/api/v1/vaults", body);
}

export async function patchVault(
  id: string,
  body: { name?: string; description?: string }
): Promise<VaultRecord> {
  return api.patch<VaultRecord>(`/api/v1/vaults/${encodeURIComponent(id)}`, body);
}

export async function deleteVault(id: string): Promise<void> {
  await api.delete(`/api/v1/vaults/${encodeURIComponent(id)}`);
}

export async function listVaultCredentials(id: string): Promise<VaultCredential[]> {
  const data = await api.get<{ credentials?: VaultCredential[] }>(
    `/api/v1/beta/vaults/${encodeURIComponent(id)}/credentials`
  );
  return data.credentials ?? [];
}

export async function putVaultCredential(
  id: string,
  body: { provider: string; oauth_value: string; expires_at?: string }
): Promise<void> {
  await api.post(`/api/v1/beta/vaults/${encodeURIComponent(id)}/credentials`, body);
}

export async function putVaultPasswordCredential(
  id: string,
  body: {
    provider: string;
    username: string;
    password: string;
    origin_pattern?: string;
    expires_at?: string;
  }
): Promise<void> {
  await api.post(`/api/v1/beta/vaults/${encodeURIComponent(id)}/credentials/password`, body);
}

export async function deleteVaultCredential(vaultId: string, credentialId: string): Promise<void> {
  await api.delete(
    `/api/v1/beta/vaults/${encodeURIComponent(vaultId)}/credentials/${encodeURIComponent(credentialId)}`
  );
}

// ─── Memory stores ───────────────────────────────────────────────────────────

/** Mirror of beta_memory_store_routes.rs MemoryStoreRow. */
export interface MemoryStoreRecord {
  id: string;
  organization_id?: string | null;
  name: string;
  redaction_policy: Record<string, unknown>;
  metadata: Record<string, unknown>;
  entry_count: number;
  last_write_at?: string | null;
  created_at: string;
  updated_at: string;
}

/** Mirror of beta_memory_store_routes.rs MemoryEntryRow. */
export interface MemoryEntryRecord {
  id: string;
  store_id: string;
  namespace: string;
  key: string;
  value: string;
  created_at: string;
  updated_at: string;
}

export async function listMemoryStores(): Promise<MemoryStoreRecord[]> {
  const data = await api.get<{ memory_stores?: MemoryStoreRecord[] }>(
    "/api/v1/beta/memory-stores"
  );
  return data.memory_stores ?? [];
}

export async function createMemoryStore(name: string): Promise<MemoryStoreRecord> {
  const data = await api.post<{ memory_store: MemoryStoreRecord }>(
    "/api/v1/beta/memory-stores",
    { name, redaction_policy: {}, metadata: {} }
  );
  return data.memory_store;
}

export async function deleteMemoryStore(id: string): Promise<void> {
  await api.delete(`/api/v1/beta/memory-stores/${encodeURIComponent(id)}`);
}

export async function listMemoryEntries(
  storeId: string,
  options: { namespace?: string; cursor?: string; limit?: number } = {}
): Promise<{ entries: MemoryEntryRecord[]; next_cursor?: string | null }> {
  const params = new URLSearchParams();
  if (options.namespace) params.set("namespace", options.namespace);
  if (options.cursor) params.set("cursor", options.cursor);
  if (options.limit) params.set("limit", String(options.limit));
  const qs = params.toString();
  return api.get(`/api/v1/beta/memory-stores/${encodeURIComponent(storeId)}/entries${qs ? `?${qs}` : ""}`);
}

export async function getMemoryEntry(
  storeId: string,
  key: string,
  namespace?: string
): Promise<MemoryEntryRecord> {
  const params = new URLSearchParams();
  if (namespace) params.set("namespace", namespace);
  const qs = params.toString();
  const data = await api.get<{ entry: MemoryEntryRecord }>(
    `/api/v1/beta/memory-stores/${encodeURIComponent(storeId)}/entries/${encodeURIComponent(key)}${qs ? `?${qs}` : ""}`
  );
  return data.entry;
}

export async function putMemoryEntry(
  storeId: string,
  key: string,
  value: string,
  namespace?: string
): Promise<MemoryEntryRecord> {
  const data = await api.put<{ entry: MemoryEntryRecord }>(
    `/api/v1/beta/memory-stores/${encodeURIComponent(storeId)}/entries/${encodeURIComponent(key)}`,
    { value, namespace: namespace || undefined }
  );
  return data.entry;
}

export async function deleteMemoryEntry(
  storeId: string,
  key: string,
  namespace?: string
): Promise<void> {
  const params = new URLSearchParams();
  if (namespace) params.set("namespace", namespace);
  const qs = params.toString();
  await api.delete(
    `/api/v1/beta/memory-stores/${encodeURIComponent(storeId)}/entries/${encodeURIComponent(key)}${qs ? `?${qs}` : ""}`
  );
}

export async function searchMemoryEntries(
  storeId: string,
  q: string,
  limit = 50
): Promise<MemoryEntryRecord[]> {
  const data = await api.get<{ entries?: MemoryEntryRecord[] }>(
    `/api/v1/beta/memory-stores/${encodeURIComponent(storeId)}/search?q=${encodeURIComponent(q)}&limit=${limit}`
  );
  return data.entries ?? [];
}

// ─── Shared formatting ───────────────────────────────────────────────────────

export function formatTime(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const diff = Date.now() - date.getTime();
  if (diff < 60_000) return "Just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} hr ago`;
  return date.toLocaleDateString();
}

export function formatDate(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString();
}

export function formatBytes(bytes?: number | null): string {
  if (bytes == null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
