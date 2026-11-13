// Generated from cmd/allternit-api/openapi/selfhost-v1.yaml by scripts/platform-sdk/generate.py. Do not edit.
/* eslint-disable */

export type Error = {
  error: {
    type: "invalid_request_error" | "authentication_error" | "permission_error" | "not_found_error" | "conflict_error" | "payload_too_large_error" | "unprocessable_error" | "api_error";
    message: string;
    /**
     * Stable code. Known codes:
     * `model_not_mapped` (422, the model has no entry in the model map; no outbound request was made),
     * `host_not_allowed` (403, a host is not on the allowlist),
     * `lease_stale` (409, the lease generation is old),
     * `result_too_large` (413, tool output over 100 KiB),
     * `no_worker` (a session status reason, not an HTTP error: no worker is attached for the environment),
     * `session_busy` (409), `agent_in_use` (409), `environment_in_use` (409),
     * `last_admin_key` (409), `timeout` (408, internal route), `no_worker` is also 504 on the internal route, `unauthorized` (401), `forbidden` (403), `not_found` (404),
     * `session_archived` (409, the session takes no new messages), `setup_already_done` (409),
     * `first_key_must_be_admin` (400), `tool_call_conflict` (409, the tool_call_id belongs to another session),
     * `invalid_body`, `invalid_query`, `invalid_field`, `invalid_limit`, `invalid_cursor`, `invalid_host` (400),
     * `not_implemented` (501, the agent loop is not available), `store_unavailable` (503),
     * `oidc_unavailable` (503, the identity provider is not reachable), `internal_error` (500).
     */
    code: string;
  };
};

export type ListEnvelope = {
  data: Array<unknown>;
  has_more: boolean;
  /**
   * Pass as `after` to get the next page. Null on the last page.
   */
  next_cursor: (string | null);
};

export type Deleted = {
  id: string;
  deleted: true;
};

export type ServerInfo = {
  version: string;
  provider: "anthropic" | "bedrock" | "vertex" | "foundry" | "gateway";
  features: {
    web_tools: boolean;
    prometheus: boolean;
    oidc: boolean;
    vault_secrets: boolean;
    /**
     * False when the server runs on SQLite or memory.
     */
    postgres: boolean;
  };
  auth_modes: Array<"api_key" | "oidc">;
  public_url?: (string | null);
  max_tool_result_bytes?: number;
};

export type McpServer = {
  name: string;
  /**
   * HTTPS only. The host must match `mcp.allowed_hostnames` exactly (no subdomain matching).
   */
  url: string;
  /**
   * Header values may be a secret reference such as `secret://name`. They are never returned in plain text.
   */
  headers?: Record<string, string>;
};

export type AgentTool = {
  /**
   * `builtin` is a tool of the agent loop (bash, file read and write, edit, search). `custom` is a tool your worker or client runs.
   */
  type: "builtin" | "custom";
  name: string;
  description?: string;
  /**
   * JSON Schema of the input. Required for `custom` tools.
   */
  input_schema?: Record<string, unknown>;
};

export type Agent = {
  id: string;
  object: "agent";
  name: string;
  /**
   * The system prompt.
   */
  instructions: string;
  /**
   * A model id. Under a gateway provider it must be a key of the model map.
   */
  model: string;
  tools: Array<AgentTool>;
  mcp_servers: Array<McpServer>;
  /**
   * Skill names to load for the agent.
   */
  skills: Array<string>;
  metadata: Record<string, unknown>;
  status: "ready";
  created_at: string;
  updated_at: string;
};

export type CreateAgentRequest = {
  name: string;
  instructions?: string;
  model: string;
  tools?: Array<AgentTool>;
  mcp_servers?: Array<McpServer>;
  skills?: Array<string>;
  metadata?: Record<string, unknown>;
};

export type UpdateAgentRequest = {
  name?: string;
  instructions?: string;
  model?: string;
  tools?: Array<AgentTool>;
  mcp_servers?: Array<McpServer>;
  skills?: Array<string>;
  metadata?: Record<string, unknown>;
};

/**
 * `none` blocks all outbound traffic. `limited` allows only `allowed_hosts`. `full` allows any host.
 */
export type NetworkMode = "none" | "limited" | "full";

export type GitPolicy = {
  /**
   * Git hosts the worker may clone from and push to. Exact match.
   */
  allowed_hostnames?: Array<string>;
  /**
   * Ref patterns a push may update, for example `refs/heads/agent/*`. A push to any other ref is refused.
   */
  allowed_push_refs?: Array<string>;
};

export type McpPolicy = {
  /**
   * MCP hosts for sessions in this environment. Exact match, HTTPS only. Can only narrow the server-wide list.
   */
  allowed_hostnames?: Array<string>;
};

export type EnvironmentImage = {
  /**
   * Container image for the sandbox, when the worker runs sandboxes as containers.
   */
  image?: string;
  packages?: {
    apt?: Array<string>;
    pip?: Array<string>;
    npm?: Array<string>;
  };
};

export type Environment = {
  id: string;
  object: "environment";
  name: string;
  network: NetworkMode;
  allowed_hosts: Array<string>;
  git: GitPolicy;
  mcp: McpPolicy;
  image: (string | null);
  packages: {
    apt?: Array<string>;
    pip?: Array<string>;
    npm?: Array<string>;
  };
  /**
   * Workers that claimed or heartbeat in the last 60 seconds.
   */
  worker_count?: number;
  /**
   * Work items waiting to be claimed.
   */
  queue_depth?: number;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
};

export type CreateEnvironmentRequest = {
  name: string;
  network?: NetworkMode;
  /**
   * Used when `network` is `limited`. Exact hostnames, optionally with a port.
   */
  allowed_hosts?: Array<string>;
  git?: GitPolicy;
  mcp?: McpPolicy;
  image?: string;
  packages?: {
    apt?: Array<string>;
    pip?: Array<string>;
    npm?: Array<string>;
  };
  metadata?: Record<string, unknown>;
};

export type UpdateEnvironmentRequest = {
  name?: string;
  network?: NetworkMode;
  allowed_hosts?: Array<string>;
  git?: GitPolicy;
  mcp?: McpPolicy;
  image?: (string | null);
  packages?: {
    apt?: Array<string>;
    pip?: Array<string>;
    npm?: Array<string>;
  };
  metadata?: Record<string, unknown>;
};

/**
 * The environment's rules, as they were when the work was claimed. The worker must enforce them.
 */
export type EnvironmentPolicy = {
  network: NetworkMode;
  allowed_hosts: Array<string>;
  git: GitPolicy;
  mcp: McpPolicy;
  image?: (string | null);
  packages?: {
    apt?: Array<string>;
    pip?: Array<string>;
    npm?: Array<string>;
  };
};

export type SessionStatus = "idle" | "running" | "waiting_for_worker" | "interrupted" | "error" | "archived";

export type Session = {
  id: string;
  object: "session";
  agent_id: string;
  environment_id: string;
  title?: (string | null);
  status: SessionStatus;
  /**
   * A code that says why, for example `no_worker` while status is `waiting_for_worker`.
   */
  status_reason?: (string | null);
  /**
   * The `seq` of the newest event.
   */
  last_seq?: number;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
  archived_at?: (string | null);
};

export type EventType = "message" | "tool_call" | "tool_result" | "status" | "error";

export type Event = {
  id: string;
  object: "event";
  session_id: string;
  /**
   * Rises by one for each event in the session.
   */
  seq: number;
  type: EventType;
  /**
   * Depends on `type`.
   */
  data: (MessageData | ToolCallData | ToolResultData | StatusData | ErrorData);
  created_at: string;
};

export type MessageData = {
  role: "user" | "assistant";
  content: string;
};

export type ToolCallData = {
  tool_call_id: string;
  name: string;
  input: Record<string, unknown>;
};

export type ToolResultData = {
  tool_call_id: string;
  output: string;
  is_error: boolean;
};

export type StatusData = {
  status: SessionStatus;
  reason?: (string | null);
};

export type ErrorData = {
  code: string;
  message: string;
};

export type WorkClaim = {
  work_id: string;
  session_id: string;
  /**
   * Secret for this session only. Send it as `X-Session-Secret` on tool call routes. Valid until the lease ends. Never log it.
   */
  session_secret: string;
  lease_generation: number;
  lease_expires_at: string;
  environment: EnvironmentPolicy;
};

export type Lease = {
  work_id: string;
  lease_generation: number;
  lease_expires_at: (string | null);
  status: "claimed" | "ready" | "completed";
};

export type ToolCall = {
  /**
   * Stable. Use it to avoid running a call twice.
   */
  tool_call_id: string;
  name: string;
  input: Record<string, unknown>;
};

export type Role = "admin" | "developer" | "viewer";

export type ApiKey = {
  id: string;
  object: "api_key";
  name: string;
  role: Role;
  /**
   * The first characters of the key, for recognition.
   */
  prefix: string;
  created_at: string;
  expires_at?: (string | null);
  last_used_at?: (string | null);
  revoked_at?: (string | null);
};

export type ApiKeyWithSecret = (ApiKey & {
  /**
   * The `ask_...` key. Shown once.
   */
  secret: string;
});

export type EnvironmentKey = {
  id: string;
  object: "environment_key";
  environment_id: string;
  name: (string | null);
  prefix: string;
  created_at: string;
  expires_at?: (string | null);
  last_used_at?: (string | null);
  revoked_at?: (string | null);
};

export type EnvironmentKeyWithSecret = (EnvironmentKey & {
  /**
   * The `aek_...` key. Shown once.
   */
  secret: string;
});

export type GetHealthLiveResponse = {
  status: "ok";
};

export type GetHealthReadyResponse = {
  status: "ok";
  store: "postgres" | "sqlite" | "memory";
  /**
   * `ready` when the gizzi child answers. `disabled` for a server built without an agent loop.
   */
  agent_loop?: "ready" | "disabled";
};

export type GetServerInfoResponse = ServerInfo;

export type ListAgentsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListAgentsResponse = (ListEnvelope & {
  data?: Array<Agent>;
});

export type CreateAgentResponse = Agent;

export type GetAgentResponse = Agent;

export type UpdateAgentResponse = Agent;

export type DeleteAgentResponse = Deleted;

export type ListEnvironmentsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListEnvironmentsResponse = (ListEnvelope & {
  data?: Array<Environment>;
});

export type CreateEnvironmentResponse = Environment;

export type GetEnvironmentResponse = Environment;

export type UpdateEnvironmentResponse = Environment;

export type DeleteEnvironmentResponse = Deleted;

export type ListEnvironmentWorkersResponse = {
  data: Array<{
    worker_id: string;
    last_seen_at: string;
    online: boolean;
    current_work_id?: (string | null);
    hostname?: (string | null);
    version?: (string | null);
  }>;
};

export type ListEnvironmentKeysQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListEnvironmentKeysResponse = (ListEnvelope & {
  data?: Array<EnvironmentKey>;
});

export type CreateEnvironmentKeyRequest = {
  name?: string;
  expires_at?: (string | null);
};

export type CreateEnvironmentKeyResponse = EnvironmentKeyWithSecret;

export type RevokeEnvironmentKeyResponse = Deleted;

export type ListSessionsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  agent_id?: string;
  environment_id?: string;
  status?: SessionStatus;
  include_archived?: boolean;
};

export type ListSessionsResponse = (ListEnvelope & {
  data?: Array<Session>;
});

export type CreateSessionRequest = {
  agent_id: string;
  environment_id: string;
  title?: string;
  metadata?: Record<string, unknown>;
};

export type CreateSessionResponse = Session;

export type GetSessionResponse = Session;

export type ArchiveSessionResponse = Session;

export type SendSessionMessageRequest = {
  content: string;
  metadata?: Record<string, unknown>;
};

export type SendSessionMessageResponse = Event;

export type InterruptSessionResponse = Session;

export type ListSessionEventsQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
  /**
   * Only events with a larger `seq`. Use it instead of `after` to resume from a known point.
   */
  after_seq?: number;
  /**
   * Comma-separated event types to keep.
   */
  types?: string;
};

export type ListSessionEventsResponse = (ListEnvelope & {
  data?: Array<Event>;
});

export type StreamSessionEventsQuery = {
  after_seq?: number;
};

export type StreamSessionEventsResponse = unknown;

export type ClaimWorkRequest = {
  /**
   * A stable name for this worker process. Shown in status.
   */
  worker_id: string;
  /**
   * How long to wait for work before answering 204.
   */
  block_ms?: number;
  /**
   * Also claim items whose live lease was last renewed at least this long ago. Expired leases are claimable without it. Values of 60000 or more add nothing.
   */
  reclaim_older_than_ms?: number;
};

export type ClaimWorkResponse = WorkClaim | undefined;

export type HeartbeatWorkRequest = {
  /**
   * The generation from the claim. A lower number than the server holds is stale.
   */
  lease_generation: number;
};

export type HeartbeatWorkResponse = Lease;

export type RenewWorkRequest = {
  /**
   * The generation from the claim. A lower number than the server holds is stale.
   */
  lease_generation: number;
};

export type RenewWorkResponse = Lease;

export type CompleteWorkRequest = {
  lease_generation: number;
  outcome?: "success" | "failed" | "cancelled";
  error_message?: string;
};

export type CompleteWorkResponse = Lease;

export type ReleaseWorkRequest = {
  /**
   * The generation from the claim. A lower number than the server holds is stale.
   */
  lease_generation: number;
};

export type ReleaseWorkResponse = Lease;

export type NextToolCallRequest = {
  block_ms?: number;
};

export type NextToolCallResponse = ToolCall | undefined;

export type SubmitToolResultRequest = {
  /**
   * Tool output as text. At most 102400 bytes (100 KiB).
   */
  output: string;
  is_error?: boolean;
};

export type SubmitToolResultResponse = {
  tool_call_id: string;
  status: "completed" | "errored";
};

export type ListApiKeysQuery = {
  /**
   * Page size, 1 to 100 (default 20).
   */
  limit?: number;
  /**
   * Opaque cursor from `next_cursor` of the previous page.
   */
  after?: string;
};

export type ListApiKeysResponse = (ListEnvelope & {
  data?: Array<ApiKey>;
});

export type CreateApiKeyRequest = {
  name: string;
  role: Role;
  expires_at?: (string | null);
};

export type CreateApiKeyResponse = ApiKeyWithSecret;

export type RevokeApiKeyResponse = Deleted;
