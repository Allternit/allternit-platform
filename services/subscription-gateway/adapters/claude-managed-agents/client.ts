// The slice of the official `@anthropic-ai/sdk` client this adapter uses (client.beta.agents / sessions / files),
// so tests can supply an offline fake. The real Anthropic client satisfies it structurally.
import Anthropic from "@anthropic-ai/sdk";

export interface MaEvent { id: string; type: string; processed_at?: string | null; [k: string]: unknown }
export interface MaSession { id: string; status: string; archived_at?: string | null; agent?: { id?: string }; [k: string]: unknown }
export interface MaAgent { id: string; name?: string; model?: unknown; archived_at?: string | null; [k: string]: unknown }
export type MaSendEvent = { type: "user.message"; content: Array<{ type: "text"; text: string }> } | { type: "user.interrupt" } |
  { type: "user.tool_confirmation"; tool_use_id: string; result: "allow" | "deny"; deny_message?: string };

export interface MaClient {
  beta: {
    agents: {
      list(params?: { include_archived?: boolean; limit?: number }): AsyncIterable<MaAgent>;
      retrieve(agentId: string): Promise<MaAgent>;
    };
    sessions: {
      create(params: Record<string, unknown>): Promise<MaSession>;
      retrieve(sessionId: string): Promise<MaSession>;
      archive(sessionId: string): Promise<unknown>;
      events: {
        list(sessionId: string, params?: { order?: "asc" | "desc" }): AsyncIterable<MaEvent>;
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
