/**
 * settings (Primitives Wave D, D2): read and change the user's settings.
 * Core in runtime/primitives/settings.ts; keys and risk classes in
 * contracts/settings/definitions.json. Every client. Permission: allow
 * (loop/agent.ts); the action is the pattern, and the plan agent denies
 * `write`, so writes are refused in plan mode. The settings card (Undo,
 * Confirm, Open Settings) rides in the tool_result metadata, as in Wave A.
 *
 * Whether a scheduled task started this turn comes from the last user
 * message's metadata (Automation.isAutomationRun), never from the model; such
 * a run can't ask for confirm keys.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Settings } from "@/runtime/primitives/settings"
import { Automation } from "@/runtime/primitives/automation"
import DESCRIPTION from "@/runtime/tools/builtins/settings.txt"

export type SettingsMetadata = {
  action: Settings.Input["action"]
  /** write: one result per change, for the card */
  changes: Settings.ChangeResult[]
  /** write: the device applier applies each apply_on_device change once per id */
  toolCallId?: string
}

function fromAutomation(messages: Tool.Context["messages"]): boolean {
  const lastUser = messages.findLast((message) => message.info.role === "user")
  if (!lastUser || lastUser.info.role !== "user") return false
  const text = lastUser.parts.find((part) => part.type === "text")
  return Automation.isAutomationRun(lastUser.info.metadata, text?.type === "text" ? text.text : undefined)
}

function title(r: Settings.Result): string {
  if (r.action === "read") return "Settings"
  const changes = r.changes ?? []
  const applied = changes.filter((c) => c.outcome === "applied" || c.outcome === "apply_on_device").length
  if (applied === changes.length) return applied === 1 ? `Changed ${changes[0].label ?? changes[0].key}` : `Changed ${applied} settings`
  if (changes.some((c) => c.outcome === "needs_confirmation")) return "Settings change to confirm"
  return applied ? `Changed ${applied} of ${changes.length} settings` : "Settings not changed"
}

export const SettingsTool = Tool.define("settings", {
  description: DESCRIPTION,
  parameters: Settings.Input,
  async execute(params, ctx) {
    if (params.action === "write" && !params.changes?.length) {
      return {
        title: "Settings not changed",
        output: "write needs changes: [{key, value}], up to 5.",
        metadata: { action: params.action, changes: [] } as SettingsMetadata,
      }
    }
    await ctx.ask({
      permission: "settings",
      patterns: [params.action],
      always: [params.action],
      metadata: { action: params.action, keys: params.keys, changes: params.changes },
    })
    try {
      const r =
        params.action === "write"
          ? await Settings.write(ctx.sessionID, ctx.callID ?? "", params.changes ?? [], ctx.abort, {
              automationRun: fromAutomation(ctx.messages ?? []),
            })
          : await Settings.read(ctx.sessionID, params.keys, ctx.abort)
      const metadata: SettingsMetadata = { action: params.action, changes: r.changes ?? [] }
      if (r.toolCallId !== undefined) metadata.toolCallId = r.toolCallId
      return { title: title(r), output: JSON.stringify(r), metadata }
    } catch (error) {
      if (ctx.abort.aborted) throw error
      return {
        title: "Settings unavailable",
        output: error instanceof Error ? error.message : String(error),
        metadata: { action: params.action, changes: [] } as SettingsMetadata,
      }
    }
  },
})
