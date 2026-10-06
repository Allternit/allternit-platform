import { HEADER_SUBSCRIPTION_ID, validateCallbackUrl, type ControlEnvelope, type EventEnvelope } from "./events.js";
import { signHeaders } from "./webhooks.js";

export interface DeliverOptions {
  /** Callback URL from the subscription (already validated at subscribe time; checked again here). */
  url: string;
  /** Key bytes from `parseSecret(delivery.secret)`. */
  key: Uint8Array;
  subscriptionId: string;
  /** `webhook-id`: unique per event, stable across retries (use the event id). */
  msgId: string;
  body: EventEnvelope | ControlEnvelope;
  /** Unix seconds; defaults to now. */
  timestamp?: number;
  fetch?: typeof fetch;
  signal?: AbortSignal;
}

/**
 * POST one signed delivery (Standard Webhooks headers + `X-MCP-Subscription-Id`).
 * Resolves with the receiver's response; callers own retry policy. The URL is
 * re-checked literally, but hostnames are not resolved here: a server that
 * delivers to user-supplied URLs must also pin/resolve DNS and refuse private
 * addresses at connect time.
 */
export async function deliverWebhook(o: DeliverOptions): Promise<Response> {
  validateCallbackUrl(o.url);
  const body = JSON.stringify(o.body);
  const ts = o.timestamp ?? Math.floor(Date.now() / 1000);
  return (o.fetch ?? fetch)(o.url, {
    method: "POST",
    redirect: "manual",
    signal: o.signal,
    headers: { "content-type": "application/json", [HEADER_SUBSCRIPTION_ID]: o.subscriptionId, ...signHeaders(o.key, o.msgId, ts, body) },
    body,
  });
}
