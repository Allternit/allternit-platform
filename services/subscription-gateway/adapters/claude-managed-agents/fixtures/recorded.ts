// Hand-built "recorded" Managed Agents event shapes, exactly as documented in
// shared/managed-agents-events.md (event types, processed_at), managed-agents-tools.md (evaluated_permission / evaluation)
// and managed-agents-client-patterns.md (idle gate, tool_confirmation). No live capture: no API key was available.
import type { MaEvent } from "../client.js";

const T0 = Date.parse("2026-09-29T12:00:00.000Z");
export const at = (n: number) => new Date(T0 + n * 1000).toISOString();
const text = (t: string) => [{ type: "text", text: t }];

export const ev = {
  userMessage: (id: string, t: number, msg: string, processed = true): MaEvent => ({ id, type: "user.message", content: text(msg), processed_at: processed ? at(t) : null }),
  running: (id: string, t: number): MaEvent => ({ id, type: "session.status_running", processed_at: at(t) }),
  thinking: (id: string, t: number): MaEvent => ({ id, type: "agent.thinking", processed_at: at(t) }),
  agentMessage: (id: string, t: number, msg: string): MaEvent => ({ id, type: "agent.message", content: text(msg), processed_at: at(t) }),
  toolUse: (id: string, t: number, name: string, input: unknown, permission: "allow" | "ask" | "deny"): MaEvent => ({
    id, type: "agent.tool_use", name, input, evaluated_permission: permission, evaluation: { type: permission === "ask" ? "always_ask" : "always_allow" }, processed_at: at(t) }),
  toolResult: (id: string, t: number, toolUseId: string): MaEvent => ({ id, type: "agent.tool_result", tool_use_id: toolUseId, content: text("ok"), is_error: false, processed_at: at(t) }),
  confirmation: (id: string, t: number, toolUseId: string, result: "allow" | "deny"): MaEvent => ({ id, type: "user.tool_confirmation", tool_use_id: toolUseId, result, processed_at: at(t) }),
  idle: (id: string, t: number, stop: "end_turn" | "requires_action", eventIds: string[] = []): MaEvent => ({
    id, type: "session.status_idle", stop_reason: stop === "requires_action" ? { type: "requires_action", event_ids: eventIds } : { type: "end_turn" }, processed_at: at(t) }),
  terminated: (id: string, t: number): MaEvent => ({ id, type: "session.status_terminated", processed_at: at(t) }),
  error: (id: string, t: number): MaEvent => ({ id, type: "session.error", error: { type: "unknown_error", message: "boom" }, processed_at: at(t) }),
  span: (id: string, t: number): MaEvent => ({ id, type: "span.model_request_end", is_error: false, model_usage: { input_tokens: 3571, output_tokens: 727, cache_creation_input_tokens: 0, cache_read_input_tokens: 6656 }, processed_at: at(t) }),
};

/** A plain turn: user -> running -> thinking -> span -> message -> idle(end_turn). */
export const RECORDED_TURN: MaEvent[] = [
  ev.userMessage("sevt_t1", 1, "Summarize the README"), ev.running("sevt_t2", 2), ev.thinking("sevt_t3", 3), ev.span("sevt_t4", 4),
  ev.agentMessage("sevt_t5", 5, "The README describes a gateway."), ev.idle("sevt_t6", 6, "end_turn"),
];

/** A turn that pauses for approval (always_ask): tool_use(ask) -> idle(requires_action). */
export const RECORDED_APPROVAL_PAUSE: MaEvent[] = [
  ev.userMessage("sevt_a1", 1, "Delete the build directory"), ev.running("sevt_a2", 2),
  ev.toolUse("sevt_a3", 3, "bash", { command: "rm -rf build" }, "ask"), ev.idle("sevt_a4", 4, "requires_action", ["sevt_a3"]),
];

/** Same session after a human allowed the call. */
export const RECORDED_APPROVAL_RESOLVED: MaEvent[] = [
  ...RECORDED_APPROVAL_PAUSE, ev.confirmation("sevt_a5", 5, "sevt_a3", "allow"), ev.toolResult("sevt_a6", 6, "sevt_a3"),
  ev.agentMessage("sevt_a7", 7, "Removed build/."), ev.idle("sevt_a8", 8, "end_turn"),
];

/** Session that ended (completion or error): error then terminated. */
export const RECORDED_TERMINATED: MaEvent[] = [ev.userMessage("sevt_x1", 1, "go"), ev.error("sevt_x2", 2), ev.terminated("sevt_x3", 3)];
