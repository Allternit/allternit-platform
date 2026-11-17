/**
 * Settings routes (Primitives Wave D, D2). Server: allternit-api
 * `agent_settings_tools.rs`. The risk gate (contracts/settings/definitions.json)
 * is enforced there: confirm and locked keys are never applied by a runtime
 * call, and confirm keys are refused for runs an automation started. Writes go
 * through the existing preference functions (/agent-preferences,
 * /cowork-preferences). Whose settings they are comes from the session id,
 * never from model input.
 *
 * - `POST /bot-routing/settings/read` `{sessionId, keys?}` → `{settings: [...]}`.
 * - `POST /bot-routing/settings/write` `{sessionId, toolCallId, changes}` →
 *   `{changes: [{key, outcome, previous?, value?, reason?, settings_path?}]}`.
 *   `toolCallId` lets the sending client apply per-device keys exactly once.
 *
 * W2 stub: final signatures; lane S fills the shapes and error handling.
 * Free of runtime state and UI imports so bun tests can stub `fetch`.
 */
import { botRoutingPost } from "@/runtime/bots/app-bot-routing"
import type { Settings } from "@/runtime/primitives/settings"

export async function readSettings(sessionID: string, input: { keys?: string[] }): Promise<{ settings: Settings.ReadEntry[] }> {
  return botRoutingPost("/bot-routing/settings/read", { sessionId: sessionID, ...input }, 10_000)
}

export async function writeSettings(
  sessionID: string,
  input: { toolCallId: string; changes: Settings.Change[] },
): Promise<{ changes: Settings.ChangeResult[] }> {
  return botRoutingPost("/bot-routing/settings/write", { sessionId: sessionID, ...input }, 15_000)
}
