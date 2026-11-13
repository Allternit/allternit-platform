/**
 * Virtual-key access to the public OpenAI-compatible gateway (`/v1/*`).
 *
 * The `/v1/chat/completions`, `/v1/files`, and `/v1/batches` routes authenticate
 * with gateway virtual keys (`Authorization: Bearer ak-…`, see
 * `cmd/allternit-api/src/llm_gateway/auth.rs::llm_key_middleware`), NOT with the
 * Clerk session token the console api client normally sends. Key management is
 * Clerk-protected at `/api/v1/gateway/keys`; the full plaintext key is returned
 * exactly once at creation.
 *
 * Console pattern: on first use, create a dedicated "Console" virtual key for
 * the signed-in user, keep it in localStorage, and send it as an explicit
 * Authorization override on `/v1/*` calls. The key is user-scoped and has no
 * budget or model restrictions unless set via the API.
 */

import { api } from "@/lib/api-client";

const STORAGE_KEY = "allternit:console:gateway-key";

let cached: string | null = null;

export function getStoredGatewayKey(): string | null {
  if (cached) return cached;
  try {
    cached = window.localStorage.getItem(STORAGE_KEY);
  } catch {
    cached = null;
  }
  return cached;
}

function storeGatewayKey(key: string): void {
  cached = key;
  try {
    window.localStorage.setItem(STORAGE_KEY, key);
  } catch {
    // Storage unavailable — the key still works for this session via the cache.
  }
}

/**
 * Return the console's virtual key, creating and persisting one on first use.
 * Throws (with the API error message) when key creation is refused.
 */
export async function ensureConsoleGatewayKey(): Promise<string> {
  const existing = getStoredGatewayKey();
  if (existing) return existing;

  const created = await api.post<{
    id: string;
    key: string;
    key_prefix: string;
    warning?: string;
  }>("/api/v1/gateway/keys", { name: "Console (auto-created)" });

  storeGatewayKey(created.key);
  return created.key;
}

export function forgetStoredGatewayKey(): void {
  cached = null;
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    // ignore
  }
}

/** RequestInit headers that authenticate a `/v1/*` call with the virtual key. */
export function gatewayAuthOptions(
  key: string,
  extra: RequestInit = {}
): RequestInit {
  return {
    ...extra,
    headers: {
      Authorization: `Bearer ${key}`,
      ...(extra.headers as Record<string, string> | undefined),
    },
  };
}

// ─── Shared OpenAI-shaped types for the gateway surface ─────────────────────

export interface OpenAiFileObject {
  id: string;
  object: string;
  bytes: number;
  created_at: number;
  filename: string;
  purpose: string;
  status: string;
  content_type?: string | null;
}

export interface OpenAiListResponse<T> {
  object: string;
  data: T[];
}

export type ChatContentPart =
  | { type: "text"; text: string }
  | { type: "image_url"; image_url: { url: string } };

export interface ChatToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

export interface ChatCompletionMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string | ChatContentPart[] | null;
  tool_calls?: ChatToolCall[];
  tool_call_id?: string;
}

export interface ChatCompletionRequest {
  model: string;
  messages: ChatCompletionMessage[];
  temperature?: number;
  top_p?: number;
  max_tokens?: number;
  stop?: string[];
  presence_penalty?: number;
  frequency_penalty?: number;
  seed?: number;
  stream?: boolean;
  stream_options?: { include_usage: boolean };
  tools?: unknown[];
  tool_choice?: unknown;
  response_format?: unknown;
}

export interface ChatCompletionChunk {
  id?: string;
  object?: string;
  created?: number;
  model?: string;
  choices?: Array<{
    index: number;
    delta?: {
      role?: string;
      content?: string;
      tool_calls?: Array<{
        index?: number;
        id?: string;
        type?: string;
        function?: { name?: string; arguments?: string };
      }>;
    };
    message?: { role?: string; content?: string | null; tool_calls?: ChatToolCall[] };
    finish_reason?: string | null;
  }>;
  usage?: {
    prompt_tokens?: number;
    completion_tokens?: number;
    total_tokens?: number;
  };
}

export interface OpenAiBatchRequestCounts {
  total: number;
  completed: number;
  failed: number;
}

export interface OpenAiBatch {
  id: string;
  object: string;
  status:
    | "validating"
    | "in_progress"
    | "finalizing"
    | "completed"
    | "failed"
    | "expired"
    | "cancelling"
    | "cancelled"
    | string;
  endpoint: string;
  input_file_id: string;
  completion_window: string;
  output_file_id?: string | null;
  error_file_id?: string | null;
  request_counts: OpenAiBatchRequestCounts;
  created_at: number;
  expires_at?: number | null;
  completed_at?: number | null;
  failed_at?: number | null;
  cancelled_at?: number | null;
  metadata?: Record<string, unknown> | null;
}

// ─── Cloud playground key (D1) ──────────────────────────────────────────────
//
// On Allternit Cloud the Playground calls the public /v1/chat/completions
// with a short-lived project key minted by the console. The key lives in
// memory only: never localStorage, never /api/v1/gateway/keys.

export interface PlaygroundKey {
  key: string;
  /** Epoch milliseconds. */
  expiresAt: number;
  projectId: string;
}

let playgroundKey: PlaygroundKey | null = null;
let minting: { projectId: string; promise: Promise<PlaygroundKey> } | null = null;

/** Re-mint this long before the server-side expiry. */
const EXPIRY_MARGIN_MS = 60_000;

export function forgetPlaygroundKey(): void {
  playgroundKey = null;
}

export async function mintPlaygroundKey(projectId: string): Promise<PlaygroundKey> {
  if (minting && minting.projectId === projectId) return minting.promise;
  const promise = api
    .post<{ key: string; expires_at: string | number }>(
      `/api/v1/platform/projects/${encodeURIComponent(projectId)}/playground-key`
    )
    .then((res) => {
      const parsed =
        typeof res.expires_at === "number"
          ? res.expires_at < 1e12
            ? res.expires_at * 1000
            : res.expires_at
          : Date.parse(res.expires_at);
      const minted: PlaygroundKey = {
        key: res.key,
        expiresAt: Number.isFinite(parsed) ? parsed : Date.now() + 14 * 60_000,
        projectId,
      };
      playgroundKey = minted;
      return minted;
    })
    .finally(() => {
      minting = null;
    });
  minting = { projectId, promise };
  return promise;
}

/** The in-memory key for `projectId`, minted or re-minted when missing, expired or `force`d. */
export async function getPlaygroundKey(projectId: string, force = false): Promise<string> {
  const cur = playgroundKey;
  if (
    !force &&
    cur &&
    cur.projectId === projectId &&
    cur.expiresAt - EXPIRY_MARGIN_MS > Date.now()
  ) {
    return cur.key;
  }
  return (await mintPlaygroundKey(projectId)).key;
}

export function gatewayBaseUrl(): string {
  return String(
    import.meta.env.VITE_ALLTERNIT_GATEWAY_URL || "https://api.allternit.com"
  ).replace(/\/+$/, "");
}

/** Register-1 message for a failed gateway call. */
export function describeGatewayError(status: number, serverMessage?: string): string {
  const detail = serverMessage ? ` (${serverMessage})` : "";
  switch (status) {
    case 401:
      return `The Playground key was rejected. Run again to get a fresh key.${detail}`;
    case 403:
      return `This project's key is not allowed to use that model or feature.${detail}`;
    case 402:
      return `Your organization is out of credits. Add credits on the Billing page, then run again.${detail}`;
    case 429:
      return `Too many requests or a rate limit was reached. Wait a moment, then run again.${detail}`;
    default:
      return serverMessage || `The request failed with HTTP ${status}.`;
  }
}

export interface GatewayChatResult {
  status: number;
  headers: Record<string, string>;
  latencyMs: number;
  /** Final response body: the JSON body, or the stream assembled by the caller. */
  json: unknown;
}

/**
 * POST /v1/chat/completions with a Bearer key. Throws GatewayHttpError on a
 * non-2xx. For `stream: true` the caller passes `onChunk` and gets each SSE
 * `data:` payload parsed; for non-stream the JSON body is returned.
 */
export class GatewayHttpError extends Error {
  constructor(
    message: string,
    public status: number,
    public headers: Record<string, string>,
    public body: unknown,
    public latencyMs: number
  ) {
    super(message);
    this.name = "GatewayHttpError";
  }
}

export async function gatewayChat(
  key: string,
  body: ChatCompletionRequest,
  opts: { signal?: AbortSignal; onChunk?: (chunk: ChatCompletionChunk) => void }
): Promise<GatewayChatResult> {
  return gatewayPost("/v1/chat/completions", key, body as unknown as Record<string, unknown>, {
    signal: opts.signal,
    onEvent: opts.onChunk ? (_name, data) => opts.onChunk?.(data as ChatCompletionChunk) : undefined,
  });
}

/** One parsed SSE event: the `event:` name ("message" when absent) and the JSON `data:` payload. */
export type GatewayStreamEvent = { event: string; data: unknown };

/**
 * POST a Platform API body (`/v1/chat/completions`, `/v1/responses`,
 * `/v1/messages`) with a Bearer key. Same error and streaming rules as
 * gatewayChat; for `stream: true` each `data:` line is parsed and handed to
 * `onEvent` with the event name that preceded it (chat completions send none).
 */
export async function gatewayPost(
  path: string,
  key: string,
  body: Record<string, unknown>,
  opts: { signal?: AbortSignal; onEvent?: (event: string, data: unknown) => void }
): Promise<GatewayChatResult> {
  const started = performance.now();
  const response = await fetch(`${gatewayBaseUrl()}${path}`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${key}`,
      "Content-Type": "application/json",
      Accept: body.stream ? "text/event-stream" : "application/json",
    },
    body: JSON.stringify(body),
    signal: opts.signal,
  });
  const headers: Record<string, string> = {};
  response.headers.forEach((v, k) => {
    headers[k] = v;
  });

  if (!response.ok) {
    const errBody = await response.json().catch(() => null);
    const nested =
      errBody && typeof errBody.error === "object" && errBody.error ? errBody.error : null;
    const msg: string | undefined =
      nested?.message ??
      (typeof errBody?.error === "string" ? errBody.error : undefined) ??
      errBody?.message;
    throw new GatewayHttpError(
      describeGatewayError(response.status, msg),
      response.status,
      headers,
      errBody,
      Math.round(performance.now() - started)
    );
  }

  if (!body.stream || !response.body) {
    const json = await response.json();
    return {
      status: response.status,
      headers,
      latencyMs: Math.round(performance.now() - started),
      json,
    };
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let eventName = "message";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let nl: number;
      while ((nl = buffer.indexOf("\n")) >= 0) {
        const line = buffer.slice(0, nl).trim();
        buffer = buffer.slice(nl + 1);
        if (line === "") {
          eventName = "message";
          continue;
        }
        if (line.startsWith("event:")) {
          eventName = line.slice(6).trim() || "message";
          continue;
        }
        if (!line.startsWith("data:")) continue;
        const payload = line.slice(5).trim();
        if (payload === "[DONE]") {
          return {
            status: response.status,
            headers,
            latencyMs: Math.round(performance.now() - started),
            json: undefined,
          };
        }
        let parsed: unknown;
        try {
          parsed = JSON.parse(payload);
        } catch {
          // keep-alive or comment payload
          continue;
        }
        try {
          opts.onEvent?.(eventName, parsed);
        } catch {
          // A handler error drops this one event, as before; the stream goes on.
        }
      }
    }
  } finally {
    reader.cancel().catch(() => undefined);
  }
  return {
    status: response.status,
    headers,
    latencyMs: Math.round(performance.now() - started),
    json: undefined,
  };
}
