/**
 * The Allternit Platform API client.
 *
 *   const client = new AllternitPlatform({ apiKey: process.env.ALLTERNIT_API_KEY });
 *   const agent = await client.agents.create({ account_id, name: "Front desk" });
 *
 * Resources (`client.agents`, `client.conversations`, …) are generated from
 * the OpenAPI file; this file adds the typed conversation stream on top.
 */

import { HttpTransport, type ClientOptions, type RequestOptions, type SSEEvent } from "./core.ts";
import { errorFor, type ErrorBody } from "./errors.ts";
import { ConversationsResource, GeneratedResources } from "./generated/operations.ts";
import type { ConversationMessage, SendConversationMessageRequest } from "./generated/types.ts";

export type MessageDeltaEvent = { type: "message.delta"; delta: string };
export type MessageCompletedEvent = { type: "message.completed"; message: ConversationMessage };
export type ConversationStreamEvent = MessageDeltaEvent | MessageCompletedEvent;

/**
 * The agent's reply as it is written. Iterate it for `message.delta` and
 * `message.completed` events; an `error` event throws a typed `APIError`.
 */
export class ConversationStream implements AsyncIterable<ConversationStreamEvent> {
  private readonly source: AsyncIterable<SSEEvent>;
  private started = false;
  private final: ConversationMessage | undefined;
  private text = "";

  constructor(source: AsyncIterable<SSEEvent>) {
    this.source = source;
  }

  async *[Symbol.asyncIterator](): AsyncIterator<ConversationStreamEvent> {
    if (this.started) throw new Error("A ConversationStream can only be read once.");
    this.started = true;
    for await (const ev of this.source) {
      if (ev.event === "message.delta") {
        const delta = String((ev.data as { delta?: unknown })?.delta ?? "");
        this.text += delta;
        yield { type: "message.delta", delta };
      } else if (ev.event === "message.completed") {
        this.final = ev.data as ConversationMessage;
        yield { type: "message.completed", message: this.final };
      } else if (ev.event === "error") {
        const body = (ev.data as { error?: ErrorBody })?.error ?? { message: String(ev.data) };
        throw errorFor(0, body, undefined, "The stream reported an error.");
      }
    }
  }

  /** Read the whole stream and return the stored reply. */
  async finalMessage(): Promise<ConversationMessage> {
    if (!this.started) {
      for await (const _ of this) { /* drain */ }
    }
    if (!this.final) throw new Error("The stream ended without a message.completed event.");
    return this.final;
  }

  /** Text received so far (all of it once the stream is done). */
  get receivedText(): string {
    return this.text;
  }
}

export class Conversations extends ConversationsResource {
  /**
   * Send a message and stream the agent's reply
   * (`POST /v1/conversations/{id}/messages` with `stream: true`).
   */
  stream(id: string, body: Omit<SendConversationMessageRequest, "stream">, options?: RequestOptions): ConversationStream {
    return new ConversationStream(this.sendMessageStream(id, body, options));
  }
}

export class AllternitPlatform extends GeneratedResources {
  declare conversations: Conversations;
  /** The transport, for calls the SDK doesn't wrap yet: `client.http.request({ method: "GET", path: "/v1/…" })`. */
  readonly http: HttpTransport;

  constructor(options: ClientOptions = {}) {
    const http = new HttpTransport(options);
    super(http);
    this.http = http;
    this.conversations = new Conversations(http);
  }
}
