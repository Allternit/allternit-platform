/**
 * Settings routes (Primitives Wave D, D2). Server: allternit-api
 * `agent_settings_tools.rs`. The risk gate (contracts/settings/definitions.json)
 * is enforced there: confirm and locked keys are never applied by a runtime
 * call, and confirm keys are refused for runs an automation started. Writes go
 * through the existing preference handlers (/agent-preferences,
 * /cowork-preferences). Whose settings they are comes from the session id,
 * never from model input.
 *
 * - `POST /bot-routing/settings/read` `{sessionId, keys?}` →
 *   `{settings: [definition + value?], unknown: [key]}`.
 * - `POST /bot-routing/settings/write` `{sessionId, toolCallId, changes, automationRun?}` →
 *   `{toolCallId, changes: [{key, label, outcome, scope, value?, previous?, reason?, code?, settings_path?}]}`,
 *   one result per change, in order. `toolCallId` lets the sending client
 *   apply per-device keys exactly once. `automationRun` is set by the runtime
 *   from the turn's metadata, never by the model.
 *
 * Throws AppRoutingError (status, message, code) as every bot-routing client.
 * Free of runtime state and UI imports so bun tests can stub `fetch`.
 */
import { botRoutingPost } from "@/runtime/bots/app-bot-routing"
import type { Settings } from "@/runtime/primitives/settings"

export async function readSettings(
  sessionID: string,
  input: { keys?: string[] },
): Promise<{ settings: Settings.ReadEntry[]; unknown?: string[] }> {
  return botRoutingPost("/bot-routing/settings/read", { sessionId: sessionID, ...input }, 10_000)
}

export async function writeSettings(
  sessionID: string,
  input: { toolCallId: string; changes: Settings.Change[]; automationRun?: boolean },
): Promise<{ toolCallId?: string; changes: Settings.ChangeResult[] }> {
  return botRoutingPost("/bot-routing/settings/write", { sessionId: sessionID, ...input }, 15_000)
}
