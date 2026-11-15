/**
 * Client for a self-hosted Allternit Agents Server (`allternit-agents-server`).
 *
 *   const client = new SelfHostClient({
 *     baseUrl: "https://agents.example.internal", // default http://127.0.0.1:8400
 *     apiKey: process.env.ALLTERNIT_SERVER_API_KEY, // ask_…
 *   });
 *   const agent = await client.agents.create({ name: "Helper", model: "claude-sonnet" });
 *
 * Resources (`client.agents`, `client.environments`, `client.sessions`, …) are
 * generated from `cmd/allternit-api/openapi/selfhost-v1.yaml`. This file adds
 * a typed, resumable session event stream. The server is yours: this client
 * never calls `*.allternit.com`.
 */

import { HttpTransport, type ClientOptions, type RequestOptions, type SSEEvent } from "../core.ts";
import { APIConnectionError } from "../errors.ts";
import { GeneratedResources, SessionsResource } from "./operations.ts";
import type { Event, SessionStatus } from "./types.ts";

export const SELFHOST_DEFAULT_BASE_URL = "http://127.0.0.1:8400";

export interface SelfHostClientOptions extends Omit<ClientOptions, "apiKey" | "baseUrl"> {
  /**
   * A server API key (`ask_…`), an environment key (`aek_…`, for workers) or
   * an OIDC id token. Defaults to `process.env.ALLTERNIT_SERVER_API_KEY`.
   */
  apiKey?: string;
  /** Defaults to `process.env.ALLTERNIT_SERVER_URL` or http://127.0.0.1:8400. */
  baseUrl?: string;
}

/** Session states after which a turn is over. */
export const TURN_END_STATUSES: ReadonlySet<SessionStatus> = new Set<SessionStatus>(["idle", "interrupted", "error", "archived"]);

export interface StreamEventsOptions extends Omit<RequestOptions, "query"> {
  /** Replay events after this `seq`, then go live. Omit to start live. */
  afterSeq?: number;
  /** Reconnect after a dropped connection, resuming from the last `seq` seen. Default true. */
  reconnect?: boolean;
  /** Give up after this many reconnects in a row without a new event. Default 5. */
  maxReconnects?: number;
  /** Stop when this returns true for an event (the event is still yielded). */
  until?: (event: Event) => boolean;
}

function env(name: string): string | undefined {
  const p = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process;
  return p?.env?.[name];
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const t = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => { clearTimeout(t); resolve(); }, { once: true });
  });
}

/** True for an event that ends a turn: a `status` event with idle, interrupted, error or archived. */
export function isTurnEnd(event: Event): boolean {
  if (event.type !== "status") return false;
  const status = (event.data as { status?: SessionStatus })?.status;
  return status !== undefined && TURN_END_STATUSES.has(status);
}

export class SelfHostSessions extends SessionsResource {
  /**
   * Session events as typed `Event`s over SSE
   * (`GET /v1/sessions/{id}/events/stream`). Keepalive comments are skipped.
   * After a dropped connection it reconnects with `Last-Event-ID`, so no event
   * is missed or repeated. Session `error` events are yielded, not thrown.
   *
   *   const sent = await client.sessions.sendMessage(id, { content: "hi" });
   *   for await (const ev of client.sessions.streamEvents(id, { afterSeq: sent.seq, until: isTurnEnd })) { … }
   */
  async *streamEvents(id: string, options: StreamEventsOptions = {}): AsyncGenerator<Event> {
    const { afterSeq, reconnect = true, maxReconnects = 5, until, ...request } = options;
    let lastSeq = afterSeq;
    let failures = 0;
    for (;;) {
      let gotEvent = false;
      try {
        const headers = { ...(request.headers ?? {}) };
        if (lastSeq !== undefined) headers["Last-Event-ID"] = String(lastSeq);
        const source: AsyncIterable<SSEEvent> = this._client.streamRequest({
          method: "GET",
          path: `/v1/sessions/${encodeURIComponent(id)}/events/stream`,
          query: lastSeq !== undefined ? { after_seq: lastSeq } : undefined,
          options: { ...request, headers },
        });
        for await (const frame of source) {
          const event = frame.data as Event;
          if (!event || typeof event !== "object" || typeof event.seq !== "number") continue;
          if (lastSeq !== undefined && event.seq <= lastSeq) continue;
          lastSeq = event.seq;
          gotEvent = true;
          failures = 0;
          yield event;
          if (until?.(event)) return;
        }
      } catch (err) {
        if (request.signal?.aborted) return;
        if (!(err instanceof APIConnectionError) || !reconnect) throw err;
      }
      if (request.signal?.aborted || !reconnect) return;
      if (!gotEvent) failures += 1;
      if (failures > maxReconnects) {
        throw new APIConnectionError(`The event stream for ${id} dropped ${failures} times in a row.`);
      }
      await sleep(Math.min(500 * 2 ** failures, 10_000), request.signal);
    }
  }
}

export class SelfHostClient extends GeneratedResources {
  declare sessions: SelfHostSessions;
  /** The transport, for calls the SDK doesn't wrap: `client.http.request({ method: "GET", path: "/v1/…" })`. */
  readonly http: HttpTransport;

  constructor(options: SelfHostClientOptions = {}) {
    const apiKey = options.apiKey ?? env("ALLTERNIT_SERVER_API_KEY");
    if (!apiKey) {
      throw new Error("No key. Pass { apiKey } or set ALLTERNIT_SERVER_API_KEY (ask_… server key or aek_… environment key).");
    }
    const baseUrl = options.baseUrl ?? env("ALLTERNIT_SERVER_URL") ?? SELFHOST_DEFAULT_BASE_URL;
    const http = new HttpTransport({ ...options, apiKey, baseUrl });
    super(http);
    this.http = http;
    this.sessions = new SelfHostSessions(http);
  }
}
