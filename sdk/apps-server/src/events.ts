import { ProtocolError, type McpServer, type ServerContext } from "@modelcontextprotocol/server";
import {
  EVENTS_METHODS,
  EventsErrorCode,
  subscriptionId,
  validateWebhook,
  type EventDef,
  type SubscribeParams,
  type SubscribeResult,
  type UnsubscribeParams,
} from "@allternit/mcp-events";
import { z } from "zod";

/** What the handler is told about one validated subscribe/unsubscribe. */
export interface EventSubscription {
  /** Deterministic id for (principal, url, name, arguments): same inputs, same id. */
  id: string;
  name: string;
  arguments: Record<string, unknown>;
  url: string;
  /** Key bytes from the client's `whsec_` secret (subscribe only). Sign deliveries with `deliverWebhook`. */
  key?: Uint8Array;
  cursor?: string | null;
  /** Absent = your default; `null` = the client asked for no expiry. */
  ttlMs?: number | null;
  /** Who is subscribing, from `principal(ctx)`; `"anonymous"` without auth. */
  principal: string;
}

export interface AppEventsInput {
  /** The events this app emits. Listed sorted by name. */
  definitions: EventDef[];
  /**
   * Store the subscription and start delivering. Return the expiry the client
   * must refresh before (ISO 8601, or null) and the cursor you start from.
   * Throw `eventsError(...)` for NotFound/Forbidden/ResourceExhausted/CallbackEndpointError.
   */
  subscribe(sub: EventSubscription, ctx: ServerContext): Promise<Pick<SubscribeResult, "refreshBefore" | "cursor"> & Partial<SubscribeResult>>;
  /** Stop delivering. Unknown subscriptions are not an error. */
  unsubscribe(sub: EventSubscription, ctx: ServerContext): Promise<void>;
  /** Derive the principal from the request (e.g. `ctx.http?.authInfo?.clientId`). Default: the auth client id, else "anonymous". */
  principal?(ctx: ServerContext): string;
}

/** A JSON-RPC error with one of the MCP Events codes (-32011..-32015). */
export function eventsError(code: (typeof EventsErrorCode)[keyof typeof EventsErrorCode], message: string, data?: unknown): ProtocolError {
  return new ProtocolError(code, message, data);
}

const loose = z.object({}).passthrough();
const delivery = z.object({ mode: z.enum(["webhook", "poll", "push"]), url: z.string().optional(), secret: z.string().optional() }).passthrough();
const subscribeParams = z
  .object({
    name: z.string(),
    arguments: z.record(z.string(), z.unknown()).optional(),
    delivery,
    cursor: z.string().nullable().optional(),
    ttlMs: z.number().int().nonnegative().nullable().optional(),
  })
  .passthrough();
const unsubscribeParams = z.object({ name: z.string(), arguments: z.record(z.string(), z.unknown()).optional(), delivery }).passthrough();

/** Validate the event definitions up front so a typo fails at startup, not on a client's subscribe. */
export function checkEventDefinitions(defs: EventDef[]): void {
  const seen = new Set<string>();
  for (const d of defs) {
    if (!d.name?.trim()) throw new Error("events: every definition needs a name");
    if (seen.has(d.name)) throw new Error(`events: duplicate event ${d.name}`);
    seen.add(d.name);
    if (!d.delivery?.includes("webhook")) throw new Error(`events: ${d.name} must offer webhook delivery`);
  }
}

/**
 * Serve the MCP Events extension (`events/list|subscribe|unsubscribe`) on an
 * SDK server and advertise the `events` capability. The official SDK does not
 * implement the extension yet; this is the thin layer over it.
 */
export function registerEvents(server: McpServer, input: AppEventsInput): void {
  checkEventDefinitions(input.definitions);
  const defs = [...input.definitions].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  const known = new Set(defs.map((d) => d.name));
  const principalOf = (ctx: ServerContext) => input.principal?.(ctx) ?? ctx.http?.authInfo?.clientId ?? "anonymous";
  server.server.registerCapabilities({ events: { listChanged: false } } as never);

  server.server.setRequestHandler(EVENTS_METHODS.list, { params: loose, result: loose }, async () => ({ events: defs }));

  const resolve = (p: UnsubscribeParams | SubscribeParams, ctx: ServerContext, needSecret: boolean): EventSubscription => {
    if (!known.has(p.name)) throw eventsError(EventsErrorCode.NotFound, `unknown event ${p.name}`);
    let target: { url: string; key?: Uint8Array };
    try {
      target = validateWebhook(p.delivery, needSecret);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (/only webhook/.test(message)) throw eventsError(EventsErrorCode.Unsupported, message);
      throw new ProtocolError(-32602, message);
    }
    const args = p.arguments ?? {};
    const principal = principalOf(ctx);
    return {
      id: subscriptionId(principal, target.url, p.name, args),
      name: p.name,
      arguments: args,
      url: target.url,
      key: target.key,
      principal,
      ...("cursor" in p ? { cursor: p.cursor } : {}),
      ...("ttlMs" in p ? { ttlMs: p.ttlMs } : {}),
    };
  };

  server.server.setRequestHandler(EVENTS_METHODS.subscribe, { params: subscribeParams, result: loose }, async (params, ctx) => {
    const sub = resolve(params as SubscribeParams, ctx as ServerContext, true);
    const r = await input.subscribe(sub, ctx as ServerContext);
    const result: SubscribeResult = { truncated: false, ...r, id: sub.id };
    return result as unknown as Record<string, unknown>;
  });

  server.server.setRequestHandler(EVENTS_METHODS.unsubscribe, { params: unsubscribeParams, result: loose }, async (params, ctx) => {
    await input.unsubscribe(resolve(params as UnsubscribeParams, ctx as ServerContext, false), ctx as ServerContext);
    return {};
  });
}
