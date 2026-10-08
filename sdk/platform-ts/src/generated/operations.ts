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
   * One message at a time per conversation (`409 conversation_busy`). A project at its
   * monthly spend cap gets `402 spend_cap_reached`; one with no card on file gets
   * `402 payment_method_required`. Each reply records `tokens_in` and
   * `tokens_out` usage (provider list price + 15%; no token charge on the project's own key).
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
   * The project's monthly spend cap and spend so far
   *
   * Spend is this UTC calendar month's usage at list prices (before plan credits or discounts). Requires the `usage` scope.
   *
   * `GET /v1/projects/current/spend_cap`
   */
  getSpendCap(options?: RequestOptions): Promise<T.GetSpendCapResponse> {
    return this._client.request<T.GetSpendCapResponse>({ method: "GET", path: `/v1/projects/current/spend_cap`, options });
  }

  /**
   * Lower the monthly spend cap
   *
   * An API key can only lower the cap (403 `spend_cap_raise_in_console` otherwise); raise it in the console. Requires the `usage` scope and a key not bound to an account.
   *
   * `PUT /v1/projects/current/spend_cap`
   */
  putSpendCap(body: T.PutSpendCapRequest, options?: RequestOptions): Promise<T.PutSpendCapResponse> {
    return this._client.request<T.PutSpendCapResponse>({ method: "PUT", path: `/v1/projects/current/spend_cap`, body: body, options });
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
   * Bind the agent that answers calls (agent_id, null unbinds)
   *
   * `PATCH /v1/numbers/{id}`
   */
  update(id: string, body: T.UpdateNumberRequest, options?: RequestOptions): Promise<T.UpdateNumberResponse> {
    return this._client.request<T.UpdateNumberResponse>({ method: "PATCH", path: `/v1/numbers/${encodeURIComponent(id)}`, body: body, options });
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

export class VoiceResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List calls (filters account_id, agent_id, status; cursor pagination)
   *
   * `GET /v1/calls`
   */
  listCalls(options?: RequestOptions): Promise<T.ListCallsResponse> {
    return this._client.request<T.ListCallsResponse>({ method: "GET", path: `/v1/calls`, options });
  }

  /**
   * Place an outbound call (consent, STOP, business hours, spend cap checked)
   *
   * `POST /v1/calls`
   */
  createCall(body: T.CreateCallRequest, options?: RequestOptions): Promise<T.CreateCallResponse> {
    return this._client.request<T.CreateCallResponse>({ method: "POST", path: `/v1/calls`, body: body, options });
  }

  /**
   * Retrieve a call
   *
   * `GET /v1/calls/{id}`
   */
  getCall(id: string, options?: RequestOptions): Promise<T.GetCallResponse> {
    return this._client.request<T.GetCallResponse>({ method: "GET", path: `/v1/calls/${encodeURIComponent(id)}`, options });
  }

  /**
   * End a call
   *
   * `POST /v1/calls/{id}/end`
   */
  endCall(id: string, options?: RequestOptions): Promise<T.EndCallResponse> {
    return this._client.request<T.EndCallResponse>({ method: "POST", path: `/v1/calls/${encodeURIComponent(id)}/end`, options });
  }

  /**
   * Transfer to one of the agent's transfer_targets
   *
   * `POST /v1/calls/{id}/transfer`
   */
  transferCall(id: string, body: T.TransferCallRequest, options?: RequestOptions): Promise<T.TransferCallResponse> {
    return this._client.request<T.TransferCallResponse>({ method: "POST", path: `/v1/calls/${encodeURIComponent(id)}/transfer`, body: body, options });
  }

  /**
   * Call transcript
   *
   * `GET /v1/calls/{id}/transcript`
   */
  getCallTranscript(id: string, options?: RequestOptions): Promise<T.GetCallTranscriptResponse> {
    return this._client.request<T.GetCallTranscriptResponse>({ method: "GET", path: `/v1/calls/${encodeURIComponent(id)}/transcript`, options });
  }

  /**
   * Recording URL (only when the call was recorded)
   *
   * `GET /v1/calls/{id}/recording`
   */
  getCallRecording(id: string, options?: RequestOptions): Promise<T.GetCallRecordingResponse> {
    return this._client.request<T.GetCallRecordingResponse>({ method: "GET", path: `/v1/calls/${encodeURIComponent(id)}/recording`, options });
  }

  /**
   * Sandbox: play what the caller says
   *
   * `POST /v1/calls/{id}/simulate_turn`
   */
  simulateCallTurn(id: string, body: T.SimulateCallTurnRequest, options?: RequestOptions): Promise<T.SimulateCallTurnResponse> {
    return this._client.request<T.SimulateCallTurnResponse>({ method: "POST", path: `/v1/calls/${encodeURIComponent(id)}/simulate_turn`, body: body, options });
  }

  /**
   * Sandbox: play an inbound call
   *
   * `POST /v1/numbers/{id}/simulate_call`
   */
  simulateInboundCall(id: string, body: T.SimulateInboundCallRequest, options?: RequestOptions): Promise<T.SimulateInboundCallResponse> {
    return this._client.request<T.SimulateInboundCallResponse>({ method: "POST", path: `/v1/numbers/${encodeURIComponent(id)}/simulate_call`, body: body, options });
  }
}

export class RealtimeResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * Ephemeral client token for talking to an agent (60 s to start)
   *
   * `POST /v1/realtime/sessions`
   */
  createSession(body: T.CreateRealtimeSessionRequest, options?: RequestOptions): Promise<T.CreateRealtimeSessionResponse> {
    return this._client.request<T.CreateRealtimeSessionResponse>({ method: "POST", path: `/v1/realtime/sessions`, body: body, options });
  }
}

export class ChannelsResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List an account's channels
   *
   * Channel connections of the account (`?agent_id=` filter). Scope `channels`.
   *
   * `GET /v1/accounts/{id}/channels`
   */
  list(id: string, options?: RequestOptions): Promise<T.ListChannelsResponse> {
    return this._client.request<T.ListChannelsResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}/channels`, options });
  }

  /**
   * Connect a channel for an agent
   *
   * `{agent_id, return_url?, local_part?}`. `email` connects at once; `slack` answers `pending` with `connect_url`; kinds without a production app answer 400 `channel_kind_unavailable`.
   *
   * `POST /v1/accounts/{id}/channels/{kind}/connect`
   */
  connect(id: string, kind: string, body: T.ConnectChannelRequest, options?: RequestOptions): Promise<T.ConnectChannelResponse> {
    return this._client.request<T.ConnectChannelResponse>({ method: "POST", path: `/v1/accounts/${encodeURIComponent(id)}/channels/${encodeURIComponent(kind)}/connect`, body: body, options });
  }

  /**
   * Retrieve a channel
   *
   * One channel connection.
   *
   * `GET /v1/accounts/{id}/channels/{channel_id}`
   */
  get(id: string, channelId: string, options?: RequestOptions): Promise<T.GetChannelResponse> {
    return this._client.request<T.GetChannelResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}/channels/${encodeURIComponent(channelId)}`, options });
  }

  /**
   * Disconnect a channel
   *
   * The runtime releases the channel, then the record is deleted.
   *
   * `DELETE /v1/accounts/{id}/channels/{channel_id}`
   */
  delete(id: string, channelId: string, options?: RequestOptions): Promise<T.DeleteChannelResponse> {
    return this._client.request<T.DeleteChannelResponse>({ method: "DELETE", path: `/v1/accounts/${encodeURIComponent(id)}/channels/${encodeURIComponent(channelId)}`, options });
  }
}

export class TwinResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List people
   *
   * People the account's agents talk to (cursor `limit`/`after`). Scope `twin`.
   *
   * `GET /v1/accounts/{id}/people`
   */
  listPeople(id: string, options?: RequestOptions): Promise<T.ListPeopleResponse> {
    return this._client.request<T.ListPeopleResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}/people`, options });
  }

  /**
   * Retrieve a person
   *
   * With identities and the account's facts.
   *
   * `GET /v1/accounts/{id}/people/{person_id}`
   */
  getPerson(id: string, personId: string, options?: RequestOptions): Promise<T.GetPersonResponse> {
    return this._client.request<T.GetPersonResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}/people/${encodeURIComponent(personId)}`, options });
  }

  /**
   * Update a person
   *
   * `{name?, notes?, org?}`. 409 `person_shared` when another account's agents also know them.
   *
   * `PATCH /v1/accounts/{id}/people/{person_id}`
   */
  updatePerson(id: string, personId: string, body: T.UpdatePersonRequest, options?: RequestOptions): Promise<T.UpdatePersonResponse> {
    return this._client.request<T.UpdatePersonResponse>({ method: "PATCH", path: `/v1/accounts/${encodeURIComponent(id)}/people/${encodeURIComponent(personId)}`, body: body, options });
  }

  /**
   * List inbox items
   *
   * `?status=open|resolved|all`, newest first.
   *
   * `GET /v1/accounts/{id}/inbox`
   */
  listInbox(id: string, options?: RequestOptions): Promise<T.ListInboxResponse> {
    return this._client.request<T.ListInboxResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}/inbox`, options });
  }

  /**
   * Resolve an inbox item
   *
   * Marks it resolved.
   *
   * `POST /v1/accounts/{id}/inbox/{item_id}/resolve`
   */
  resolveInboxItem(id: string, itemId: string, options?: RequestOptions): Promise<T.ResolveInboxItemResponse> {
    return this._client.request<T.ResolveInboxItemResponse>({ method: "POST", path: `/v1/accounts/${encodeURIComponent(id)}/inbox/${encodeURIComponent(itemId)}/resolve`, options });
  }

  /**
   * List approvals
   *
   * `?status=pending|approved|denied|all`, newest first.
   *
   * `GET /v1/accounts/{id}/approvals`
   */
  listApprovals(id: string, options?: RequestOptions): Promise<T.ListApprovalsResponse> {
    return this._client.request<T.ListApprovalsResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}/approvals`, options });
  }

  /**
   * Approve a held action
   *
   * The API key is recorded as the actor.
   *
   * `POST /v1/accounts/{id}/approvals/{approval_id}/approve`
   */
  approveApproval(id: string, approvalId: string, options?: RequestOptions): Promise<T.ApproveApprovalResponse> {
    return this._client.request<T.ApproveApprovalResponse>({ method: "POST", path: `/v1/accounts/${encodeURIComponent(id)}/approvals/${encodeURIComponent(approvalId)}/approve`, options });
  }

  /**
   * Deny a held action
   *
   * The API key is recorded as the actor.
   *
   * `POST /v1/accounts/{id}/approvals/{approval_id}/deny`
   */
  denyApproval(id: string, approvalId: string, options?: RequestOptions): Promise<T.DenyApprovalResponse> {
    return this._client.request<T.DenyApprovalResponse>({ method: "POST", path: `/v1/accounts/${encodeURIComponent(id)}/approvals/${encodeURIComponent(approvalId)}/deny`, options });
  }

  /**
   * List account memory
   *
   * Facts every agent of the account knows.
   *
   * `GET /v1/accounts/{id}/memory`
   */
  listMemory(id: string, options?: RequestOptions): Promise<T.ListMemoryResponse> {
    return this._client.request<T.ListMemoryResponse>({ method: "GET", path: `/v1/accounts/${encodeURIComponent(id)}/memory`, options });
  }

  /**
   * Add account memory
   *
   * `{content, subject?, kind?, source_ref?}`; at most 500 per account.
   *
   * `POST /v1/accounts/{id}/memory`
   */
  addMemory(id: string, body: T.AddMemoryRequest, options?: RequestOptions): Promise<T.AddMemoryResponse> {
    return this._client.request<T.AddMemoryResponse>({ method: "POST", path: `/v1/accounts/${encodeURIComponent(id)}/memory`, body: body, options });
  }

  /**
   * Delete account memory
   *
   * Removes one item; agents get the change on their next sync.
   *
   * `DELETE /v1/accounts/{id}/memory/{memory_id}`
   */
  deleteMemory(id: string, memoryId: string, options?: RequestOptions): Promise<T.DeleteMemoryResponse> {
    return this._client.request<T.DeleteMemoryResponse>({ method: "DELETE", path: `/v1/accounts/${encodeURIComponent(id)}/memory/${encodeURIComponent(memoryId)}`, options });
  }

  /**
   * Retrieve an agent's autonomy
   *
   * Agent-wide `level` and per channel / person `rules`.
   *
   * `GET /v1/agents/{id}/autonomy`
   */
  getAutonomy(id: string, options?: RequestOptions): Promise<T.GetAutonomyResponse> {
    return this._client.request<T.GetAutonomyResponse>({ method: "GET", path: `/v1/agents/${encodeURIComponent(id)}/autonomy`, options });
  }

  /**
   * Replace an agent's autonomy
   *
   * `{level?, rules:[{channel?, person?, level, limits?}]}` replaces every rule.
   *
   * `PUT /v1/agents/{id}/autonomy`
   */
  putAutonomy(id: string, body: T.PutAutonomyRequest, options?: RequestOptions): Promise<T.PutAutonomyResponse> {
    return this._client.request<T.PutAutonomyResponse>({ method: "PUT", path: `/v1/agents/${encodeURIComponent(id)}/autonomy`, body: body, options });
  }
}

export class ComputersResource {
  protected readonly _client: Transport;
  constructor(client: Transport) {
    this._client = client;
  }

  /**
   * List computers
   *
   * Hosted-driver computers in this project (an account-bound key sees its account's only). Requires the `computers` scope and the project's hosted driver flag (404 `hosted_driver_disabled` until Allternit turns it on).
   *
   * `GET /v1/computers`
   */
  list(query?: T.ListComputersQuery, options?: RequestOptions): Promise<T.ListComputersResponse> {
    return this._client.request<T.ListComputersResponse>({ method: "GET", path: `/v1/computers`, query: query, options });
  }

  /**
   * Every item of `list`, fetching pages as you iterate.
   */
  listAll(query?: T.ListComputersQuery, options?: RequestOptions): AsyncIterable<T.Computer> {
    return this._client.paginate<T.Computer>((after) => this.list({ ...(query ?? {}), after } as T.ListComputersQuery, options) as unknown as Promise<Page<T.Computer>>);
  }

  /**
   * Create a computer
   *
   * Provisions a cloud computer your agents drive with the computer and browser toolsets. 402 `payment_method_required` without a card on file, 402 `spend_cap_reached` at the spend cap; 429 `concurrency_limit` when this key already has `per_key_concurrency` live computers.
   *
   * `POST /v1/computers`
   */
  create(body?: T.CreateComputerRequest, options?: RequestOptions): Promise<T.CreateComputerResponse> {
    return this._client.request<T.CreateComputerResponse>({ method: "POST", path: `/v1/computers`, body: body, options });
  }

  /**
   * Get a computer
   *
   * `GET /v1/computers/{id}`
   */
  get(id: string, options?: RequestOptions): Promise<T.GetComputerResponse> {
    return this._client.request<T.GetComputerResponse>({ method: "GET", path: `/v1/computers/${encodeURIComponent(id)}`, options });
  }

  /**
   * Delete a computer
   *
   * Deletes the computer and its disk. Running minutes up to now are billed.
   *
   * `DELETE /v1/computers/{id}`
   */
  delete(id: string, options?: RequestOptions): Promise<T.DeleteComputerResponse> {
    return this._client.request<T.DeleteComputerResponse>({ method: "DELETE", path: `/v1/computers/${encodeURIComponent(id)}`, options });
  }

  /**
   * Start a computer
   *
   * `POST /v1/computers/{id}/start`
   */
  start(id: string, options?: RequestOptions): Promise<T.StartComputerResponse> {
    return this._client.request<T.StartComputerResponse>({ method: "POST", path: `/v1/computers/${encodeURIComponent(id)}/start`, options });
  }

  /**
   * Stop a computer
   *
   * `POST /v1/computers/{id}/stop`
   */
  stop(id: string, options?: RequestOptions): Promise<T.StopComputerResponse> {
    return this._client.request<T.StopComputerResponse>({ method: "POST", path: `/v1/computers/${encodeURIComponent(id)}/stop`, options });
  }

  /**
   * Run one toolset call
   *
   * One `allternit.computer.v1` / `allternit.browser.v1` call (member names and inputs match Anthropic's `computer_toolset_20260801` / `browser_toolset_20260801`). Runs through the computer's executor: contract validation, control lease, policy, approval, audit, then the action. Action failures are 200 with `is_error: true`.
   * 409 `approval_required` carries `approval`: approve it (`POST /v1/computers/{id}/approvals/{approval_id}`, or the project owner in the console), then resend the same call with `approval_grant`.
   *
   * `POST /v1/computers/{id}/toolset`
   */
  callToolset(id: string, body: T.CallComputerToolsetRequest, options?: RequestOptions): Promise<T.CallComputerToolsetResponse> {
    return this._client.request<T.CallComputerToolsetResponse>({ method: "POST", path: `/v1/computers/${encodeURIComponent(id)}/toolset`, body: body, options });
  }

  /**
   * Members available on this computer
   *
   * `GET /v1/computers/{id}/toolset/schema`
   */
  getToolsetSchema(id: string, query?: T.GetComputerToolsetSchemaQuery, options?: RequestOptions): Promise<T.GetComputerToolsetSchemaResponse> {
    return this._client.request<T.GetComputerToolsetSchemaResponse>({ method: "GET", path: `/v1/computers/${encodeURIComponent(id)}/toolset/schema`, query: query, options });
  }

  /**
   * Poll the computer's action events
   *
   * `computer.action` events, oldest first. Pass the last page's `next_cursor` as `after` to poll.
   *
   * `GET /v1/computers/{id}/events`
   */
  listEvents(id: string, query?: T.ListComputerEventsQuery, options?: RequestOptions): Promise<T.ListComputerEventsResponse> {
    return this._client.request<T.ListComputerEventsResponse>({ method: "GET", path: `/v1/computers/${encodeURIComponent(id)}/events`, query: query, options });
  }

  /**
   * Every item of `listEvents`, fetching pages as you iterate.
   */
  listEventsAll(id: string, query?: T.ListComputerEventsQuery, options?: RequestOptions): AsyncIterable<T.ComputerEvent> {
    return this._client.paginate<T.ComputerEvent>((after) => this.listEvents(id, { ...(query ?? {}), after } as T.ListComputerEventsQuery, options) as unknown as Promise<Page<T.ComputerEvent>>);
  }

  /**
   * Approve a held action
   *
   * The project owner (console) can always approve. An API key can only when the project's `approval_mode` is `api_key` (403 `approval_requires_owner` otherwise).
   *
   * `POST /v1/computers/{id}/approvals/{approval_id}`
   */
  approveAction(id: string, approvalId: string, options?: RequestOptions): Promise<T.ApproveComputerActionResponse> {
    return this._client.request<T.ApproveComputerActionResponse>({ method: "POST", path: `/v1/computers/${encodeURIComponent(id)}/approvals/${encodeURIComponent(approvalId)}`, options });
  }

  /**
   * The project's hosted driver settings
   *
   * Readable while the flag is off (so you can see it). Requires the `computers` scope.
   *
   * `GET /v1/computer_settings`
   */
  getSettings(options?: RequestOptions): Promise<T.GetComputerSettingsResponse> {
    return this._client.request<T.GetComputerSettingsResponse>({ method: "GET", path: `/v1/computer_settings`, options });
  }

  /**
   * Change the hosted driver settings
   *
   * Console only (403 `settings_require_console` for API keys), so a key can't widen its own approvals. `hosted_driver_enabled` is set by Allternit.
   *
   * `PATCH /v1/computer_settings`
   */
  updateSettings(body: T.UpdateComputerSettingsRequest, options?: RequestOptions): Promise<T.UpdateComputerSettingsResponse> {
    return this._client.request<T.UpdateComputerSettingsResponse>({ method: "PATCH", path: `/v1/computer_settings`, body: body, options });
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
  voice: VoiceResource;
  realtime: RealtimeResource;
  channels: ChannelsResource;
  twin: TwinResource;
  computers: ComputersResource;
  constructor(client: Transport) {
    this.accounts = new AccountsResource(client);
    this.agents = new AgentsResource(client);
    this.conversations = new ConversationsResource(client);
    this.usage = new UsageResource(client);
    this.numbers = new NumbersResource(client);
    this.messaging = new MessagingResource(client);
    this.webhooks = new WebhooksResource(client);
    this.voice = new VoiceResource(client);
    this.realtime = new RealtimeResource(client);
    this.channels = new ChannelsResource(client);
    this.twin = new TwinResource(client);
    this.computers = new ComputersResource(client);
  }
}
