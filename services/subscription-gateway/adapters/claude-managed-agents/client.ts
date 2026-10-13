// The slice of the official `@anthropic-ai/sdk` client (v0.95) this adapter uses (client.beta.agents / environments /
// sessions / sessions.events / files), so tests can supply an offline fake. The real Anthropic client satisfies it
// structurally: list methods return PagePromise (async-iterable), `events.stream` returns APIPromise<Stream<...>>
// (async-iterable, with an AbortController), `files.list` resolves to a page with `data`.
import Anthropic from "@anthropic-ai/sdk";

export interface MaEvent { id: string; type: string; processed_at?: string | null; [k: string]: unknown }
export interface MaSession {
  id: string; status: string; archived_at?: string | null; agent?: { id?: string }; title?: string | null;
  created_at?: string; metadata?: Record<string, string>; [k: string]: unknown;
}
export interface MaAgent { id: string; name?: string; model?: unknown; archived_at?: string | null; [k: string]: unknown }
export interface MaEnvironment { id: string; name?: string; archived_at?: string | null; [k: string]: unknown }
export type MaTextBlock = { type: "text"; text: string };
export type MaSendEvent =
  | { type: "user.message"; content: MaTextBlock[] }
  | { type: "user.interrupt" }
  | { type: "user.tool_confirmation"; tool_use_id: string; result: "allow" | "deny"; deny_message?: string }
  | { type: "user.custom_tool_result"; custom_tool_use_id: string; content?: MaTextBlock[]; is_error?: boolean; session_thread_id?: string };

/** `events.list` query (EventListParams): ascending by created_at, optionally from a created_at cursor. */
export interface MaEventListParams { order?: "asc" | "desc"; "created_at[gte]"?: string; limit?: number }
/** What `events.stream` resolves to: the SDK's Stream (async-iterable SSE, abortable through its controller). */
export type MaEventStream = AsyncIterable<MaEvent> & { controller?: AbortController };

export interface MaClient {
  beta: {
    agents: {
      list(params?: { include_archived?: boolean; limit?: number }): AsyncIterable<MaAgent>;
      retrieve(agentId: string): Promise<MaAgent>;
    };
    environments: {
      list(params?: { include_archived?: boolean; limit?: number }): AsyncIterable<MaEnvironment>;
    };
    sessions: {
      create(params: Record<string, unknown>): Promise<MaSession>;
      retrieve(sessionId: string): Promise<MaSession>;
      list(params?: { agent_id?: string; include_archived?: boolean; limit?: number; order?: "asc" | "desc" }): AsyncIterable<MaSession>;
      archive(sessionId: string): Promise<unknown>;
      events: {
        list(sessionId: string, params?: MaEventListParams): AsyncIterable<MaEvent>;
        stream(sessionId: string): Promise<MaEventStream>;
        send(sessionId: string, params: { events: MaSendEvent[] }): Promise<{ data?: MaEvent[] }>;
      };
    };
    files: { list(params: { scope_id: string; betas?: string[] }): Promise<{ data: Array<{ id: string; filename: string }> }> };
  };
}

export type ClientFactory = (apiKey: string, opts: { baseURL?: string }) => MaClient;

/** Default factory: one official SDK client per credential (the SDK sets the managed-agents beta header itself). */
export const sdkClientFactory: ClientFactory = (apiKey, opts) =>
  new Anthropic({ apiKey, baseURL: opts.baseURL, maxRetries: 2 }) as unknown as MaClient;
