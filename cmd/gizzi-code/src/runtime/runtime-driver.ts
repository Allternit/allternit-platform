import type { LanguageModelV2StreamPart } from "@ai-sdk/provider"

export interface Attachment {
  filename: string
  mimeType: string
  content: string | Uint8Array
}

export interface AgentTask {
  taskId: string
  prompt: string
  model?: string
  cwd?: string
  env?: Record<string, string>
  systemPrompt?: string
  attachments?: Attachment[]
  /**
   * Gizzi session this task belongs to, when the task originates from a
   * session turn (set from the stream context by the subprocess language
   * model). Lets session-agnostic driver events (ACP permission requests)
   * resolve the session they belong to.
   */
  sessionID?: string
  /**
   * An MCP server (HTTP) the CLI should load for this task: gizzi's CLI tool
   * bridge, which gives the CLI the session's own tools (see CliBridge).
   */
  mcp?: { name: string; url: string; headers: Record<string, string> }
}

export interface TaskHandle {
  taskId: string
  runtimeId: string
  cliName: string
}

export type AgentEvent =
  | { type: "status"; status: "queued" | "running" | "completed" | "failed" | "cancelled" }
  | { type: "text_delta"; delta: string }
  /**
   * The CLI's own reasoning/thinking stream (ACP agent_thought_chunk, Claude
   * stream-json thinking blocks, Codex reasoning items). Kept separate from
   * text so every provider surfaces thinking the same way — never as reply text.
   */
  | { type: "reasoning_delta"; delta: string }
  /** The agent's own context report (ACP usage_update): tokens in context / window size. */
  | { type: "context"; used: number; size: number }
  | { type: "tool_call"; id: string; name: string; arguments: unknown }
  | { type: "tool_result"; id: string; content: string; isError?: boolean }
  | { type: "error"; error: unknown }
  | {
      type: "finish"
      finishReason: string
      usage?: { inputTokens: number; outputTokens: number; totalTokens: number }
    }

export interface ExecutionLog {
  taskId: string
  runtimeId: string
  cliName: string
  status: AgentEvent & { type: "status" } extends { status: infer S } ? S : never
  startedAt?: number
  finishedAt?: number
  events: AgentEvent[]
  usage?: { inputTokens: number; outputTokens: number; totalTokens: number }
  exitCode?: number
  errorMessage?: string
}

export interface RuntimeDriver {
  assign(task: AgentTask): Promise<TaskHandle>
  stream(handle: TaskHandle): AsyncIterable<AgentEvent>
  abort(handle: TaskHandle): Promise<void>
  inspect(handle: TaskHandle): Promise<ExecutionLog>
}
