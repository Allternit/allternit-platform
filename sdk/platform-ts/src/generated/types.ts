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
    /**
     * Where to fix it. Set on 402 payment_method_required: the console billing page for the project.
     */
    url?: string;
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
   * true once the project has a card on file and a plan in good standing. Without one, billable requests answer 402 payment_method_required.
   */
  payment_method?: boolean;
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
  /**
   * Null for a conversation made without an agent (used with `model` responses).
   */
  agent_id: (string | null);
  account_id: (string | null);
  metadata: Record<string, unknown>;
  /**
   * Number of items; list them with listConversationItems.
   */
  item_count?: number;
  created_at: string;
  /**
   * The latest 200 user and assistant messages, oldest first: on GET /v1/conversations/{id} and the legacy alias routes.
   */
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

export type ProjectTracing = {
  object: "project.tracing";
  project_id: string;
  endpoint: string;
  /**
   * Names of the stored headers; values are never returned.
   */
  header_names: Array<string>;
  service_name: string;
  enabled: boolean;
  last_export_at?: (string | null);
  /**
   * Why the last export failed; null after a success.
   */
  last_error?: (string | null);
  created_at: string;
  updated_at: string;
};

export type UsageRow = {
  /**
   * The meter, key id or account id, per `group_by`. Null for usage with no account.
   */
  group: (string | null);
  meter: "voice_min_allternit" | "voice_min_byok" | "agent_month" | "tokens_in" | "tokens_out" | "number_local_month" | "number_tollfree_month" | "sms_segment" | "mms" | "registration_passthrough_cents" | "recording_min_month" | "computer_minute" | "computer_action" | "mailbox_month" | "identity_month";
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
  type: "approval.requested" | "approval.resolved" | "approval.expired" | "message.received" | "message.status" | "registration.updated" | "inbox.item.created" | "call.started" | "call.ended" | "call.transcript.ready" | "usage.threshold" | "phone.step" | "phone.run.completed" | "phone.fleet_run.completed" | "phone.approval.requested" | "phone.approval.resolved" | "mail.received" | "webhook.test";
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
    /**
     * Who is driving. agent_id is the request's agent, else the computer's bot. Null when neither is known.
     */
    actor?: ({
      agent_id?: (string | null);
      name?: (string | null);
    } | null);
    /**
     * The step text the call sent (masked), or null.
     */
    step?: (string | null);
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
  /**
   * Off-by-default members (file_upload, read_console, read_network, javascript_exec) to allow for this call.
   */
  enable?: Array<string>;
  /**
   * The bot or agent driving the call, shown on the agent cursor. Display only. When left out, the computer's own bot is used.
   */
  agent_id?: string;
  /**
   * Its display name for the cursor badge. Trimmed and cut to 60 characters.
   */
  agent_name?: string;
  /**
   * A few words saying what this action does, shown to people watching. Trimmed, cut to 120 characters, and masked like other logged text.
   */
  step?: string;
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

export type Mailbox = {
  id: string;
  object: "mailbox";
  account_id: string;
  agent_id?: (string | null);
  address: string;
  display_name?: (string | null);
  created_at: string;
};

export type MailboxList = {
  data?: Array<Mailbox>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type MailMessage = {
  id: string;
  object: "mail_message";
  mailbox_id: string;
  from: string;
  to: string;
  subject?: (string | null);
  snippet?: (string | null);
  /**
   * One-time codes found
   */
  codes: Array<string>;
  /**
   * Sign-in and confirm links found.
   */
  links: Array<string>;
  is_verification: boolean;
  received_at: string;
};

export type MailMessageList = {
  data?: Array<MailMessage>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type Identity = {
  id: string;
  object: "identity";
  account_id: string;
  agent_id?: (string | null);
  number?: (PhoneNumber | (unknown | null));
  mailbox?: (Mailbox | (unknown | null));
  phone_serial?: (string | null);
  bundle: {
    name?: string;
    meter?: "identity_month";
    monthly_price_cents?: number;
    includes?: {
      numbers?: number;
      mailboxes?: number;
      linked_phone_slots?: number;
      phone_minutes?: number;
    };
  };
  created_at: string;
};

export type IdentityList = {
  data?: Array<Identity>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type Phone = {
  object?: "phone";
  serial?: string;
  name?: (string | null);
  model?: (string | null);
  /**
   * Connected to the Desktop right now.
   */
  online?: boolean;
  desktop_online?: boolean;
  last_seen_at?: (string | null);
  /**
   * {enabled, botName} when a bot answers this phone's calls.
   */
  call_bridge?: (Record<string, unknown> | null);
};

export type PhoneList = {
  data?: Array<Phone>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type PhoneSkillList = {
  data?: Array<{
    object?: "phone.skill";
    name?: string;
    inputs?: Array<string>;
    [key: string]: unknown;
  }>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type PhoneStep = {
  object?: "phone.step";
  id?: string;
  run_id?: string;
  serial?: string;
  at?: string;
  tool?: string;
  label?: string;
  /**
   * Redacted: typed text, numbers and bodies are {redacted: true, len}.
   */
  args?: Record<string, unknown>;
  ok?: boolean;
  error?: (string | null);
  duration_ms?: number;
  /**
   * bot, skill, task, routine or api.
   */
  source?: (string | null);
  /**
   * A frame is kept on the Desktop.
   */
  snapshot?: boolean;
};

export type PhoneHistory = {
  object?: "phone.history";
  serial?: string;
  steps?: Array<PhoneStep>;
  runs?: Array<{
    object?: "phone.run";
    run_id?: string;
    serial?: string;
    started_at?: string;
    ended_at?: string;
    steps?: number;
    failed?: number;
    source?: (string | null);
  }>;
  retention_days?: number;
  max_steps?: number;
};

export type PhoneLiveView = {
  object?: "phone.live_view";
  /**
   * The live view id; it is the credential in both URLs.
   */
  id?: string;
  serial?: string;
  view_only?: true;
  /**
   * Seconds.
   */
  expires_in?: number;
  expires_at?: string;
  viewer_url?: string;
  stream_url?: string;
};

export type PhoneFleetRun = {
  object?: "phone.fleet_run";
  id?: string;
  kind?: "task" | "skill";
  goal?: (string | null);
  skill?: (string | null);
  status?: "running" | "completed";
  created_at?: string;
  completed_at?: (string | null);
  counts?: {
    runs?: number;
    running?: number;
    succeeded?: number;
    failed?: number;
    not_started?: number;
  };
  runs?: Array<{
    object?: "phone.run";
    run_id?: string;
    serial?: (string | null);
    /**
     * running, waiting_approval (a step waits on the owner's approval), or the end status (done, failed, max_steps, timeout, succeeded, diverged, denied, approval_timeout, phone_locked).
     */
    status?: string;
    ok?: (boolean | null);
    started_at?: string;
    ended_at?: (string | null);
  }>;
  not_started?: Array<{
    serial?: string;
    code?: string;
    message?: string;
  }>;
};

export type PhoneRunStarted = {
  object?: "phone.run";
  run_id?: string;
  serial?: (string | null);
  skill?: (string | null);
  status?: "running";
};

export type PhoneToolResult = {
  object?: "phone.tool_result";
  ok: boolean;
  run_id?: string;
  tool?: string;
  /**
   * On ok: false, e.g. approval_denied, approval_timeout, rate_limited, not_found, accessibility_off.
   */
  error?: string;
  message?: string;
  [key: string]: unknown;
};

/**
 * A held phone action (409). API keys can't approve: the project owner approves in the Allternit app, then the call is repeated with hold_id.
 */
export type PhoneApprovalRequired = {
  /**
   * The usual error object, type conflict_error, code approval_required.
   */
  error: Record<string, unknown>;
  /**
   * The hold; send it back as hold_id once approved.
   */
  approval_id?: string;
  /**
   * The owner's "Needs you" item.
   */
  inbox_id?: string;
  /**
   * The hold expires unanswered after this.
   */
  expires_at?: string;
};

export type DecisionQuestion = {
  name: string;
  type: "predicate" | "choice" | "score";
  instructions: string;
  /**
   * Required for `choice`.
   */
  choices?: Array<{
    value: string;
    description?: string;
  }>;
  /**
   * Required for `score`, ordered low to high.
   */
  levels?: Array<{
    label: string;
    description?: string;
  }>;
};

/**
 * The question list, or the single-question Agency body (`context`, `options`, `kind`, `question`).
 */
export type DecisionRequest = {
  /**
   * Text, or items with `input_text` or `input_json` content parts. Text up to 4 KiB. Image parts are not supported yet (`400 image_not_supported`).
   */
  input?: (string | Array<{
    role: string;
    content: Array<Record<string, unknown>>;
  }>);
  instructions?: string;
  /**
   * 1 to 16 questions. An object keyed by question name is also accepted.
   */
  questions?: (Array<DecisionQuestion> | Record<string, unknown>);
  /**
   * When omitted: 5000 + 15 * input_bytes + 3000 * questions, clamped to 10000-120000.
   */
  latency_budget_ms?: number;
  allow_abstain?: boolean;
  metadata?: Record<string, unknown>;
  /**
   * Agency shape: a string or {text}, text up to 4 KiB. Images are not supported yet.
   */
  context?: unknown;
  /**
   * Agency shape: 2 to 255 {id, text}.
   */
  options?: Array<Record<string, unknown>>;
  /**
   * Agency shape.
   */
  kind?: string;
  /**
   * Agency shape.
   */
  question?: string;
};

export type DecisionAnswer = {
  name: string;
  type: "predicate" | "choice" | "score" | "refusal";
  /**
   * predicate: P(yes).
   */
  probability?: number;
  /**
   * choice: the best value.
   */
  choice?: string;
  /**
   * score: probability-weighted level index scaled to 0..1.
   */
  score?: number;
  probabilities?: Array<{
    /**
     * A choice value (string) or a level index (integer).
     */
    value?: unknown;
    label?: string;
    probability?: number;
  }>;
  confidence?: number;
  /**
   * The backend that answered this question (e.g. local, open, oracle).
   */
  backend?: string;
  /**
   * refusal only: abstained (below the confidence threshold with allow_abstain), unavailable (no backend could answer), budget_exhausted (latency_ms budget ran out).
   */
  reason?: "abstained" | "unavailable" | "budget_exhausted";
};

export type DecisionOutcome = {
  status: "success" | "failure" | "error" | "skipped";
  label?: string;
  detail?: string;
};

export type Decision = {
  id: string;
  object: "decision";
  created_at: string;
  answers: Array<DecisionAnswer>;
  /**
   * The backend that answered the most questions (each answer has its own `backend`).
   */
  backend: string;
  latency_ms: number;
  usage: {
    /**
     * Answered (non-refusal) questions; this is what is billed.
     */
    questions?: number;
    input_tokens?: number;
  };
  outcomes?: Record<string, DecisionOutcome>;
  metadata?: Record<string, unknown>;
};

export type DecisionList = (ListEnvelope & {
  data?: Array<Decision>;
});

export type ContentPart = {
  type: "input_text" | "output_text" | "input_image";
  text?: string;
  image_url?: string;
};

/**
 * An item to give the model or append to a conversation.
 */
export type ItemInput = {
  type: "message" | "function_call_output";
  role?: "user" | "assistant" | "system" | "developer";
  content?: (string | Array<ContentPart>);
  call_id?: string;
  output?: string;
};

export type Item = {
  /**
   * `item_…`
   */
  id: string;
  object: "item";
  type: "message" | "function_call" | "function_call_output" | "tool_call" | "approval_request";
  conversation_id?: (string | null);
  response_id?: (string | null);
  status?: "in_progress" | "completed" | "failed" | "incomplete";
  role?: ("user" | "assistant" | "system" | "developer" | null);
  /**
   * message items.
   */
  content?: Array<ContentPart>;
  /**
   * function_call, function_call_output.
   */
  call_id?: string;
  /**
   * function_call, tool_call.
   */
  name?: string;
  /**
   * function_call: JSON text.
   */
  arguments?: string;
  /**
   * function_call_output (string) or tool_call (any).
   */
  output?: unknown;
  /**
   * tool_call.
   */
  input?: unknown;
  error?: (string | null);
  /**
   * approval_request.
   */
  approval_id?: string;
  /**
   * approval_request.
   */
  proposed_action?: Record<string, unknown>;
  created_at: string;
};

export type ItemList = (ListEnvelope & {
  data?: Array<Item>;
});

export type ResponseRequest = {
  /**
   * Exactly one of `model` or `agent`.
   */
  model?: string;
  /**
   * An agent id.
   */
  agent?: string;
  input: (string | Array<ItemInput>);
  instructions?: string;
  tools?: Array<{
    type: "function";
    name: string;
    description?: string;
    /**
     * JSON Schema.
     */
    parameters?: Record<string, unknown>;
  }>;
  tool_choice?: unknown;
  stream?: boolean;
  store?: boolean;
  /**
   * A `conv_…` id. Not with `previous_response_id`.
   */
  conversation?: string;
  previous_response_id?: string;
  metadata?: Record<string, unknown>;
  max_output_tokens?: number;
  temperature?: number;
};

export type Response = {
  /**
   * `resp_…`
   */
  id: string;
  object: "response";
  status: "in_progress" | "completed" | "failed" | "incomplete";
  model?: (string | null);
  agent_id?: (string | null);
  conversation_id?: (string | null);
  previous_response_id?: (string | null);
  output: Array<Item>;
  /**
   * Why the response is `incomplete`; null otherwise.
   */
  incomplete_details?: ({
    reason?: "max_output_tokens";
  } | null);
  /**
   * Reserved; always empty in this release. Approvals are their own resource: subscribe to `approval.requested` / `approval.resolved`; after resolution send a new response in the same conversation.
   */
  required_actions?: Array<{
    type: "approval";
    approval_id: string;
  }>;
  usage?: {
    input_tokens?: number;
    output_tokens?: number;
  };
  error?: (Record<string, unknown> | null);
  metadata?: Record<string, unknown>;
  created_at: string;
  completed_at?: (string | null);
};

export type ModelMessageRequest = {
  model: string;
  /**
   * A string or text blocks.
   */
  system?: (string | Array<Record<string, unknown>>);
  messages: Array<{
    role: "user" | "assistant";
    /**
     * A string or blocks of type text, image, tool_use or tool_result.
     */
    content: (string | Array<Record<string, unknown>>);
  }>;
  tools?: Array<{
    name: string;
    description?: string;
    input_schema: Record<string, unknown>;
  }>;
  tool_choice?: Record<string, unknown>;
  max_tokens: number;
  temperature?: number;
  stream?: boolean;
  metadata?: Record<string, unknown>;
  thread?: {
    type: "create" | "continue";
    /**
     * Required for `continue`: the `mdl_…` id of the thread's latest message.
     */
    previous_message_id?: string;
  };
};

export type ModelMessage = {
  /**
   * `mdl_…`
   */
  id: string;
  type: "message";
  role: "assistant";
  content: Array<Record<string, unknown>>;
  model: string;
  stop_reason: "end_turn" | "tool_use" | "max_tokens";
  usage: {
    input_tokens?: number;
    output_tokens?: number;
  };
};

export type Text = {
  /**
   * Keeps the stored `msg_` prefix.
   */
  id: string;
  object: "text";
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

export type TextList = {
  data?: Array<Text>;
  has_more?: boolean;
  next_cursor?: (string | null);
};

export type Approval = {
  /**
   * `apr_…`
   */
  id: string;
  object: "approval";
  source: "twin" | "computer";
  /**
   * The id in the source system.
   */
  source_ref: string;
  account_id?: (string | null);
  conversation_id?: (string | null);
  response_id?: (string | null);
  kind: string;
  proposed_action: Record<string, unknown>;
  status: "pending" | "approved" | "denied" | "expired" | "consumed";
  /**
   * Twin: the runtime's deadline when it sends one, else null (a twin approval with no deadline never expires). Computer: the executor's deadline, else 300 seconds after the request.
   */
  expires_at?: (string | null);
  created_at: string;
  resolved_at?: (string | null);
  resolved_by?: (string | null);
};

export type ApprovalList = (ListEnvelope & {
  data?: Array<Approval>;
});

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

export type ListAgentConversationsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListAgentConversationsResponse = (ListEnvelope & {
  data?: Array<Conversation>;
});

export type CreateAgentConversationRequest = {
  metadata?: Record<string, unknown>;
};

export type CreateAgentConversationResponse = Conversation;

export type RunAgentAgUiRequest = {
  threadId?: string;
  runId?: string;
  parentRunId?: string;
  /**
   * AG-UI messages; the last `user` message (string content, or its `text` parts) is the turn's input.
   */
  messages: Array<{
    id?: string;
    role: "developer" | "system" | "assistant" | "user" | "tool" | "activity" | "reasoning";
    content?: unknown;
  }>;
  tools?: Array<Record<string, unknown>>;
  context?: Array<Record<string, unknown>>;
  state?: unknown;
  forwardedProps?: unknown;
};

export type RunAgentAgUiResponse = unknown;

export type GetConversationResponse = Conversation;

export type UpdateConversationRequest = {
  metadata: Record<string, unknown>;
};

export type UpdateConversationResponse = Conversation;

export type DeleteConversationResponse = {
  id: string;
  object: "conversation.deleted";
  deleted: boolean;
};

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

export type GetProjectTracingResponse = ProjectTracing;

export type PutProjectTracingRequest = {
  endpoint: string;
  headers?: Record<string, string>;
  service_name?: string;
  enabled?: boolean;
};

export type PutProjectTracingResponse = ProjectTracing;

export type DeleteProjectTracingResponse = {
  object?: "project.tracing";
  project_id?: string;
  deleted?: true;
};

export type TestProjectTracingResponse = {
  object: "project.tracing.test";
  ok: boolean;
  /**
   * The collector's HTTP status
   */
  status?: (number | null);
  message: string;
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

export type ListTextsQuery = {
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

export type ListTextsResponse = TextList;

export type CreateTextRequest = {
  number_id: string;
  to: string;
  body: string;
};

export type CreateTextResponse = Text;

export type GetTextResponse = Text;

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
  events: Array<"*" | "approval.requested" | "approval.resolved" | "approval.expired" | "message.received" | "message.status" | "registration.updated" | "inbox.item.created" | "call.started" | "call.ended" | "call.transcript.ready" | "usage.threshold" | "phone.step" | "phone.run.completed" | "phone.fleet_run.completed" | "phone.approval.requested" | "phone.approval.resolved" | "mail.received">;
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

export type ListAccountApprovalsResponse = Record<string, unknown>;

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

export type ListPhonesResponse = PhoneList;

export type ListPhoneSkillsResponse = PhoneSkillList;

export type RunPhoneSkillRequest = {
  /**
   * The phone; omit for the only connected one.
   */
  serial?: string;
  /**
   * A value for each of the skill's inputs.
   */
  inputs?: Record<string, string>;
  bot_id?: string;
  /**
   * provider/model
   */
  brain?: string;
};

export type RunPhoneSkillResponse = PhoneRunStarted;

export type GetPhoneHistoryQuery = {
  /**
   * Only this run's steps.
   */
  run_id?: string;
  /**
   * At most this many newest steps.
   */
  limit?: number;
};

export type GetPhoneHistoryResponse = PhoneHistory;

export type CallPhoneToolRequest = {
  /**
   * e.g. phone.tap (the phone. prefix is optional).
   */
  name: string;
  arguments?: Record<string, unknown>;
  /**
   * Group several calls into one run in the history (default: a new api- run).
   */
  run_id?: string;
  /**
   * On the retry of a held call: the approval_id the owner approved. Sent to the phone as `_hold` in arguments.
   */
  hold_id?: string;
};

export type CallPhoneToolResponse = PhoneToolResult;

export type RunPhoneTaskRequest = {
  goal: string;
  max_steps?: number;
  bot_id?: string;
  /**
   * provider/model
   */
  brain?: string;
};

export type RunPhoneTaskResponse = PhoneRunStarted;

export type GetPhoneLiveViewResponse = PhoneLiveView;

export type StartPhoneFleetRunRequest = {
  serials: Array<string>;
  /**
   * A task for every phone. Give this or skill.
   */
  goal?: string;
  /**
   * A saved phone skill to replay on every phone.
   */
  skill?: string;
  /**
   * Skill inputs.
   */
  inputs?: Record<string, string>;
  max_steps?: number;
  bot_id?: string;
  /**
   * provider/model
   */
  brain?: string;
};

export type StartPhoneFleetRunResponse = PhoneFleetRun;

export type GetPhoneFleetRunResponse = PhoneFleetRun;

export type ListMailboxesQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  account_id?: string;
  agent_id?: string;
};

export type ListMailboxesResponse = MailboxList;

export type CreateMailboxRequest = {
  account_id?: string;
  agent_id?: string;
  /**
   * The part before @. Lower-cased; letters, digits and dashes. Default: the agent's name.
   */
  local_part?: string;
  display_name?: string;
};

export type CreateMailboxResponse = Mailbox;

export type GetMailboxResponse = Mailbox;

export type DeleteMailboxResponse = {
  id?: string;
  object?: "mailbox";
  deleted?: boolean;
};

export type ListMailMessagesQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  /**
   * Only messages that look like verification email.
   */
  verification?: boolean;
};

export type ListMailMessagesResponse = MailMessageList;

export type GetMailMessageResponse = (MailMessage & {
  text?: (string | null);
  html?: (string | null);
});

export type ReadMailOtpRequest = {
  wait_seconds?: number;
  since?: string;
};

export type ReadMailOtpResponse = {
  object: "mail_otp";
  found: boolean;
  code?: (string | null);
  codes?: Array<string>;
  links?: Array<string>;
  message?: (MailMessage | (unknown | null));
};

export type SendMailRequest = {
  to: string;
  subject: string;
  text?: string;
  html?: string;
};

export type SendMailResponse = {
  object?: "mail_send";
  mailbox_id?: string;
  from?: string;
  to?: string;
  message_id?: (string | null);
  /**
   * queued or pending_approval
   */
  status?: string;
};

export type ListIdentitiesQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  account_id?: string;
  agent_id?: string;
};

export type ListIdentitiesResponse = IdentityList;

export type CreateIdentityRequest = {
  account_id?: string;
  agent_id?: string;
  area_code?: string;
  number_type?: "local" | "toll_free";
  phone_serial?: string;
  local_part?: string;
  display_name?: string;
};

export type CreateIdentityResponse = Identity;

export type GetIdentityResponse = Identity;

export type DeleteIdentityResponse = {
  id?: string;
  object?: "identity";
  deleted?: boolean;
};

export type ListDecisionsQuery = {
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

export type ListDecisionsResponse = DecisionList;

export type CreateDecisionRequest = DecisionRequest;

export type CreateDecisionResponse = Decision;

export type GetDecisionResponse = Decision;

export type UpdateDecisionRequest = {
  /**
   * Keyed by question name.
   */
  outcomes: Record<string, DecisionOutcome>;
};

export type UpdateDecisionResponse = Decision;

export type CreateResponseRequest = ResponseRequest;

export type CreateResponseResponse = Response;

export type GetResponseResponse = Response;

export type ListResponseInputItemsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListResponseInputItemsResponse = ItemList;

export type CreateMessageRequest = ModelMessageRequest;

export type CreateMessageResponse = ModelMessage;

export type ListConversationsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  agent_id?: string;
  account_id?: string;
};

export type ListConversationsResponse = (ListEnvelope & {
  data?: Array<Conversation>;
});

export type CreateConversationRequest = {
  agent_id?: string;
  account_id?: string;
  metadata?: Record<string, unknown>;
  items?: Array<ItemInput>;
};

export type CreateConversationResponse = Conversation;

export type ListConversationItemsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  order?: "asc" | "desc";
};

export type ListConversationItemsResponse = ItemList;

export type CreateConversationItemsRequest = {
  items: Array<ItemInput>;
};

export type CreateConversationItemsResponse = ItemList;

export type GetConversationItemResponse = Item;

export type ListApprovalsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  status?: "pending" | "approved" | "denied" | "expired" | "consumed";
  account_id?: string;
  source?: "twin" | "computer";
};

export type ListApprovalsResponse = ApprovalList;

export type GetApprovalResponse = Approval;

export type ResolveApprovalRequest = {
  decision: "approve" | "deny";
  reason?: string;
};

export type ResolveApprovalResponse = Approval;
