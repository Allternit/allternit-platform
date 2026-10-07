/**
 * HTTP transport shared by every generated resource: auth, Idempotency-Key,
 * timeouts, error mapping, cursor pagination and server-sent events.
 */

import { APIConnectionError, APITimeoutError, errorFor, type ErrorBody } from "./errors.ts";

export const DEFAULT_BASE_URL = "https://api.allternit.com";
export const DEFAULT_TIMEOUT_MS = 60_000;

export interface RequestOptions {
  /** Sent as `Idempotency-Key` on POST. A random UUID is used when omitted. */
  idempotencyKey?: string;
  /** Abort the request (and, for streams, stop reading). */
  signal?: AbortSignal;
  /** Milliseconds before the request is aborted. For streams this bounds the wait for the first byte. */
  timeout?: number;
  /** Extra headers. */
  headers?: Record<string, string>;
  /** Extra query parameters. */
  query?: Record<string, unknown>;
}

export interface Page<T> {
  data: T[];
  has_more: boolean;
  next_cursor: string | null;
  [key: string]: unknown;
}

/** One server-sent event: its `event:` name and its `data:` payload, JSON-parsed when possible. */
export interface SSEEvent {
  event: string;
  data: unknown;
}

export interface RequestSpec {
  method: string;
  path: string;
  query?: object | undefined;
  body?: unknown;
  options?: RequestOptions | undefined;
}

/** What generated resources call. Implemented by `AllternitPlatform`. */
export interface Transport {
  request<T>(spec: RequestSpec): Promise<T>;
  streamRequest(spec: RequestSpec): AsyncIterable<SSEEvent>;
  paginate<T>(fetchPage: (after: string | undefined) => Promise<Page<T>>): AsyncIterable<T>;
}

export interface ClientOptions {
  /** Project API key (`alt_live_…` or `alt_test_…`). Defaults to `process.env.ALLTERNIT_API_KEY`. */
  apiKey?: string;
  /** Defaults to `process.env.ALLTERNIT_BASE_URL` or https://api.allternit.com. */
  baseUrl?: string;
  /** Default per-request timeout in milliseconds (60 000). */
  timeout?: number;
  /** Headers sent with every request. */
  defaultHeaders?: Record<string, string>;
  /** A custom fetch (tests, proxies). Defaults to the global fetch. */
  fetch?: typeof fetch;
}

function env(name: string): string | undefined {
  const p = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process;
  return p?.env?.[name];
}

function uuid(): string {
  const c = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  if (c?.randomUUID) return c.randomUUID();
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (ch) => {
    const r = (Math.random() * 16) | 0;
    return (ch === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}

export class HttpTransport implements Transport {
  readonly baseUrl: string;
  readonly timeout: number;
  private readonly apiKey: string;
  private readonly defaultHeaders: Record<string, string>;
  private readonly fetchImpl: typeof fetch;

  constructor(opts: ClientOptions = {}) {
    const apiKey = opts.apiKey ?? env("ALLTERNIT_API_KEY");
    if (!apiKey) {
      throw new Error("No API key. Pass { apiKey } or set ALLTERNIT_API_KEY (alt_test_… or alt_live_…).");
    }
    this.apiKey = apiKey;
    this.baseUrl = (opts.baseUrl ?? env("ALLTERNIT_BASE_URL") ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
    this.timeout = opts.timeout ?? DEFAULT_TIMEOUT_MS;
    this.defaultHeaders = opts.defaultHeaders ?? {};
    const f = opts.fetch ?? globalThis.fetch;
    if (!f) throw new Error("No global fetch; pass { fetch } (Node 18+ has one built in).");
    this.fetchImpl = f;
  }

  private url(path: string, query?: object, extra?: Record<string, unknown>): string {
    const params = new URLSearchParams();
    for (const src of [query, extra]) {
      if (!src) continue;
      for (const [k, v] of Object.entries(src)) {
        if (v === undefined || v === null) continue;
        if (Array.isArray(v)) v.forEach((x) => params.append(k, String(x)));
        else params.append(k, String(v));
      }
    }
    const qs = params.toString();
    return `${this.baseUrl}${path}${qs ? `?${qs}` : ""}`;
  }

  private async send(spec: RequestSpec, accept: string): Promise<{ response: Response; done: () => void }> {
    const o = spec.options ?? {};
    const headers: Record<string, string> = {
      Accept: accept,
      Authorization: `Bearer ${this.apiKey}`,
      ...this.defaultHeaders,
      ...(o.headers ?? {}),
    };
    let body: string | undefined;
    if (spec.body !== undefined && spec.method !== "GET") {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(spec.body);
    }
    if (spec.method === "POST" && !Object.keys(headers).some((h) => h.toLowerCase() === "idempotency-key")) {
      headers["Idempotency-Key"] = o.idempotencyKey ?? uuid();
    }

    const controller = new AbortController();
    const timeoutMs = o.timeout ?? this.timeout;
    let timedOut = false;
    const timer = timeoutMs > 0 ? setTimeout(() => { timedOut = true; controller.abort(); }, timeoutMs) : undefined;
    const onAbort = () => controller.abort();
    if (o.signal) {
      if (o.signal.aborted) controller.abort();
      else o.signal.addEventListener("abort", onAbort, { once: true });
    }
    const done = () => {
      if (timer) clearTimeout(timer);
      o.signal?.removeEventListener("abort", onAbort);
    };

    let response: Response;
    try {
      response = await this.fetchImpl(this.url(spec.path, spec.query, o.query), {
        method: spec.method,
        headers,
        body,
        signal: controller.signal,
      });
    } catch (err) {
      done();
      if (timedOut) throw new APITimeoutError(`Request timed out after ${timeoutMs} ms.`, err);
      if (o.signal?.aborted) throw err;
      throw new APIConnectionError(`Could not reach ${this.baseUrl}: ${(err as Error)?.message ?? err}`, err);
    }
    if (!response.ok) {
      const text = await response.text().catch(() => "");
      done();
      let parsed: { error?: ErrorBody } | undefined;
      try { parsed = text ? JSON.parse(text) : undefined; } catch { parsed = undefined; }
      const errBody = parsed && typeof parsed.error === "object" ? parsed.error : undefined;
      throw errorFor(response.status, errBody, response.headers, text.slice(0, 200) || undefined);
    }
    return { response, done };
  }

  async request<T>(spec: RequestSpec): Promise<T> {
    const { response, done } = await this.send(spec, "application/json");
    try {
      if (response.status === 204) return undefined as T;
      const text = await response.text();
      return (text ? JSON.parse(text) : undefined) as T;
    } finally {
      done();
    }
  }

  async *streamRequest(spec: RequestSpec): AsyncGenerator<SSEEvent> {
    const { response, done } = await this.send(spec, "text/event-stream");
    // The first byte arrived: the timeout only bounds the wait for it.
    try {
      if (!response.body) return;
      yield* parseSSE(response.body);
    } finally {
      done();
    }
  }

  async *paginate<T>(fetchPage: (after: string | undefined) => Promise<Page<T>>): AsyncGenerator<T> {
    let after: string | undefined;
    for (;;) {
      const page = await fetchPage(after);
      for (const item of page.data ?? []) yield item;
      if (!page.has_more || !page.next_cursor) return;
      after = page.next_cursor;
    }
  }
}

/** Parse a `text/event-stream` body into events. */
export async function* parseSSE(body: ReadableStream<Uint8Array>): AsyncGenerator<SSEEvent> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let event = "message";
  let data: string[] = [];
  const flush = (): SSEEvent | undefined => {
    if (data.length === 0) {
      event = "message";
      return undefined;
    }
    const raw = data.join("\n");
    let parsed: unknown = raw;
    try { parsed = JSON.parse(raw); } catch { /* keep the raw string */ }
    const out = { event, data: parsed };
    event = "message";
    data = [];
    return out;
  };
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (value) buffer += decoder.decode(value, { stream: true });
      if (done) buffer += "\n\n";
      let nl: number;
      while ((nl = buffer.search(/\r\n|\r|\n/)) >= 0) {
        const line = buffer.slice(0, nl);
        buffer = buffer.slice(nl + (buffer.startsWith("\r\n", nl) ? 2 : 1));
        if (line === "") {
          const ev = flush();
          if (ev) yield ev;
          continue;
        }
        if (line.startsWith(":")) continue;
        const colon = line.indexOf(":");
        const field = colon < 0 ? line : line.slice(0, colon);
        let value = colon < 0 ? "" : line.slice(colon + 1);
        if (value.startsWith(" ")) value = value.slice(1);
        if (field === "event") event = value;
        else if (field === "data") data.push(value);
      }
      if (done) return;
    }
  } finally {
    reader.cancel().catch(() => undefined);
  }
}
