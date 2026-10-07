// Generated from cmd/allternit-cloud-api/openapi/platform-v1.yaml by scripts/platform-sdk/generate.py. Do not edit.
/* eslint-disable */
import type { RequestOptions, Transport, Page, SSEEvent } from "../core.ts";
import type * as T from "./types.ts";

export class AccountsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List accounts
   *
   * Accounts in the project, oldest first. A key bound to an account sees only that account. Requires a resource scope.
   *
   * `GET /v1/accounts`
   */
  list(query?: T.ListAccountsQuery, options?: RequestOptions): Promise<T.ListAccountsResponse> {
    return this._client.request<T.ListAccountsResponse>({ method: "GET", path: `/v1/accounts`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListAccountsQuery, options?: RequestOptions): AsyncIterable<T.Account> {
    return this._client.paginate<T.Account>((after) => this.list({ ...(query ?? {}), after } as T.ListAccountsQuery, options) as unknown as Promise<Page<T.Account>>);
  }

  /**
   * Create an account
   *
   * One account per end-customer business. Not available to account-bound keys.
   *
   * `POST /v1/accounts`
   */
  create(body: T.CreateAccountRequest, options?: RequestOptions): Promise<T.CreateAccountResponse> {
    return this._client.request<T.CreateAccountResponse>({ method: "POST", path: `/v1/accounts`, body: body, options });
  }

  /**
   * Retrieve an account
   *
   * `GET /v1/accounts/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetAccountResponse> {
    return this._client.request<T.GetAccountResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}`, options });
  }

  /**
   * Update an account
   *
   * Absent fields are unchanged; `external_ref: null` clears it; `metadata` replaces the object.
   *
   * `PATCH /v1/accounts/{id}`
   */
  update(id: string, body: T.UpdateAccountRequest, options?: RequestOptions): Promise<T.UpdateAccountResponse> {
    return this._client.request<T.UpdateAccountResponse>({ method: "PATCH", path: `/v1/accounts/${encodeURIComponent(id)}`, body: body, options });
  }

  /**
   * Delete an account
   *
   * Soft delete. Also revokes the API keys bound to the account. Not available to account-bound keys.
   *
   * `DELETE /v1/accounts/{id}`
   */
  delete(id: string, options?: RequestOptions): Promise<T.DeleteAccountResponse> {
    return this._client.request<T.DeleteAccountResponse>({ method: "DELETE", path: `/v1/accounts/${encodeURIComponent(id)}`, options });
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
   * Agents in the project, oldest first. A key bound to an account sees only that account's agents. Requires the `agents` scope.
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
   * A hosted agent for one of your accounts. The greeting must say the caller is talking to an AI
   * (`greeting_missing_ai_disclosure` otherwise); voices are stock voices only; tools come from the
   * launch list. Sandbox projects can have 3 agents (`agent_limit_reached`).
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
   * Absent fields are unchanged; `business_hours: null` clears it; arrays and `metadata` replace the old value. The agent's account can't change.
   *
   * `PATCH /v1/agents/{id}`
   */
  update(id: string, body: T.UpdateAgentRequest, options?: RequestOptions): Promise<T.UpdateAgentResponse> {
    return this._client.request<T.UpdateAgentResponse>({ method: "PATCH", path: `/v1/agents/${encodeURIComponent(id)}`, body: body, options });
  }

  /**
   * Delete an agent
   *
   * `DELETE /v1/agents/{id}`
   */
  delete(id: string, options?: RequestOptions): Promise<T.DeleteAgentResponse> {
    return this._client.request<T.DeleteAgentResponse>({ method: "DELETE", path: `/v1/agents/${encodeURIComponent(id)}`, options });
  }

  /**
   * List an agent's knowledge files
   *
   * Oldest first. Requires the `agents` scope; a key bound to an account sees only its own account's agents.
   *
   * `GET /v1/agents/{id}/knowledge`
   */
  listKnowledgeFiles(id: string, query?: T.ListKnowledgeFilesQuery, options?: RequestOptions): Promise<T.ListKnowledgeFilesResponse> {
    return this._client.request<T.ListKnowledgeFilesResponse>({ method: "GET", path: `/v1/agents/${encodeURIComponent(id)}/knowledge`, query: query, options });
  }

  /**
   * Every item of `listKnowledgeFiles`, fetching pages as you iterate.
   */
  listKnowledgeFilesAll(id: string, query?: T.ListKnowledgeFilesQuery, options?: RequestOptions): AsyncIterable<T.KnowledgeFile> {
    return this._client.paginate<T.KnowledgeFile>((after) => this.listKnowledgeFiles(id, { ...(query ?? {}), after } as T.ListKnowledgeFilesQuery, options) as unknown as Promise<Page<T.KnowledgeFile>>);
  }

  /**
   * Upload a knowledge file
   *
   * Text files the agent searches with the knowledge_search tool. Types: text/plain, text/markdown, text/csv, text/html, application/json (UTF-8). At most 1 MB per file, 20 files and 5 MB in total per agent. Send the text as `content`, or the bytes as `content_base64`. `content_type` defaults from the name's extension.
   *
   * `POST /v1/agents/{id}/knowledge`
   */
  uploadKnowledgeFile(id: string, body: T.UploadKnowledgeFileRequest, options?: RequestOptions): Promise<T.UploadKnowledgeFileResponse> {
    return this._client.request<T.UploadKnowledgeFileResponse>({ method: "POST", path: `/v1/agents/${encodeURIComponent(id)}/knowledge`, body: body, options });
  }

  /**
   * Delete a knowledge file
   *
   * The agent stops finding it right away; the stored original is removed.
   *
   * `DELETE /v1/agents/{id}/knowledge/{file_id}`
   */
  deleteKnowledgeFile(id: string, fileId: string, options?: RequestOptions): Promise<T.DeleteKnowledgeFileResponse> {
    return this._client.request<T.DeleteKnowledgeFileResponse>({ method: "DELETE", path: `/v1/agents/${encodeURIComponent(id)}/knowledge/${encodeURIComponent(fileId)}`, options });
  }

  /**
   * List the project's model keys
   *
   * Masked; the key itself is never returned.
   *
   * `GET /v1/model_keys`
   */
  listModelKeys(options?: RequestOptions): Promise<T.ListModelKeysResponse> {
    return this._client.request<T.ListModelKeysResponse>({ method: "GET", path: `/v1/model_keys`, options });
  }

  /**
   * Set a model key
   *
   * The project's own key for a provider. Agents whose `model` is `provider/…` run on it. Not available to account-bound keys.
   *
   * `PUT /v1/model_keys/{provider}`
   */
  putModelKey(provider: string, body: T.PutModelKeyRequest, options?: RequestOptions): Promise<T.PutModelKeyResponse> {
    return this._client.request<T.PutModelKeyResponse>({ method: "PUT", path: `/v1/model_keys/${encodeURIComponent(provider)}`, body: body, options });
  }

  /**
   * Delete a model key
   *
   * `DELETE /v1/model_keys/{provider}`
   */
  deleteModelKey(provider: string, options?: RequestOptions): Promise<T.DeleteModelKeyResponse> {
    return this._client.request<T.DeleteModelKeyResponse>({ method: "DELETE", path: `/v1/model_keys/${encodeURIComponent(provider)}`, options });
  }
}

export class ConversationsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List an agent's conversations
   *
   * The agent's conversations, oldest first, without their messages (use getConversation for those). A key bound to an account sees only that account's agent. Requires the `agents` scope.
   *
   * `GET /v1/agents/{id}/conversations`
   */
  list(id: string, query?: T.ListConversationsQuery, options?: RequestOptions): Promise<T.ListConversationsResponse> {
    return this._client.request<T.ListConversationsResponse>({ method: "GET", path: `/v1/agents/${encodeURIComponent(id)}/conversations`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(id: string, query?: T.ListConversationsQuery, options?: RequestOptions): AsyncIterable<T.Conversation> {
    return this._client.paginate<T.Conversation>((after) => this.list(id, { ...(query ?? {}), after } as T.ListConversationsQuery, options) as unknown as Promise<Page<T.Conversation>>);
  }

  /**
   * Start a conversation
   *
   * Opens a conversation with the agent, in the agent's account. No message is sent yet.
   *
   * `POST /v1/agents/{id}/conversations`
   */
  create(id: string, body?: T.CreateConversationRequest, options?: RequestOptions): Promise<T.CreateConversationResponse> {
    return this._client.request<T.CreateConversationResponse>({ method: "POST", path: `/v1/agents/${encodeURIComponent(id)}/conversations`, body: body, options });
  }

  /**
   * Retrieve a conversation
   *
   * The conversation with its latest 200 messages, oldest first.
   *
   * `GET /v1/conversations/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetConversationResponse> {
    return this._client.request<T.GetConversationResponse>({ method: "GET", path: `/v1/conversations/${encodeURIComponent(id)}`, options });
  }

  /**
   * Send a message
   *
   * Sends a message and answers with the agent's reply. With `stream: true` the answer is
   * server-sent events: `message.delta` (`{"delta": "..."}`) while the agent writes, then
   * `message.completed` (the stored reply) or `error`. The first message starts the
   * project's hosted runtime: `503 runtime_starting` means retry in a few seconds.
   * One message at a time per conversation (`409 conversation_busy`).
   *
   * `POST /v1/conversations/{id}/messages`
   */
  sendMessage(id: string, body: Omit<T.SendConversationMessageRequest, "stream">, options?: RequestOptions): Promise<T.SendConversationMessageResponse> {
    return this._client.request<T.SendConversationMessageResponse>({ method: "POST", path: `/v1/conversations/${encodeURIComponent(id)}/messages`, body: body, options });
  }

  /**
   * `sendMessage` with `stream: true`: yields the raw server-sent events.
   */
  sendMessageStream(id: string, body: Omit<T.SendConversationMessageRequest, "stream">, options?: RequestOptions): AsyncIterable<SSEEvent> {
    return this._client.streamRequest({ method: "POST", path: `/v1/conversations/${encodeURIComponent(id)}/messages`, body: { ...(body ?? {}), stream: true }, options });
  }
}

export class UsageResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * Usage by meter, key or account
   *
   * Sums of recorded usage events in `[from, to)`. Requires the `usage` scope.
   *
   * `GET /v1/usage`
   */
  get(query?: T.GetUsageQuery, options?: RequestOptions): Promise<T.GetUsageResponse> {
    return this._client.request<T.GetUsageResponse>({ method: "GET", path: `/v1/usage`, query: query, options });
  }
}

export class NumbersResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * Search numbers to buy (live projects)
   *
   * `GET /v1/numbers/available`
   */
  listAvailableNumbers(query?: T.ListAvailableNumbersQuery, options?: RequestOptions): Promise<T.ListAvailableNumbersResponse> {
    return this._client.request<T.ListAvailableNumbersResponse>({ method: "GET", path: `/v1/numbers/available`, query: query, options });
  }

  /**
   * List numbers
   *
   * `GET /v1/numbers`
   */
  list(query?: T.ListNumbersQuery, options?: RequestOptions): Promise<T.ListNumbersResponse> {
    return this._client.request<T.ListNumbersResponse>({ method: "GET", path: `/v1/numbers`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListNumbersQuery, options?: RequestOptions): AsyncIterable<T.PhoneNumber> {
    return this._client.paginate<T.PhoneNumber>((after) => this.list({ ...(query ?? {}), after } as T.ListNumbersQuery, options) as unknown as Promise<Page<T.PhoneNumber>>);
  }

  /**
   * Get a number for an account
   *
   * Sandbox projects get a simulated number (no `e164`). Live projects buy `e164` from `/v1/numbers/available`; needs a paid plan.
   *
   * `POST /v1/numbers`
   */
  create(body: T.CreateNumberRequest, options?: RequestOptions): Promise<T.CreateNumberResponse> {
    return this._client.request<T.CreateNumberResponse>({ method: "POST", path: `/v1/numbers`, body: body, options });
  }

  /**
   * Retrieve a number
   *
   * `GET /v1/numbers/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetNumberResponse> {
    return this._client.request<T.GetNumberResponse>({ method: "GET", path: `/v1/numbers/${encodeURIComponent(id)}`, options });
  }

  /**
   * Release a number
   *
   * `DELETE /v1/numbers/{id}`
   */
  release(id: string, options?: RequestOptions): Promise<T.ReleaseNumberResponse> {
    return this._client.request<T.ReleaseNumberResponse>({ method: "DELETE", path: `/v1/numbers/${encodeURIComponent(id)}`, options });
  }

  /**
   * Carrier registration status
   *
   * `GET /v1/numbers/{id}/registration`
   */
  getRegistration(id: string, options?: RequestOptions): Promise<T.GetNumberRegistrationResponse> {
    return this._client.request<T.GetNumberRegistrationResponse>({ method: "GET", path: `/v1/numbers/${encodeURIComponent(id)}/registration`, options });
  }

  /**
   * Register the number for US texting (10DLC or toll-free)
   *
   * The end-customer business's own details. The campaign files itself once the brand is verified.
   *
   * `POST /v1/numbers/{id}/registration`
   */
  submitRegistration(id: string, body: T.SubmitNumberRegistrationRequest, options?: RequestOptions): Promise<T.SubmitNumberRegistrationResponse> {
    return this._client.request<T.SubmitNumberRegistrationResponse>({ method: "POST", path: `/v1/numbers/${encodeURIComponent(id)}/registration`, body: body, options });
  }

  /**
   * Sole proprietors — verify the texted code, or send a new one
   *
   * `{ "pin": "123456" }` checks the code the person received; an empty body sends a new code. Once verified, the campaign files itself.
   *
   * `POST /v1/numbers/{id}/registration/otp`
   */
  verifyRegistrationCode(id: string, body?: T.VerifyRegistrationCodeRequest, options?: RequestOptions): Promise<T.VerifyRegistrationCodeResponse> {
    return this._client.request<T.VerifyRegistrationCodeResponse>({ method: "POST", path: `/v1/numbers/${encodeURIComponent(id)}/registration/otp`, body: body, options });
  }

  /**
   * Record that a person agreed to be contacted
   *
   * `POST /v1/numbers/{id}/consent`
   */
  recordConsent(id: string, body: T.RecordConsentRequest, options?: RequestOptions): Promise<T.RecordConsentResponse> {
    return this._client.request<T.RecordConsentResponse>({ method: "POST", path: `/v1/numbers/${encodeURIComponent(id)}/consent`, body: body, options });
  }

  /**
   * Sandbox only — play a text arriving
   *
   * `POST /v1/numbers/{id}/simulate_inbound`
   */
  simulateInbound(id: string, body: T.SimulateInboundRequest, options?: RequestOptions): Promise<T.SimulateInboundResponse> {
    return this._client.request<T.SimulateInboundResponse>({ method: "POST", path: `/v1/numbers/${encodeURIComponent(id)}/simulate_inbound`, body: body, options });
  }
}

export class MessagingResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List texts
   *
   * `GET /v1/messages`
   */
  listMessages(query?: T.ListMessagesQuery, options?: RequestOptions): Promise<T.ListMessagesResponse> {
    return this._client.request<T.ListMessagesResponse>({ method: "GET", path: `/v1/messages`, query: query, options });
  }

  /**
   * Every item of `listMessages`, fetching pages as you iterate.
   */
  listMessagesAll(query?: T.ListMessagesQuery, options?: RequestOptions): AsyncIterable<T.Message> {
    return this._client.paginate<T.Message>((after) => this.listMessages({ ...(query ?? {}), after } as T.ListMessagesQuery, options) as unknown as Promise<Page<T.Message>>);
  }

  /**
   * Send a text
   *
   * Refused when texting isn't active on the number, the person sent STOP, or they never texted first and have no consent recorded.
   *
   * `POST /v1/messages`
   */
  sendMessage(body: T.SendMessageRequest, options?: RequestOptions): Promise<T.SendMessageResponse> {
    return this._client.request<T.SendMessageResponse>({ method: "POST", path: `/v1/messages`, body: body, options });
  }

  /**
   * Retrieve a text
   *
   * `GET /v1/messages/{id}`
   */
  getMessage(id: string, options?: RequestOptions): Promise<T.GetMessageResponse> {
    return this._client.request<T.GetMessageResponse>({ method: "GET", path: `/v1/messages/${encodeURIComponent(id)}`, options });
  }
}

export class WebhooksResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List webhook endpoints
   *
   * `GET /v1/webhooks`
   */
  list(query?: T.ListWebhooksQuery, options?: RequestOptions): Promise<T.ListWebhooksResponse> {
    return this._client.request<T.ListWebhooksResponse>({ method: "GET", path: `/v1/webhooks`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListWebhooksQuery, options?: RequestOptions): AsyncIterable<T.WebhookEndpoint> {
    return this._client.paginate<T.WebhookEndpoint>((after) => this.list({ ...(query ?? {}), after } as T.ListWebhooksQuery, options) as unknown as Promise<Page<T.WebhookEndpoint>>);
  }

  /**
   * Create a webhook endpoint
   *
   * `url` must be public https. The response carries `secret` once.
   *
   * `POST /v1/webhooks`
   */
  create(body: T.CreateWebhookRequest, options?: RequestOptions): Promise<T.CreateWebhookResponse> {
    return this._client.request<T.CreateWebhookResponse>({ method: "POST", path: `/v1/webhooks`, body: body, options });
  }

  /**
   * Retrieve a webhook endpoint
   *
   * `GET /v1/webhooks/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetWebhookResponse> {
    return this._client.request<T.GetWebhookResponse>({ method: "GET", path: `/v1/webhooks/${encodeURIComponent(id)}`, options });
  }

  /**
   * Delete a webhook endpoint
   *
   * `DELETE /v1/webhooks/{id}`
   */
  delete(id: string, options?: RequestOptions): Promise<T.DeleteWebhookResponse> {
    return this._client.request<T.DeleteWebhookResponse>({ method: "DELETE", path: `/v1/webhooks/${encodeURIComponent(id)}`, options });
  }

  /**
   * Send a webhook.test event to this endpoint
   *
   * `POST /v1/webhooks/{id}/test`
   */
  test(id: string, options?: RequestOptions): Promise<T.TestWebhookResponse> {
    return this._client.request<T.TestWebhookResponse>({ method: "POST", path: `/v1/webhooks/${encodeURIComponent(id)}/test`, options });
  }

  /**
   * Delivery log for an endpoint
   *
   * `GET /v1/webhooks/{id}/deliveries`
   */
  listDeliveries(id: string, query?: T.ListWebhookDeliveriesQuery, options?: RequestOptions): Promise<T.ListWebhookDeliveriesResponse> {
    return this._client.request<T.ListWebhookDeliveriesResponse>({ method: "GET", path: `/v1/webhooks/${encodeURIComponent(id)}/deliveries`, query: query, options });
  }

  /**
   * Every item of `listDeliveries`, fetching pages as you iterate.
   */
  listDeliveriesAll(id: string, query?: T.ListWebhookDeliveriesQuery, options?: RequestOptions): AsyncIterable<unknown> {
    return this._client.paginate<unknown>((after) => this.listDeliveries(id, { ...(query ?? {}), after } as T.ListWebhookDeliveriesQuery, options) as unknown as Promise<Page<unknown>>);
  }

  /**
   * Try a delivery again now
   *
   * `POST /v1/webhooks/{id}/deliveries/{delivery_id}/redeliver`
   */
  redeliver(id: string, deliveryId: string, options?: RequestOptions): Promise<T.RedeliverWebhookResponse> {
    return this._client.request<T.RedeliverWebhookResponse>({ method: "POST", path: `/v1/webhooks/${encodeURIComponent(id)}/deliveries/${encodeURIComponent(deliveryId)}/redeliver`, options });
  }
}

/** Every API area as a client property. `AllternitPlatform` extends this. */
export class GeneratedResources {
  accounts: AccountsResource;
  agents: AgentsResource;
  conversations: ConversationsResource;
  usage: UsageResource;
  numbers: NumbersResource;
  messaging: MessagingResource;
  webhooks: WebhooksResource;
  constructor(client: Transport) {
    this.accounts = new AccountsResource(client);
    this.agents = new AgentsResource(client);
    this.conversations = new ConversationsResource(client);
    this.usage = new UsageResource(client);
    this.numbers = new NumbersResource(client);
    this.messaging = new MessagingResource(client);
    this.webhooks = new WebhooksResource(client);
  }
}
