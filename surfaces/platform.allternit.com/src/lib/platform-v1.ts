/**
 * Platform API `/v1` client for the console.
 *
 * The console calls the same `/v1` endpoints developers call, with the signed-in
 * Clerk session plus `X-Allternit-Project: proj_…`. The cloud API then acts on
 * that project as its owner (every scope, no account binding; see
 * cmd/allternit-cloud-api/src/routes/platform_v1/caller.rs `console_caller`).
 * A project the user can't manage answers 404 `project_not_found`.
 */

import { api } from "@/lib/api-client";

export const PROJECT_HEADER = "X-Allternit-Project";

export interface V1Page<T> {
  data: T[];
  has_more: boolean;
  next_cursor: string | null;
}

export interface V1Account {
  id: string;
  object: "account";
  name: string;
  external_ref: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
}

export type AgentTool =
  | "send_text"
  | "call"
  | "email"
  | "channel_post"
  | "calendar"
  | "web_fetch"
  | "web_search"
  | "knowledge_search"
  | "ask_human"
  | "transfer";

export type Autonomy = "draft" | "ask" | "tell" | "limits";

export interface V1Agent {
  id: string;
  object: "agent";
  account_id: string;
  name: string;
  instructions: string;
  greeting: string;
  model: string;
  voice: string;
  tools: AgentTool[];
  autonomy: Autonomy;
  transfer_targets: string[];
  business_hours: Record<string, unknown> | null;
  metadata: Record<string, unknown>;
  status: "pending" | "ready";
  created_at: string;
  updated_at: string;
}

export interface AgentInput {
  account_id?: string;
  name?: string;
  instructions?: string;
  greeting?: string;
  model?: string;
  voice?: string;
  tools?: AgentTool[];
  autonomy?: Autonomy;
  transfer_targets?: string[];
}

export interface V1ConversationMessage {
  id: string;
  object: "conversation.message";
  conversation_id: string;
  role: "user" | "assistant";
  content: string;
  status: "completed" | "failed";
  error: string | null;
  created_at: string;
}

export interface V1Conversation {
  id: string;
  object: "conversation";
  agent_id: string;
  account_id: string;
  metadata: Record<string, unknown>;
  created_at: string;
  messages?: V1ConversationMessage[];
}

export type ModelProvider = "anthropic" | "openai" | "xai";

export interface V1ModelKey {
  provider: ModelProvider;
  object: "model_key";
  masked: string;
  created_at: string;
  updated_at: string;
}

export interface V1UsageRow {
  group: string | null;
  meter: string;
  unit: string | null;
  quantity: number;
  events: number;
}

export interface V1Usage {
  object: "usage";
  group_by: string;
  from: string;
  to: string;
  data: V1UsageRow[];
}

export interface V1Webhook {
  id: string;
  object: "webhook_endpoint";
  url: string;
  events: string[];
  description: string | null;
  created_at: string;
}

export interface V1WebhookDelivery {
  id: string;
  object: "webhook_delivery";
  event_id: string;
  event_type: string;
  state: string;
  attempts: number;
  last_status: number | null;
  last_error: string | null;
  next_attempt_at: string | null;
  delivered_at: string | null;
  created_at: string;
}

export type ComputerStatus = "provisioning" | "running" | "stopped" | "starting" | "stopping" | "error" | "deleted";

/** A hosted computer (hosted computer driver, `/v1/computers`). */
export interface V1Computer {
  id: string;
  object: "computer";
  name: string;
  status: ComputerStatus;
  account_id: string | null;
  key_id: string;
  created_at: string;
  started_at: string | null;
  metadata: Record<string, unknown>;
}

export type ApprovalMode = "owner" | "api_key";

/** `GET/PATCH /v1/computer_settings`. `hosted_driver_enabled` is set by Allternit only. */
export interface V1ComputerSettings {
  hosted_driver_enabled: boolean;
  approval_mode: ApprovalMode;
  per_key_concurrency: number;
  browser_toolset: boolean;
}

/** Stock voices (spec §6: no custom or cloned voices). Mirrors `StockVoice` in platform-v1.yaml. */
export const STOCK_VOICES = [
  "af_alloy", "af_aoede", "af_bella", "af_heart", "af_jessica", "af_kore", "af_nicole", "af_nova",
  "af_river", "af_sarah", "af_sky", "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam",
  "am_michael", "am_onyx", "am_puck", "am_santa", "bf_alice", "bf_emma", "bf_isabella", "bf_lily",
  "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
] as const;

export const AGENT_TOOLS: { value: AgentTool; label: string; available: boolean }[] = [
  { value: "send_text", label: "Send texts", available: true },
  { value: "call", label: "Place calls", available: true },
  { value: "email", label: "Email", available: true },
  { value: "web_fetch", label: "Fetch web pages", available: true },
  { value: "web_search", label: "Web search", available: true },
  { value: "ask_human", label: "Ask a human", available: true },
  { value: "channel_post", label: "Post to channels", available: false },
  { value: "calendar", label: "Calendar", available: false },
  { value: "knowledge_search", label: "Knowledge search", available: false },
  { value: "transfer", label: "Transfer calls", available: false },
];

export const AUTONOMY_LEVELS: { value: Autonomy; label: string }[] = [
  { value: "draft", label: "Draft: writes, never sends" },
  { value: "ask", label: "Ask: asks a human first" },
  { value: "tell", label: "Tell: acts, then tells you" },
  { value: "limits", label: "Limits: acts within set limits" },
];

export const MODEL_PROVIDERS: { value: ModelProvider; label: string }[] = [
  { value: "anthropic", label: "Anthropic" },
  { value: "openai", label: "OpenAI" },
  { value: "xai", label: "xAI" },
];

/** Webhook event types the API accepts today (platform-v1.yaml createWebhook). */
export const WEBHOOK_EVENT_TYPES = ["message.received", "message.status", "registration.updated"] as const;

export const USAGE_METER_LABELS: Record<string, string> = {
  voice_min_allternit: "Voice minutes (Allternit model)",
  voice_min_byok: "Voice minutes (your model key)",
  agent_month: "Hosted agent months",
  tokens_in: "Model tokens in",
  tokens_out: "Model tokens out",
  number_local_month: "Local number months",
  number_tollfree_month: "Toll-free number months",
  sms_segment: "SMS segments",
  mms: "MMS messages",
  registration_passthrough_cents: "Carrier registration (cents)",
  recording_min_month: "Recording storage (minute-months)",
  computer_minute: "Hosted computer minutes",
  computer_action: "Hosted computer actions",
};

/** A `/v1` client bound to one project. */
export function v1(projectId: string) {
  const headers = { [PROJECT_HEADER]: projectId };
  const opts = { headers };
  const enc = encodeURIComponent;
  const qs = (params: Record<string, string | number | undefined | null>) => {
    const p = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== "") p.set(k, String(v));
    const s = p.toString();
    return s ? `?${s}` : "";
  };

  return {
    listAccounts: (after?: string | null) => api.get<V1Page<V1Account>>(`/v1/accounts${qs({ limit: 100, after })}`, opts),
    createAccount: (input: { name: string; external_ref?: string }) => api.post<V1Account>("/v1/accounts", input, opts),

    listAgents: (after?: string | null) => api.get<V1Page<V1Agent>>(`/v1/agents${qs({ limit: 100, after })}`, opts),
    getAgent: (id: string) => api.get<V1Agent>(`/v1/agents/${enc(id)}`, opts),
    createAgent: (input: AgentInput) => api.post<V1Agent>("/v1/agents", input, opts),
    updateAgent: (id: string, input: AgentInput) => api.patch<V1Agent>(`/v1/agents/${enc(id)}`, input, opts),
    deleteAgent: (id: string) => api.delete<{ id: string; deleted: boolean }>(`/v1/agents/${enc(id)}`, opts),

    listConversations: (agentId: string, after?: string | null) =>
      api.get<V1Page<V1Conversation>>(`/v1/agents/${enc(agentId)}/conversations${qs({ limit: 50, after })}`, opts),
    getConversation: (id: string) => api.get<V1Conversation>(`/v1/conversations/${enc(id)}`, opts),

    listModelKeys: () => api.get<{ data: V1ModelKey[] }>("/v1/model_keys", opts),
    putModelKey: (provider: ModelProvider, apiKey: string) =>
      api.put<V1ModelKey>(`/v1/model_keys/${provider}`, { api_key: apiKey }, opts),
    deleteModelKey: (provider: ModelProvider) => api.delete(`/v1/model_keys/${provider}`, opts),

    usage: (params: { group_by: "meter" | "key" | "account"; from?: string; to?: string }) =>
      api.get<V1Usage>(`/v1/usage${qs(params)}`, opts),

    listComputers: (after?: string | null) => api.get<V1Page<V1Computer>>(`/v1/computers${qs({ limit: 100, after })}`, opts),
    startComputer: (id: string) => api.post<V1Computer>(`/v1/computers/${enc(id)}/start`, undefined, opts),
    stopComputer: (id: string) => api.post<V1Computer>(`/v1/computers/${enc(id)}/stop`, undefined, opts),
    deleteComputer: (id: string) => api.delete<V1Computer>(`/v1/computers/${enc(id)}`, opts),
    getComputerSettings: () => api.get<V1ComputerSettings>("/v1/computer_settings", opts),
    updateComputerSettings: (input: Partial<Omit<V1ComputerSettings, "hosted_driver_enabled">>) =>
      api.patch<V1ComputerSettings>("/v1/computer_settings", input, opts),

    listWebhooks: () => api.get<V1Page<V1Webhook>>(`/v1/webhooks${qs({ limit: 100 })}`, opts),
    createWebhook: (input: { url: string; events: string[]; description?: string }) =>
      api.post<V1Webhook & { secret: string }>("/v1/webhooks", input, opts),
    deleteWebhook: (id: string) => api.delete(`/v1/webhooks/${enc(id)}`, opts),
    testWebhook: (id: string) => api.post<{ event_id: string; queued: boolean }>(`/v1/webhooks/${enc(id)}/test`, undefined, opts),
    listDeliveries: (id: string, after?: string | null) =>
      api.get<V1Page<V1WebhookDelivery>>(`/v1/webhooks/${enc(id)}/deliveries${qs({ limit: 50, after })}`, opts),
    redeliver: (id: string, deliveryId: string) =>
      api.post<{ id: string; queued: boolean }>(`/v1/webhooks/${enc(id)}/deliveries/${enc(deliveryId)}/redeliver`, undefined, opts),
  };
}

export type V1Client = ReturnType<typeof v1>;

/** Every page of a cursor list (the console's lists are small; capped at 20 pages). */
export async function allPages<T>(fetchPage: (after: string | null) => Promise<V1Page<T>>): Promise<T[]> {
  const out: T[] = [];
  let after: string | null = null;
  for (let i = 0; i < 20; i++) {
    const page: V1Page<T> = await fetchPage(after);
    out.push(...page.data);
    if (!page.has_more || !page.next_cursor) break;
    after = page.next_cursor;
  }
  return out;
}
