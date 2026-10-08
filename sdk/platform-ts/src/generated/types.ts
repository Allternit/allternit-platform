// Generated from cmd/allternit-cloud-api/openapi/platform-v1.yaml by scripts/platform-sdk/generate.py. Do not edit.
/* eslint-disable */

export type Call = {
  id?: string;
  object?: "call";
  account_id?: string;
  agent_id?: string;
  number_id?: (string | null);
  direction?: "inbound" | "outbound" | "realtime";
  from?: (string | null);
  to?: (string | null);
  purpose?: (string | null);
  status?: "queued" | "ringing" | "in_progress" | "completed" | "failed" | "no_answer" | "canceled";
  end_reason?: (string | null);
  simulated?: boolean;
  recording?: boolean;
  transferred_to?: (string | null);
  duration_seconds?: (number | null);
  created_at?: string;
  answered_at?: (string | null);
  ended_at?: (string | null);
};

export type Error = {
  error: {
    type: "invalid_request_error" | "authentication_error" | "permission_error" | "not_found_error" | "rate_limit_error" | "conflict_error" | "billing_error" | "api_error";
    code: string;
    message: string;
    param: (string | null);
  };
};

export type SpendCap = {
  object: "spend_cap";
  project_id: string;
  plan: "sandbox" | "payg" | "growth" | "enterprise";
  spend_cap_cents: number;
  /**
   * The month, e.g. 2026-10 (UTC).
   */
  period: string;
  /**
   * Spend this month
   */
  spent_cents: number;
  spent_microusd: number;
  remaining_cents: number;
  /**
   * true once spend is at or over the cap: billable requests answer 402 spend_cap_reached.
   */
  reached: boolean;
  /**
   * usage.threshold webhooks already sent this month (50, 80, 100).
   */
  thresholds_sent: Array<number>;
};

export type ListEnvelope = {
  data: Array<unknown>;
  has_more: boolean;
  /**
   * Pass as `after` to get the next page; null on the last page.
   */
  next_cursor: (string | null);
};

export type Account = {
  id: string;
  object: "account";
  name: string;
  external_ref: (string | null);
  metadata: Record<string, unknown>;
  created_at: string;
};

export type Agent = {
  id: string;
  object: "agent";
  account_id: string;
  name: string;
  instructions: string;
  greeting: string;
  model: string;
  voice: StockVoice;
  tools: Array<AgentTool>;
  autonomy: Autonomy;
  transfer_targets: Array<string>;
  business_hours: (Record<string, unknown> | null);
  metadata: Record<string, unknown>;
  /**
   * `ready` once the project's hosted runtime holds the agent.
   */
  status: "pending" | "ready";
  created_at: string;
  updated_at: string;
};

export type KnowledgeFile = {
  id: string;
  object: "knowledge_file";
  agent_id: string;
  account_id: string;
  name: string;
  content_type: string;
  bytes: number;
  /**
   * Searchable pieces (about 1,200 characters each).
   */
  chunk_count: number;
  created_at: string;
};

/**
 * calendar (needs end-customer Google/Microsoft calendar connections) and transfer (arrives with voice calls) aren't available for hosted agents yet (tool_not_available). knowledge_search searches the agent's knowledge files; channel_post posts to a channel connected for the agent's account.
 */
export type AgentTool = "send_text" | "call" | "email" | "channel_post" | "calendar" | "web_fetch" | "web_search" | "knowledge_search" | "ask_human" | "transfer";

/**
 * What the agent may do on its own: draft (writes, never sends), ask (asks a human first), tell (acts, then tells), limits (acts within set limits). Payments always wait for a human.
 */
export type Autonomy = "draft" | "ask" | "tell" | "limits";

export type StockVoice = "af_alloy" | "af_aoede" | "af_bella" | "af_heart" | "af_jessica" | "af_kore" | "af_nicole" | "af_nova" | "af_river" | "af_sarah" | "af_sky" | "am_adam" | "am_echo" | "am_eric" | "am_fenrir" | "am_liam" | "am_michael" | "am_onyx" | "am_puck" | "am_santa" | "bf_alice" | "bf_emma" | "bf_isabella" | "bf_lily" | "bm_daniel" | "bm_fable" | "bm_george" | "bm_lewis";

export type Conversation = {
  id: string;
  object: "conversation";
  agent_id: string;
  account_id: string;
  metadata: Record<string, unknown>;
  created_at: string;
  messages?: Array<ConversationMessage>;
};

export type ConversationMessage = {
  id: string;
  object: "conversation.message";
  conversation_id: string;
  role: "user" | "assistant";
  content: string;
  status: "completed" | "failed";
  error: (string | null);
  created_at: string;
};

export type ModelKey = {
  provider: "anthropic" | "openai" | "xai";
  object: "model_key";
  /**
   * Last four characters, e.g. "…7890".
   */
  masked: string;
  created_at: string;
  updated_at: string;
};

export type UsageRow = {
  /**
   * The meter, key id or account id, per `group_by`. Null for usage with no account.
   */
  group: (string | null);
  meter: "voice_min_allternit" | "voice_min_byok" | "agent_month" | "tokens_in" | "tokens_out" | "number_local_month" | "number_tollfree_month" | "sms_segment" | "mms" | "registration_passthrough_cents" | "recording_min_month";
  unit: (string | null);
  quantity: number;
  events: number;
};

export type PhoneNumber = {
  id: string;
  object: "phone_number";
  account_id: (string | null);
  e164: string;
  type: "local" | "toll_free";
  sms_state: "pending_registration" | "active" | "rejected" | "blocked";
  simulated: boolean;
  created_at: string;
};

export type PhoneNumberList = {
  data?: Array<PhoneNumber>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type Message = {
  id: string;
  object: "message";
  account_id?: (string | null);
  number_id: string;
  direction: "inbound" | "outbound";
  from: string;
  to: string;
  body: string;
  segments: number;
  status: "sent" | "simulated" | "received" | "failed";
  error_code?: (string | null);
  media?: Array<Record<string, unknown>>;
  created_at: string;
};

export type MessageList = {
  data?: Array<Message>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type WebhookEndpoint = {
  id: string;
  object: "webhook_endpoint";
  url: string;
  events: Array<string>;
  description?: (string | null);
  created_at: string;
};

/**
 * What a webhook receives. Header `allternit-signature: t=<ts>,v1=<hex HMAC-SHA256(secret, "<ts>.<body>")>`.
 */
export type Event = {
  id: string;
  object: "event";
  type: "approval.requested" | "message.received" | "message.status" | "registration.updated" | "inbox.item.created" | "call.started" | "call.ended" | "call.transcript.ready" | "usage.threshold" | "webhook.test";
  /**
   * Unix seconds.
   */
  created: number;
  project_id: string;
  account_id?: (string | null);
  data: Record<string, unknown>;
};

export type Computer = {
  id: string;
  object: "computer";
  name: string;
  status: "provisioning" | "starting" | "running" | "stopping" | "stopped" | "error" | "deleted";
  account_id?: (string | null);
  /**
   * The key that created it (`console` for the console).
   */
  key_id: string;
  created_at: string;
  started_at?: (string | null);
  metadata: Record<string, unknown>;
};

export type ComputerList = {
  data?: Array<Computer>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type ComputerEvent = {
  id?: string;
  type?: "computer.action";
  ts?: string;
  data?: {
    toolset?: string;
    member?: string;
    x?: (number | null);
    y?: (number | null);
    screen_w?: (number | null);
    screen_h?: (number | null);
    run_id?: (string | null);
    ok?: boolean;
  };
};

export type ComputerEventList = {
  data?: Array<ComputerEvent>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type ComputerSettings = {
  hosted_driver_enabled?: boolean;
  approval_mode?: "owner" | "api_key";
  per_key_concurrency?: number;
  browser_toolset?: boolean;
};

export type ToolsetCall = {
  toolset: "computer" | "browser";
  /**
   * A contract member, e.g. screenshot, left_click, type, navigate.
   */
  member: string;
  input?: Record<string, unknown>;
  run_id?: string;
  turn_id?: string;
  call_index?: number;
  model_frame?: {
    width?: number;
    height?: number;
  };
  coordinate_space?: "pixels" | "normalized_1000";
  approval_grant?: string;
  browser_session_id?: string;
};

export type ToolsetResult = {
  is_error: boolean;
  content: Array<{
    type?: "text" | "image";
    text?: string;
    media_type?: string;
    /**
     * Base64 image.
     */
    data?: string;
  }>;
  browser_state?: Record<string, unknown>;
  screen?: {
    width?: number;
    height?: number;
    scale?: number;
    frame_width?: number;
    frame_height?: number;
  };
  /**
   * Machine-readable code on failures: action_failed, unimplemented, not_executed, driver_failed.
   */
  error?: string;
};

export type ToolsetApprovalRequired = {
  /**
   * The usual error object, code approval_required.
   */
  error?: Record<string, unknown>;
  approval?: {
    id?: string;
    action_hash?: string;
    member?: string;
    toolset?: string;
    risk?: string;
    confirmation_class?: string;
    approve_url?: string;
    approver?: "owner" | "api_key";
  };
  result?: ToolsetResult;
};

export type ListAccountsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  /**
   * Only the account with this external_ref.
   */
  external_ref?: string;
};

export type ListAccountsResponse = (ListEnvelope & {
  data?: Array<Account>;
});

export type CreateAccountRequest = {
  name: string;
  /**
   * Your own id for this customer. Unique within the project.
   */
  external_ref?: string;
  /**
   * Free-form JSON object, at most 8 KB.
   */
  metadata?: Record<string, unknown>;
};

export type CreateAccountResponse = Account;

export type GetAccountResponse = Account;

export type UpdateAccountRequest = {
  name?: string;
  external_ref?: (string | null);
  metadata?: Record<string, unknown>;
};

export type UpdateAccountResponse = Account;

export type DeleteAccountResponse = {
  id: string;
  object: "account";
  deleted: true;
};

export type ListAgentsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  /**
   * Only this account's agents.
   */
  account_id?: string;
};

export type ListAgentsResponse = (ListEnvelope & {
  data?: Array<Agent>;
});

export type CreateAgentRequest = {
  /**
   * Required unless the key is bound to an account (then that account).
   */
  account_id?: string;
  name: string;
  instructions?: string;
  /**
   * First thing the agent says. Must say it is an AI. Default "Hi, this is {name}, an AI assistant. How can I help?"
   */
  greeting?: string;
  /**
   * "allternit" (default, routed by Allternit) or "provider/model".
   */
  model?: string;
  voice?: StockVoice;
  tools?: Array<AgentTool>;
  autonomy?: Autonomy;
  transfer_targets?: Array<string>;
  business_hours?: Record<string, unknown>;
  metadata?: Record<string, unknown>;
};

export type CreateAgentResponse = Agent;

export type GetAgentResponse = Agent;

export type UpdateAgentRequest = {
  name?: string;
  instructions?: string;
  greeting?: string;
  model?: string;
  voice?: StockVoice;
  tools?: Array<AgentTool>;
  autonomy?: Autonomy;
  transfer_targets?: Array<string>;
  business_hours?: (Record<string, unknown> | null);
  metadata?: Record<string, unknown>;
};

export type UpdateAgentResponse = Agent;

export type DeleteAgentResponse = {
  id: string;
  object: "agent";
  deleted: true;
};

export type ListKnowledgeFilesQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListKnowledgeFilesResponse = (ListEnvelope & {
  data?: Array<KnowledgeFile>;
});

export type UploadKnowledgeFileRequest = {
  name: string;
  content_type?: "text/plain" | "text/markdown" | "text/csv" | "text/html" | "application/json";
  content?: string;
  content_base64?: string;
};

export type UploadKnowledgeFileResponse = KnowledgeFile;

export type DeleteKnowledgeFileResponse = {
  id: string;
  object: "knowledge_file";
  deleted: true;
};

export type ListConversationsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListConversationsResponse = (ListEnvelope & {
  data?: Array<Conversation>;
});

export type CreateConversationRequest = {
  metadata?: Record<string, unknown>;
};

export type CreateConversationResponse = Conversation;

export type GetConversationResponse = Conversation;

export type SendConversationMessageRequest = {
  content: string;
  stream?: boolean;
};

export type SendConversationMessageResponse = ConversationMessage;

export type ListModelKeysResponse = {
  object?: "list";
  data?: Array<ModelKey>;
  has_more?: boolean;
};

export type PutModelKeyRequest = {
  api_key: string;
};

export type PutModelKeyResponse = ModelKey;

export type DeleteModelKeyResponse = {
  provider?: string;
  object?: "model_key";
  deleted?: true;
};

export type GetSpendCapResponse = SpendCap;

export type PutSpendCapRequest = {
  spend_cap_cents: number;
};

export type PutSpendCapResponse = SpendCap;

export type GetUsageQuery = {
  group_by?: "meter" | "key" | "account";
  /**
   * RFC 3339 timestamp or YYYY-MM-DD. Defaults to 30 days before `to`.
   */
  from?: string;
  /**
   * RFC 3339 timestamp or YYYY-MM-DD. Defaults to now.
   */
  to?: string;
  /**
   * Only this account's usage.
   */
  account_id?: string;
};

export type GetUsageResponse = {
  object: "usage";
  group_by: string;
  from: string;
  to: string;
  data: Array<UsageRow>;
  has_more: boolean;
  next_cursor: (string | null);
};

export type ListAvailableNumbersQuery = {
  country?: string;
  area_code?: string;
  locality?: string;
  type?: "local" | "toll_free";
  limit?: number;
};

export type ListAvailableNumbersResponse = {
  data?: Array<{
    e164?: string;
    type?: string;
    locality?: (string | null);
    monthlyCost?: (string | null);
  }>;
};

export type ListNumbersQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  account_id?: string;
};

export type ListNumbersResponse = PhoneNumberList;

export type CreateNumberRequest = {
  account_id: string;
  e164?: string;
  type?: "local" | "toll_free";
};

export type CreateNumberResponse = PhoneNumber;

export type GetNumberResponse = PhoneNumber;

export type UpdateNumberRequest = {
  agent_id?: (string | null);
};

export type UpdateNumberResponse = PhoneNumber;

export type ReleaseNumberResponse = {
  id?: string;
  object?: "phone_number";
  deleted?: true;
};

export type GetNumberRegistrationResponse = Record<string, unknown>;

export type SubmitNumberRegistrationRequest = Record<string, unknown>;

export type SubmitNumberRegistrationResponse = Record<string, unknown>;

export type VerifyRegistrationCodeRequest = {
  pin?: string;
};

export type VerifyRegistrationCodeResponse = Record<string, unknown>;

export type RecordConsentRequest = {
  e164: string;
  /**
   * How they agreed: a form, a call, an existing customer record.
   */
  source: string;
  evidence?: string;
};

export type RecordConsentResponse = {
  number_id?: string;
  e164?: string;
  recorded?: boolean;
  opted_out?: boolean;
};

export type SimulateInboundRequest = {
  from: string;
  body: string;
};

export type SimulateInboundResponse = {
  number_id?: string;
  handled?: string;
};

export type ListMessagesQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  number_id?: string;
  account_id?: string;
  direction?: "inbound" | "outbound";
};

export type ListMessagesResponse = MessageList;

export type SendMessageRequest = {
  number_id: string;
  to: string;
  body: string;
};

export type SendMessageResponse = Message;

export type GetMessageResponse = Message;

export type ListWebhooksQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListWebhooksResponse = {
  data?: Array<WebhookEndpoint>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type CreateWebhookRequest = {
  url: string;
  events: Array<"*" | "approval.requested" | "message.received" | "message.status" | "registration.updated" | "inbox.item.created" | "call.started" | "call.ended" | "call.transcript.ready" | "usage.threshold">;
  description?: string;
};

export type CreateWebhookResponse = (WebhookEndpoint & {
  secret?: string;
});

export type GetWebhookResponse = WebhookEndpoint;

export type DeleteWebhookResponse = {
  id?: string;
  object?: string;
  deleted?: boolean;
};

export type TestWebhookResponse = {
  event_id?: string;
  webhook_id?: string;
  queued?: boolean;
};

export type ListWebhookDeliveriesQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListWebhookDeliveriesResponse = Record<string, unknown>;

export type RedeliverWebhookResponse = {
  id?: string;
  queued?: boolean;
};

export type ListCallsResponse = {
  data?: Array<Call>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type CreateCallRequest = {
  agent_id: string;
  from_number_id: string;
  to: string;
  purpose: string;
  record?: boolean;
};

export type CreateCallResponse = Call;

export type GetCallResponse = Call;

export type EndCallResponse = Call;

export type TransferCallRequest = {
  to?: string;
  mode?: "warm" | "cold";
};

export type TransferCallResponse = Call;

export type GetCallTranscriptResponse = Record<string, unknown>;

export type GetCallRecordingResponse = Record<string, unknown>;

export type SimulateCallTurnRequest = {
  text: string;
};

export type SimulateCallTurnResponse = Record<string, unknown>;

export type SimulateInboundCallRequest = {
  from: string;
};

export type SimulateInboundCallResponse = Call;

export type CreateRealtimeSessionRequest = {
  agent_id: string;
};

export type CreateRealtimeSessionResponse = Record<string, unknown>;

export type ListChannelsResponse = Record<string, unknown>;

export type ConnectChannelRequest = Record<string, unknown>;

export type ConnectChannelResponse = Record<string, unknown>;

export type GetChannelResponse = Record<string, unknown>;

export type DeleteChannelResponse = Record<string, unknown>;

export type ListPeopleResponse = Record<string, unknown>;

export type GetPersonResponse = Record<string, unknown>;

export type UpdatePersonRequest = Record<string, unknown>;

export type UpdatePersonResponse = Record<string, unknown>;

export type ListInboxResponse = Record<string, unknown>;

export type ResolveInboxItemResponse = Record<string, unknown>;

export type ListApprovalsResponse = Record<string, unknown>;

export type ApproveApprovalResponse = Record<string, unknown>;

export type DenyApprovalResponse = Record<string, unknown>;

export type ListMemoryResponse = Record<string, unknown>;

export type AddMemoryRequest = Record<string, unknown>;

export type AddMemoryResponse = Record<string, unknown>;

export type DeleteMemoryResponse = Record<string, unknown>;

export type GetAutonomyResponse = Record<string, unknown>;

export type PutAutonomyRequest = Record<string, unknown>;

export type PutAutonomyResponse = Record<string, unknown>;

export type ListComputersQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListComputersResponse = ComputerList;

export type CreateComputerRequest = {
  name?: string;
  account_id?: string;
  metadata?: Record<string, unknown>;
};

export type CreateComputerResponse = Computer;

export type GetComputerResponse = Computer;

export type DeleteComputerResponse = Computer;

export type StartComputerResponse = Computer;

export type StopComputerResponse = Computer;

export type CallComputerToolsetRequest = ToolsetCall;

export type CallComputerToolsetResponse = ToolsetResult;

export type GetComputerToolsetSchemaQuery = {
  toolset?: "computer" | "browser";
};

export type GetComputerToolsetSchemaResponse = Record<string, unknown>;

export type ListComputerEventsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListComputerEventsResponse = ComputerEventList;

export type ApproveComputerActionResponse = {
  approval_id?: string;
  approved?: boolean;
  approval_grant?: string;
};

export type GetComputerSettingsResponse = ComputerSettings;

export type UpdateComputerSettingsRequest = {
  approval_mode?: "owner" | "api_key";
  per_key_concurrency?: number;
  browser_toolset?: boolean;
};

export type UpdateComputerSettingsResponse = ComputerSettings;
