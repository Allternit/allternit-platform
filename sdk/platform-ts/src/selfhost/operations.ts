// Generated from cmd/allternit-api/openapi/selfhost-v1.yaml by scripts/platform-sdk/generate.py. Do not edit.
/* eslint-disable */
import type { RequestOptions, Transport, Page, SSEEvent } from "../core.ts";
import type * as T from "./types.ts";

export class ServerResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * Server info
   *
   * Version, inference provider, enabled features and accepted auth modes. Safe to call before you sign in, so a client can show the right sign-in screen. Reveals no secrets.
   *
   * `GET /v1/server/info`
   */
  getInfo(options?: RequestOptions): Promise<T.GetServerInfoResponse> {
    return this._client.request<T.GetServerInfoResponse>({ method: "GET", path: `/v1/server/info`, options });
  }
}

export class AgentsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List agents
   *
   * Agents, oldest first.
   *
   * `GET /v1/agents`
   */
  list(query?: T.ListAgentsQuery, options?: RequestOptions): Promise<T.ListAgentsResponse> {
    return this._client.request<T.ListAgentsResponse>({ method: "GET", path: `/v1/agents`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListAgentsQuery, options?: RequestOptions): AsyncIterable<T.Agent> {
    return this._client.paginate<T.Agent>((after) => this.list({ ...(query ?? {}), after } as T.ListAgentsQuery, options) as unknown as Promise<Page<T.Agent>>);
  }

  /**
   * Create an agent
   *
   * An agent is a model, a system prompt and a set of tools. If the server
   * uses a gateway provider, `model` must be in the settings `model_map`
   * (else `422 model_not_mapped`). Needs role `developer` or `admin`.
   *
   * `POST /v1/agents`
   */
  create(body: T.CreateAgentRequest, options?: RequestOptions): Promise<T.CreateAgentResponse> {
    return this._client.request<T.CreateAgentResponse>({ method: "POST", path: `/v1/agents`, body: body, options });
  }

  /**
   * Retrieve an agent
   *
   * `GET /v1/agents/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetAgentResponse> {
    return this._client.request<T.GetAgentResponse>({ method: "GET", path: `/v1/agents/${encodeURIComponent(id)}`, options });
  }

  /**
   * Update an agent
   *
   * Absent fields are unchanged. Arrays and `metadata` replace the old value. Running sessions keep the agent version they started with.
   *
   * `PATCH /v1/agents/{id}`
   */
  update(id: string, body: T.UpdateAgentRequest, options?: RequestOptions): Promise<T.UpdateAgentResponse> {
    return this._client.request<T.UpdateAgentResponse>({ method: "PATCH", path: `/v1/agents/${encodeURIComponent(id)}`, body: body, options });
  }

  /**
   * Delete an agent
   *
   * Fails with 409 `agent_in_use` while the agent has a non-archived session.
   *
   * `DELETE /v1/agents/{id}`
   */
  delete(id: string, options?: RequestOptions): Promise<T.DeleteAgentResponse> {
    return this._client.request<T.DeleteAgentResponse>({ method: "DELETE", path: `/v1/agents/${encodeURIComponent(id)}`, options });
  }
}

export class EnvironmentsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List environments
   *
   * An environment is the place where an agent's tools run, with its own network rules. Oldest first.
   *
   * `GET /v1/environments`
   */
  list(query?: T.ListEnvironmentsQuery, options?: RequestOptions): Promise<T.ListEnvironmentsResponse> {
    return this._client.request<T.ListEnvironmentsResponse>({ method: "GET", path: `/v1/environments`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListEnvironmentsQuery, options?: RequestOptions): AsyncIterable<T.Environment> {
    return this._client.paginate<T.Environment>((after) => this.list({ ...(query ?? {}), after } as T.ListEnvironmentsQuery, options) as unknown as Promise<Page<T.Environment>>);
  }

  /**
   * Create an environment
   *
   * Network defaults to `none`. Workers enforce the policy on every
   * outbound connection. An environment's `git` and `mcp` lists can only
   * narrow the server-wide lists in the settings file, never widen them.
   *
   * `POST /v1/environments`
   */
  create(body: T.CreateEnvironmentRequest, options?: RequestOptions): Promise<T.CreateEnvironmentResponse> {
    return this._client.request<T.CreateEnvironmentResponse>({ method: "POST", path: `/v1/environments`, body: body, options });
  }

  /**
   * Retrieve an environment
   *
   * `GET /v1/environments/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetEnvironmentResponse> {
    return this._client.request<T.GetEnvironmentResponse>({ method: "GET", path: `/v1/environments/${encodeURIComponent(id)}`, options });
  }

  /**
   * Update an environment
   *
   * Policy changes reach a worker on its next claim. Work already claimed keeps the policy it was claimed with.
   *
   * `PATCH /v1/environments/{id}`
   */
  update(id: string, body: T.UpdateEnvironmentRequest, options?: RequestOptions): Promise<T.UpdateEnvironmentResponse> {
    return this._client.request<T.UpdateEnvironmentResponse>({ method: "PATCH", path: `/v1/environments/${encodeURIComponent(id)}`, body: body, options });
  }

  /**
   * Delete an environment
   *
   * Fails with 409 `environment_in_use` while a non-archived session uses it. Revokes the environment's keys.
   *
   * `DELETE /v1/environments/{id}`
   */
  delete(id: string, options?: RequestOptions): Promise<T.DeleteEnvironmentResponse> {
    return this._client.request<T.DeleteEnvironmentResponse>({ method: "DELETE", path: `/v1/environments/${encodeURIComponent(id)}`, options });
  }

  /**
   * List workers seen for an environment
   *
   * Workers that claimed, heartbeat or renewed in the last 10 minutes, newest first. `online` is true when the worker was seen in the last 60 seconds.
   *
   * `GET /v1/environments/{id}/workers`
   */
  listWorkers(id: string, options?: RequestOptions): Promise<T.ListEnvironmentWorkersResponse> {
    return this._client.request<T.ListEnvironmentWorkersResponse>({ method: "GET", path: `/v1/environments/${encodeURIComponent(id)}/workers`, options });
  }
}

export class EnvironmentKeysResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List environment keys
   *
   * Keys for this environment. The secret is never shown again. Needs role `admin`.
   *
   * `GET /v1/environments/{id}/keys`
   */
  list(id: string, query?: T.ListEnvironmentKeysQuery, options?: RequestOptions): Promise<T.ListEnvironmentKeysResponse> {
    return this._client.request<T.ListEnvironmentKeysResponse>({ method: "GET", path: `/v1/environments/${encodeURIComponent(id)}/keys`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(id: string, query?: T.ListEnvironmentKeysQuery, options?: RequestOptions): AsyncIterable<T.EnvironmentKey> {
    return this._client.paginate<T.EnvironmentKey>((after) => this.list(id, { ...(query ?? {}), after } as T.ListEnvironmentKeysQuery, options) as unknown as Promise<Page<T.EnvironmentKey>>);
  }

  /**
   * Create an environment key
   *
   * Returns the `aek_...` secret once. Give it to the workers of this environment. Needs role `admin`.
   *
   * `POST /v1/environments/{id}/keys`
   */
  create(id: string, body: T.CreateEnvironmentKeyRequest, options?: RequestOptions): Promise<T.CreateEnvironmentKeyResponse> {
    return this._client.request<T.CreateEnvironmentKeyResponse>({ method: "POST", path: `/v1/environments/${encodeURIComponent(id)}/keys`, body: body, options });
  }

  /**
   * Revoke an environment key
   *
   * Takes effect at once. Workers using the key get 401 on their next call. Needs role `admin`.
   *
   * `DELETE /v1/environments/{id}/keys/{key_id}`
   */
  revoke(id: string, keyId: string, options?: RequestOptions): Promise<T.RevokeEnvironmentKeyResponse> {
    return this._client.request<T.RevokeEnvironmentKeyResponse>({ method: "DELETE", path: `/v1/environments/${encodeURIComponent(id)}/keys/${encodeURIComponent(keyId)}`, options });
  }
}

export class SessionsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List sessions
   *
   * Newest first. Archived sessions are hidden unless `include_archived` is true.
   *
   * `GET /v1/sessions`
   */
  list(query?: T.ListSessionsQuery, options?: RequestOptions): Promise<T.ListSessionsResponse> {
    return this._client.request<T.ListSessionsResponse>({ method: "GET", path: `/v1/sessions`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListSessionsQuery, options?: RequestOptions): AsyncIterable<T.Session> {
    return this._client.paginate<T.Session>((after) => this.list({ ...(query ?? {}), after } as T.ListSessionsQuery, options) as unknown as Promise<Page<T.Session>>);
  }

  /**
   * Create a session
   *
   * A session pairs one agent with one environment. Creating it does not start a turn. Send a message to do that. Returns 422 `model_not_mapped` when the agent's model is not in the model map, with no outbound request made.
   *
   * `POST /v1/sessions`
   */
  create(body: T.CreateSessionRequest, options?: RequestOptions): Promise<T.CreateSessionResponse> {
    return this._client.request<T.CreateSessionResponse>({ method: "POST", path: `/v1/sessions`, body: body, options });
  }

  /**
   * Retrieve a session
   *
   * `GET /v1/sessions/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetSessionResponse> {
    return this._client.request<T.GetSessionResponse>({ method: "GET", path: `/v1/sessions/${encodeURIComponent(id)}`, options });
  }

  /**
   * Archive a session
   *
   * Stops the session and hides it from the default list. Events stay readable. Archiving cannot be undone.
   *
   * `POST /v1/sessions/{id}/archive`
   */
  archive(id: string, options?: RequestOptions): Promise<T.ArchiveSessionResponse> {
    return this._client.request<T.ArchiveSessionResponse>({ method: "POST", path: `/v1/sessions/${encodeURIComponent(id)}/archive`, options });
  }

  /**
   * Send a user message
   *
   * Adds a user turn and starts the agent loop. Returns at once with the
   * event that was stored. Follow progress on the events stream. A model-only
   * turn needs no worker. A turn that calls a tool waits until a worker
   * for the environment is attached; the session shows `waiting_for_worker`.
   * Returns 409 `session_busy` if a turn is already running.
   *
   * `POST /v1/sessions/{id}/messages`
   */
  sendMessage(id: string, body: T.SendSessionMessageRequest, options?: RequestOptions): Promise<T.SendSessionMessageResponse> {
    return this._client.request<T.SendSessionMessageResponse>({ method: "POST", path: `/v1/sessions/${encodeURIComponent(id)}/messages`, body: body, options });
  }

  /**
   * Interrupt the running turn
   *
   * Stops the current turn. Pending tool calls are cancelled. Does nothing if the session is idle.
   *
   * `POST /v1/sessions/{id}/interrupt`
   */
  interrupt(id: string, options?: RequestOptions): Promise<T.InterruptSessionResponse> {
    return this._client.request<T.InterruptSessionResponse>({ method: "POST", path: `/v1/sessions/${encodeURIComponent(id)}/interrupt`, options });
  }
}

export class EventsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List session events
   *
   * The session log, oldest first, in pages. Events are never edited or removed. `seq` rises by one for each event in a session.
   *
   * `GET /v1/sessions/{id}/events`
   */
  listSessionEvents(id: string, query?: T.ListSessionEventsQuery, options?: RequestOptions): Promise<T.ListSessionEventsResponse> {
    return this._client.request<T.ListSessionEventsResponse>({ method: "GET", path: `/v1/sessions/${encodeURIComponent(id)}/events`, query: query, options });
  }

  /**
   * Every item of `listSessionEvents`, fetching pages as you iterate.
   */
  listSessionEventsAll(id: string, query?: T.ListSessionEventsQuery, options?: RequestOptions): AsyncIterable<T.Event> {
    return this._client.paginate<T.Event>((after) => this.listSessionEvents(id, { ...(query ?? {}), after } as T.ListSessionEventsQuery, options) as unknown as Promise<Page<T.Event>>);
  }

  /**
   * Stream session events (SSE)
   *
   * Server-sent events. Each frame has `id: <seq>`, `event: <type>` and
   * `data: <Event JSON>`. Event types: `message`, `tool_call`,
   * `tool_result`, `status`, `error`. A `: keepalive` comment goes out every
   * 15 seconds. To resume after a dropped connection, send `Last-Event-ID`
   * (or `after_seq`); the server replays everything after it, then goes live.
   *
   * `GET /v1/sessions/{id}/events/stream`
   */

  streamSessionEvents(id: string, query?: T.StreamSessionEventsQuery, options?: RequestOptions): AsyncIterable<SSEEvent> {
    return this._client.streamRequest({ method: "GET", path: `/v1/sessions/${encodeURIComponent(id)}/events/stream`, query: query, body: {}, options });
  }
}

export class WorkResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * Claim a work item
   *
   * Worker side. Claims the oldest ready work item for this environment and
   * starts a 60 second lease. Waits up to `block_ms` for work to appear.
   * Returns 204 when there is nothing. If `reclaim_older_than_ms` is set, the
   * worker may also take an item whose lease ran out that long ago (a worker
   * that died). Each claim raises the item's `lease_generation`. Auth: environment key of this environment.
   *
   * `POST /v1/environments/{id}/work/claim`
   */
  claim(id: string, body: T.ClaimWorkRequest, options?: RequestOptions): Promise<T.ClaimWorkResponse> {
    return this._client.request<T.ClaimWorkResponse>({ method: "POST", path: `/v1/environments/${encodeURIComponent(id)}/work/claim`, body: body, options });
  }

  /**
   * Heartbeat a work item
   *
   * Tells the server the worker is alive, without moving the lease. Send it between renewals. 409 `lease_stale` if the generation is old.
   *
   * `POST /v1/work/{work_id}/heartbeat`
   */
  heartbeat(workId: string, body: T.HeartbeatWorkRequest, options?: RequestOptions): Promise<T.HeartbeatWorkResponse> {
    return this._client.request<T.HeartbeatWorkResponse>({ method: "POST", path: `/v1/work/${encodeURIComponent(workId)}/heartbeat`, body: body, options });
  }

  /**
   * Renew a lease
   *
   * Moves `lease_expires_at` to 60 seconds from now. Renew every 20 seconds. 409 `lease_stale` if the generation is old, which means another worker owns the item now. Stop at once.
   *
   * `POST /v1/work/{work_id}/renew`
   */
  renew(workId: string, body: T.RenewWorkRequest, options?: RequestOptions): Promise<T.RenewWorkResponse> {
    return this._client.request<T.RenewWorkResponse>({ method: "POST", path: `/v1/work/${encodeURIComponent(workId)}/renew`, body: body, options });
  }

  /**
   * Complete a work item
   *
   * Marks the item done and ends the lease. 409 `lease_stale` if the generation is old. Completing twice with the current generation returns 200 both times.
   *
   * `POST /v1/work/{work_id}/complete`
   */
  complete(workId: string, body: T.CompleteWorkRequest, options?: RequestOptions): Promise<T.CompleteWorkResponse> {
    return this._client.request<T.CompleteWorkResponse>({ method: "POST", path: `/v1/work/${encodeURIComponent(workId)}/complete`, body: body, options });
  }

  /**
   * Hand a work item back
   *
   * The worker is shutting down (or was started with `--drain` and is done) and gives the item back. The lease ends, the item goes back to ready, and another worker can claim it without waiting for the lease to run out. Tool calls that were running are retried (at-least-once). 409 `lease_stale` if the generation is old.
   *
   * `POST /v1/work/{work_id}/release`
   */
  release(workId: string, body: T.ReleaseWorkRequest, options?: RequestOptions): Promise<T.ReleaseWorkResponse> {
    return this._client.request<T.ReleaseWorkResponse>({ method: "POST", path: `/v1/work/${encodeURIComponent(workId)}/release`, body: body, options });
  }
}

export class ToolCallsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * Get the next tool call
   *
   * Worker side. Long-polls up to `block_ms` for the next tool call of the
   * session. Returns 204 when none arrived. A tool call that was handed out but has no result yet
   * is handed out again after the lease changes hands (at-least-once), with the
   * same `tool_call_id`. Run it once per id. Auth: the session secret.
   *
   * `POST /v1/work/{work_id}/tool_calls/next`
   */
  next(workId: string, body: T.NextToolCallRequest, options?: RequestOptions): Promise<T.NextToolCallResponse> {
    return this._client.request<T.NextToolCallResponse>({ method: "POST", path: `/v1/work/${encodeURIComponent(workId)}/tool_calls/next`, body: body, options });
  }

  /**
   * Submit a tool result
   *
   * Posts the output of a tool call. Sending the same id again stores nothing new and returns 200 (idempotent on `tool_call_id`). Output over 100 KiB is refused with 413 `result_too_large`; the worker should truncate and retry with `is_error` as needed. Auth is the session secret of the session that owns the call.
   *
   * `POST /v1/tool_calls/{id}/result`
   */
  submitToolResult(id: string, body: T.SubmitToolResultRequest, options?: RequestOptions): Promise<T.SubmitToolResultResponse> {
    return this._client.request<T.SubmitToolResultResponse>({ method: "POST", path: `/v1/tool_calls/${encodeURIComponent(id)}/result`, body: body, options });
  }
}

export class APIKeysResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List server API keys
   *
   * Prefix and metadata only. The secret is never shown again. Needs role `admin`.
   *
   * `GET /v1/api_keys`
   */
  list(query?: T.ListApiKeysQuery, options?: RequestOptions): Promise<T.ListApiKeysResponse> {
    return this._client.request<T.ListApiKeysResponse>({ method: "GET", path: `/v1/api_keys`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListApiKeysQuery, options?: RequestOptions): AsyncIterable<T.ApiKey> {
    return this._client.paginate<T.ApiKey>((after) => this.list({ ...(query ?? {}), after } as T.ListApiKeysQuery, options) as unknown as Promise<Page<T.ApiKey>>);
  }

  /**
   * Create a server API key
   *
   * Returns the `ask_...` secret once. The server stores only a hash. The first key is minted by `allternit-agents-server admin setup`, not by this route. Needs role `admin`.
   *
   * `POST /v1/api_keys`
   */
  create(body: T.CreateApiKeyRequest, options?: RequestOptions): Promise<T.CreateApiKeyResponse> {
    return this._client.request<T.CreateApiKeyResponse>({ method: "POST", path: `/v1/api_keys`, body: body, options });
  }

  /**
   * Revoke a server API key
   *
   * Takes effect at once. The last admin key cannot be revoked (409 `last_admin_key`). Needs role `admin`.
   *
   * `DELETE /v1/api_keys/{id}`
   */
  revoke(id: string, options?: RequestOptions): Promise<T.RevokeApiKeyResponse> {
    return this._client.request<T.RevokeApiKeyResponse>({ method: "DELETE", path: `/v1/api_keys/${encodeURIComponent(id)}`, options });
  }
}

/** Every API area as a client property. `SelfHostClient` extends this. */
export class GeneratedResources {
  server: ServerResource;
  agents: AgentsResource;
  environments: EnvironmentsResource;
  environmentKeys: EnvironmentKeysResource;
  sessions: SessionsResource;
  events: EventsResource;
  work: WorkResource;
  toolCalls: ToolCallsResource;
  apiKeys: APIKeysResource;
  constructor(client: Transport) {
    this.server = new ServerResource(client);
    this.agents = new AgentsResource(client);
    this.environments = new EnvironmentsResource(client);
    this.environmentKeys = new EnvironmentKeysResource(client);
    this.sessions = new SessionsResource(client);
    this.events = new EventsResource(client);
    this.work = new WorkResource(client);
    this.toolCalls = new ToolCallsResource(client);
    this.apiKeys = new APIKeysResource(client);
  }
}
