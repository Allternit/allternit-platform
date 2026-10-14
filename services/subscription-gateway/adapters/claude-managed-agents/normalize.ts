// Pure mapping from documented Managed Agents session events to gateway event drafts.
// Shapes: shared/managed-agents-events.md (event types), managed-agents-tools.md (evaluated_permission),
// managed-agents-client-patterns.md (tool_confirmation, idle gate). Unknown/noise events (span.*, usage) yield nothing.
import type { GatewayEvent } from "@allternit/agent-gateway";
import type { MaEvent } from "./client.js";

export interface Draft { type: GatewayEvent["type"]; source: GatewayEvent["source"]; suffix: string; payload: Record<string, unknown> }

const textOf = (content: unknown): string =>
  Array.isArray(content) ? content.map((b) => (b && (b as { type?: string }).type === "text" ? String((b as { text?: unknown }).text ?? "") : "")).join("") : "";

export const isToolUse = (e: MaEvent) => e.type === "agent.tool_use" || e.type === "agent.mcp_tool_use";
export const needsApproval = (e: MaEvent) => isToolUse(e) && e.evaluated_permission === "ask";

/** stop reason of an idle event, "" if none */
export const stopType = (e: MaEvent): string => String((e.stop_reason as { type?: string } | undefined)?.type ?? "");
/** The documented "turn is truly over" gate: terminated, or idle for any reason other than requires_action. */
export const isTurnEnd = (e: MaEvent) => e.type === "session.status_terminated" || (e.type === "session.status_idle" && stopType(e) !== "requires_action");
export const isPaused = (e: MaEvent) => e.type === "session.status_idle" && stopType(e) === "requires_action";

/**
 * `live`: the event arrived on the session's SSE stream as it happened. Managed Agents streams whole `agent.message`
 * events (no token-level deltas), so each one becomes one delta + completed pair; `buffered: true` marks a message that
 * was only recovered from history (backfill after a disconnect, adoption, or a read while no stream was open).
 */
export function normalize(e: MaEvent, opts: { live?: boolean } = {}): Draft[] {
  switch (e.type) {
    case "user.message":
      return [{ type: "agent.activity.started", source: "allternit", suffix: "user", payload: { state: "message_sent", text: textOf(e.content), queued: e.processed_at == null } }];
    case "agent.message": {
      const text = textOf(e.content);
      return [
        { type: "agent.message.delta", source: "vendor", suffix: "delta", payload: { text, buffered: !opts.live, granularity: "event" } },
        { type: "agent.message.completed", source: "vendor", suffix: "completed", payload: { text } },
      ];
    }
    case "agent.thinking": return [{ type: "agent.activity.started", source: "vendor", suffix: "thinking", payload: { state: "thinking" } }];
    case "agent.tool_use":
    case "agent.mcp_tool_use": {
      const call: Draft = { type: "agent.tool.called", source: "vendor", suffix: "call", payload: {
        phase: "use", toolUseId: e.id, name: e.name, mcp: e.type === "agent.mcp_tool_use", mcpServer: e.mcp_server_name, input: e.input,
        evaluatedPermission: e.evaluated_permission, evaluation: e.evaluation } };
      if (!needsApproval(e)) return [call];
      return [call, { type: "agent.approval.requested", source: "vendor", suffix: "approval", payload: {
        approvalId: e.id, authority: "vendor", action: `${String(e.name)} ${JSON.stringify(e.input ?? {})}`.slice(0, 500), toolName: e.name, evaluation: e.evaluation } }];
    }
    case "agent.tool_result":
    case "agent.mcp_tool_result":
      return [{ type: "agent.tool.called", source: "vendor", suffix: "result", payload: { phase: "result", toolUseId: e.tool_use_id ?? e.mcp_tool_use_id, isError: Boolean(e.is_error) } }];
    case "agent.custom_tool_use":
      // The agent waits (idle, requires_action) until Allternit answers with user.custom_tool_result.
      return [{ type: "agent.tool.called", source: "vendor", suffix: "call", payload: {
        phase: "use", custom: true, toolUseId: e.id, name: e.name, input: e.input, sessionThreadId: e.session_thread_id ?? null, awaiting: "allternit" } }];
    case "user.custom_tool_result":
      return [{ type: "agent.tool.called", source: "allternit", suffix: "result", payload: {
        phase: "result", custom: true, toolUseId: e.custom_tool_use_id, isError: Boolean(e.is_error), text: textOf(e.content) } }];
    case "user.tool_confirmation":
      return [{ type: "agent.approval.resolved", source: "allternit", suffix: "resolved", payload: {
        approvalId: e.tool_use_id, outcome: e.result === "allow" ? "approved" : "denied", authority: "vendor" } }];
    case "session.status_running": return [{ type: "agent.task.updated", source: "vendor", suffix: "status", payload: { kind: "session_status", status: "running" } }];
    case "session.status_idle": return [{ type: "agent.task.updated", source: "vendor", suffix: "status", payload: { kind: "session_status", status: "idle", stopReason: stopType(e) || null, final: isTurnEnd(e) } }];
    case "session.status_terminated": return [{ type: "agent.task.updated", source: "vendor", suffix: "status", payload: { kind: "session_status", status: "terminated", final: true } }];
    case "session.status_rescheduled": return [{ type: "agent.task.updated", source: "vendor", suffix: "status", payload: { kind: "session_status", status: "rescheduling" } }];
    case "session.error": return [{ type: "agent.health.changed", source: "vendor", suffix: "error", payload: { status: "degraded", errorType: (e.error as { type?: string } | undefined)?.type ?? "session_error" } }];
    default: return [];
  }
}

/** Event ids an idle(requires_action) is waiting on (tool confirmations and custom tool results). */
export const waitingOn = (e: MaEvent): string[] => {
  const ids = (e.stop_reason as { event_ids?: unknown } | undefined)?.event_ids;
  return Array.isArray(ids) ? ids.map(String) : [];
};
